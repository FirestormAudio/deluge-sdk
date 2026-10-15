//! RUSB1 FIFO port operations.
//!
//! This module wraps the three hardware FIFO ports (CFIFO, D0FIFO, D1FIFO)
//! and provides the `sw_to_hw_fifo` / `hw_to_sw_fifo` byte-copy routines
//! that handle the RUSB1's MBW (memory bus width) switching rules.
//!
//! Routing policy (as in TinyUSB `dcd_rusb1.c`):
//! - Pipe 0 (DCP / control)         → CFIFO
//! - Pipes 1–2 (ISO)                → D0FIFO  (dedicated for audio)
//! - Pipes 3–15 (bulk / interrupt)  → D1FIFO
//!
//! ## MBW rules (TRM §28.3.8)
//! The MBW field in the FIFO SEL register must not change once a FIFO read
//! has begun.  MBW=32 is set together with CURPIPE and never narrowed; sub-word
//! tails use byte-lane stores (write) or unpack one extra word (read).
//!
//! ## RZ/A1 D1FIFO quirk
//! Writing D1FIFOSEL re-triggers the FIFO port switching state machine even
//! if only MBW changes, and a read immediately after the MBW write returns
//! `0xFF`, so MBW is never changed after selection.

use super::regs::{
    FIFOCTR_BCLR, FIFOCTR_BVAL, FIFOCTR_DTLN_MASK, FIFOCTR_FRDY, FIFOSEL_CURPIPE_MASK,
    FIFOSEL_MBW_MASK, FIFOSEL_MBW_SHIFT, MBW_32,
};
use crate::pac::common::{RW, Reg};
use crate::pac::usb::Usb;

/// Hardware FIFO port: data register, its byte lanes, SEL register and CTR register.
pub struct FifoPort {
    /// The 32-bit data register (CFIFO / D0FIFO / D1FIFO).
    pub data: Reg<u32, RW>,
    /// The data register's four byte lanes, at byte addresses `data + 0` … `data + 3`.
    pub lanes: [Reg<u8, RW>; 4],
    /// The 16-bit select register (CFIFOSEL / D0FIFOSEL / D1FIFOSEL).
    pub sel: Reg<u16, RW>,
    /// The 16-bit control register (CFIFOCTR / D0FIFOCTR / D1FIFOCTR).
    pub ctr: Reg<u16, RW>,
}

// Safety: FifoPort names memory-mapped registers; it holds no memory of its own.
unsafe impl Send for FifoPort {}

impl FifoPort {
    /// The CFIFO port of USB port `u`.
    pub fn cfifo(u: Usb) -> Self {
        Self {
            data: u.cfifo(),
            lanes: [u.cfifo_b(0), u.cfifo_b(1), u.cfifo_b(2), u.cfifo_b(3)],
            sel: u.cfifosel(),
            ctr: u.cfifoctr(),
        }
    }

    /// The D0FIFO port of USB port `u`.
    pub fn d0fifo(u: Usb) -> Self {
        Self {
            data: u.d0fifo(),
            lanes: [u.d0fifo_b(0), u.d0fifo_b(1), u.d0fifo_b(2), u.d0fifo_b(3)],
            sel: u.d0fifosel(),
            ctr: u.d0fifoctr(),
        }
    }

    /// The D1FIFO port of USB port `u`.
    pub fn d1fifo(u: Usb) -> Self {
        Self {
            data: u.d1fifo(),
            lanes: [u.d1fifo_b(0), u.d1fifo_b(1), u.d1fifo_b(2), u.d1fifo_b(3)],
            sel: u.d1fifosel(),
            ctr: u.d1fifoctr(),
        }
    }
}

// ---------------------------------------------------------------------------
// Pipe → FIFO port routing
// ---------------------------------------------------------------------------

/// Return the correct [`FifoPort`] for `pipe_num`.
///
/// Routing: pipe 0 → CFIFO, pipes 1-2 → D0FIFO, pipes 3+ → D1FIFO.
pub fn fifo_for_pipe(u: Usb, pipe_num: usize) -> FifoPort {
    match pipe_num {
        0 => FifoPort::cfifo(u),
        1 | 2 => FifoPort::d0fifo(u),
        _ => FifoPort::d1fifo(u),
    }
}

// ---------------------------------------------------------------------------
// MBW helpers
// ---------------------------------------------------------------------------

/// Change the MBW field in a FIFO SEL register without touching other bits.
fn set_mbw(sel: Reg<u16, RW>, mbw: u16) {
    sel.modify(|r| *r = (*r & !FIFOSEL_MBW_MASK) | ((mbw << FIFOSEL_MBW_SHIFT) & FIFOSEL_MBW_MASK));
}

// ---------------------------------------------------------------------------
// FIFO ready check
// ---------------------------------------------------------------------------

/// Returns `true` if the FIFO port is pointing at `pipe_num` AND is ready.
///
/// After writing CURPIPE in `fifo_select_pipe`, the hardware needs a few bus
/// cycles to settle.  This function busy-waits briefly for CURPIPE to match
/// and FRDY to assert.  Returns false if the FIFO is not ready within the
/// timeout — the caller should abort and retry on the next BRDY.
///
/// # Safety
/// `fifo` fields must be valid register pointers.
pub unsafe fn fifo_is_ready(fifo: &FifoPort, pipe_num: usize) -> bool {
    // Confirm CURPIPE has latched to the requested pipe (TRM §28: after a
    // CURPIPE write, read it back to confirm it updated), then poll FRDY.
    // Correct selection — including the deselect-first dance the TRM mandates
    // when switching to a *receiving* pipe — is handled by the caller via
    // `fifo_select_recv` so the port can't surface the prior pipe's data.
    for _ in 0..64 {
        let sel = fifo.sel.read();
        if (sel & FIFOSEL_CURPIPE_MASK) as usize != pipe_num {
            continue;
        }
        if (fifo.ctr.read() & FIFOCTR_FRDY) != 0 {
            return true;
        }
    }
    false
}

/// Select a **receiving-direction** (OUT) pipe on a DnFIFO port, following the
/// TRM §28 CURPIPE-change procedure: set CURPIPE to a different value (0 = no
/// pipe) first, confirm it latched, then select the target pipe.
///
/// On a shared D1FIFO, an OUT drain that selects its pipe in a single write can
/// observe the FRDY/data of the *previously selected* IN pipe and read its
/// staged TX bytes back onto the OUT path.  Deselecting first resets the port
/// state machine so the subsequent read reflects only the target pipe.
///
/// # Safety
/// `fifo` must be a valid DnFIFO port; `pipe_num` must be a DnFIFO pipe (≥ 1).
pub unsafe fn fifo_select_recv(fifo: &FifoPort, pipe_num: usize) {
    unsafe {
        // Step 1: deselect (CURPIPE = 0 → "no pipe" on D0/D1FIFO) and confirm.
        fifo_select_pipe(fifo, 0, false);
        for _ in 0..64 {
            if (fifo.sel.read() & FIFOSEL_CURPIPE_MASK) == 0 {
                break;
            }
        }
        // Step 2: select the target receiving pipe.
        fifo_select_pipe(fifo, pipe_num, false);
    }
}

/// Return the number of valid data bytes waiting in the FIFO.
///
/// # Safety
/// `fifo` must be valid.
pub unsafe fn fifo_dtln(fifo: &FifoPort) -> u16 {
    fifo.ctr.read() & FIFOCTR_DTLN_MASK
}

/// Issue BCLR (clear the FIFO buffer) on `fifo`.
///
/// # Safety
/// `fifo` must be valid.
pub unsafe fn fifo_bclr(fifo: &FifoPort) {
    fifo.ctr.write_value(FIFOCTR_BCLR);
}

/// Issue BVAL (commit the write buffer) on `fifo`.
///
/// # Safety
/// `fifo` must be valid.
pub unsafe fn fifo_bval(fifo: &FifoPort) {
    fifo.ctr.write_value(FIFOCTR_BVAL);
}

// ---------------------------------------------------------------------------
// Software → hardware FIFO (write path: IN endpoint or control IN)
// ---------------------------------------------------------------------------

/// Copy `len` bytes from `buf` into the hardware FIFO.
///
/// Uses 32-bit FIFO access throughout: the 4-byte body via native `u32` stores
/// and the 1-3 byte tail via 8-bit lane stores (MBW stays 32 — it is never
/// narrowed).  This follows the Linux `renesas_usbhs` PIO push; TinyUSB's
/// `dcd_rusb1.c` uses the wrong tail lane.
///
/// # Safety
/// - `buf` must be valid for `len` byte reads.
/// - The FIFO must have been selected via [`fifo_select_pipe`] (CURPIPE + MBW=32)
///   before calling.
pub unsafe fn sw_to_hw_fifo(fifo: &FifoPort, buf: *const u8, len: usize) {
    unsafe {
        // 32-bit FIFO access for the body.  With BIGEND=0 (the reset default,
        // never overridden) Table 28.7 maps byte N+0 → bits[7:0], i.e. little
        // endian matching the ARM, so a native `u32` write emits bytes in order
        // with no swap.
        //
        // Reassert MBW=32 rather than trusting the caller's select width.
        // Callers already select CURPIPE | MBW=32, so this is a same-value write
        // and does not re-trigger the port switch.
        set_mbw(fifo.sel, MBW_32);
        let mut p = buf;
        let mut rem = len;
        while rem >= 4 {
            let w = (p as *const u32).read_unaligned();
            fifo.data.write_value(w);
            p = p.add(4);
            rem -= 4;
        }

        // 1-3 byte tail.  Only non-multiple-of-4 transfers reach here (e.g. a
        // 13-byte CSW); block data is always 512-byte aligned.
        //
        // Do NOT narrow MBW: leave it at 32 and emit each remaining byte as an
        // 8-bit store to the correct byte lane.  Per TRM Table 28.9 (8-bit
        // access, BIGEND=0) the valid byte for the RZ/A1L sits on bits[31:24],
        // i.e. CPU byte-address `data + 3`.  As in the Linux `renesas_usbhs` PIO
        // push (cfifo_byte_addr=0 → `addr + (3 - (i & 3))`), byte `i` of the
        // tail goes to lane `3 - (i & 3)`.  A byte in bits[7:0] would be
        // ignored: the hardware latches bits[31:24].
        if rem > 0 {
            let mut i = 0usize;
            while rem > 0 {
                fifo.lanes[3 - (i & 3)].write_value(*p);
                p = p.add(1);
                rem -= 1;
                i += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hardware → software FIFO (read path: OUT endpoint or control OUT)
// ---------------------------------------------------------------------------

/// Copy `len` bytes from the hardware FIFO into `buf`.
///
/// Reads 32-bit words at MBW=32; with BIGEND=0 the bytes arrive little-endian,
/// already in memory order.  A 1-3 byte tail is taken from the low bytes of one
/// extra word read.
///
/// # Safety
/// - `buf` must be valid for `len` byte writes.
/// - The FIFO must have been selected with [`fifo_select_pipe`] (CURPIPE +
///   MBW=32) before calling.
pub unsafe fn hw_to_sw_fifo(fifo: &FifoPort, buf: *mut u8, len: usize) {
    unsafe {
        if len == 0 {
            return;
        }

        // 32-bit reads.  MBW=32 was established by the preceding `fifo_select_pipe`
        // (whose `fifo_is_ready` poll absorbs the port-switch settle), so we do
        // NOT change MBW here — avoiding the documented "first read after an MBW
        // write returns 0xFF" hazard.  BIGEND=0 (Table 28.7) maps byte N+0 →
        // bits[7:0], i.e. little endian, so the word's bytes are already in order.
        let mut p = buf;
        let mut rem = len;
        while rem >= 4 {
            let w = fifo.data.read();
            (p as *mut u32).write_unaligned(w);
            p = p.add(4);
            rem -= 4;
        }

        // 1-3 byte tail: read one more 32-bit word and copy its low `rem` bytes
        // (little-endian).  Only a non-multiple-of-4 packet reaches here — for MSC
        // that is just the 31-byte CBW, whose packet length equals the FIFO's
        // valid byte count, so the over-read is discarded by the caller's BCLR.
        if rem > 0 {
            let bytes = fifo.data.read().to_le_bytes();
            let mut i = 0;
            while i < rem {
                *p.add(i) = bytes[i];
                i += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Select a pipe on a FIFO port
// ---------------------------------------------------------------------------

/// Write the CURPIPE + MBW fields of `fifo.sel` together.
///
/// Selects 32-bit FIFO access (MBW=32) for both directions.  `sw_to_hw_fifo` /
/// `hw_to_sw_fifo` move the data as 32-bit words (BIGEND=0 ⇒ little-endian,
/// matching the ARM), handling a non-word-aligned tail without narrowing MBW.  The `fifo_is_ready` poll that follows this call absorbs the FIFO
/// port-switch settle so the first access does not hit the "0xFF after MBW
/// write" hazard.
///
/// `isel`: set the ISEL bit for CFIFO IN direction (ignored for D0/D1FIFO).
///
/// # Safety
/// `fifo.sel` must be valid.
pub unsafe fn fifo_select_pipe(fifo: &FifoPort, pipe_num: usize, isel: bool) {
    // ISEL selects the access direction for the *bidirectional* DCP buffer
    // only (CFIFO / pipe 0); see TRM §28.3 "The ISEL bit determines this only
    // for the DCP", and regs `FIFOSEL_ISEL` ("CFIFOSEL only").  For D0/D1FIFO
    // the direction is fixed by PIPECFG.DIR, and writing the ISEL bit there
    // flips the FIFO-port DIR, raising a *spurious* BRDY on every IN select
    // (TRM §28.4.2(2): "DIR bit changed 0→1").  That spurious BRDY marks the
    // single-packet IN transfer complete before the host has actually read
    // it, so `write()` returns early.  Only set it for the DCP.
    let isel_bit: u16 = if isel && pipe_num == 0 { 0x0020 } else { 0 };
    // 32-bit access for both directions; the copy routines handle any
    // non-word-aligned tail.
    let val: u16 = (pipe_num as u16) | (MBW_32 << FIFOSEL_MBW_SHIFT) | isel_bit;
    fifo.sel.write_value(val);
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::mmio::test;
    use crate::usb::regs::usb;

    #[test]
    fn sw_to_hw_roundtrip() {
        // The unaligned source reads in sw_to_hw_fifo don't panic for any length.
        test::reset();
        let src = [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let fifo = FifoPort::cfifo(usb(0));
        for len in 0..=7 {
            unsafe {
                sw_to_hw_fifo(&fifo, src.as_ptr(), len);
            }
        }
    }

    #[test]
    fn sw_to_hw_tail_uses_bigend0_lanes() {
        // TRM Table 28.9 (8-bit access, BIGEND=0): the valid byte lane for the
        // RZ/A1L is bits[31:24] = CPU byte-address base+3, and tail byte `i`
        // lands on lane `3 - (i & 3)` (matching Linux renesas_usbhs,
        // cfifo_byte_addr=0).  Write a pure 3-byte tail (no 4-byte body) to CFIFO.
        test::reset();
        let fifo = FifoPort::cfifo(usb(0));
        let src = [0xAAu8, 0xBB, 0xCC];
        unsafe { sw_to_hw_fifo(&fifo, src.as_ptr(), 3) };
        // src[0]→lane3, src[1]→lane2, src[2]→lane1.
        assert_eq!(
            test::peek8(0xE801_0017),
            0xAA,
            "tail byte 0 must land on lane 3 (bits[31:24])"
        );
        assert_eq!(
            test::peek8(0xE801_0016),
            0xBB,
            "tail byte 1 must land on lane 2"
        );
        assert_eq!(
            test::peek8(0xE801_0015),
            0xCC,
            "tail byte 2 must land on lane 1"
        );
        assert_eq!(
            test::peek8(0xE801_0014),
            0x00,
            "lane 0 (bits[7:0]) is the prohibited lane, untouched"
        );
    }

    #[test]
    fn a_packet_tail_goes_to_cfifo_byte_lanes_through_the_seam() {
        test::reset();
        let port = FifoPort::cfifo(usb(0));
        unsafe { sw_to_hw_fifo(&port, [1u8, 2, 3, 4, 5, 6].as_ptr(), 6) };
        let w: Vec<_> = test::log()
            .iter()
            .filter(|a| a.write && (0xE801_0014..0xE801_0018).contains(&a.addr))
            .map(|a| (a.addr, a.width))
            .collect();
        // One 32-bit word, then bytes 5 and 6 into lanes 3 and 2 (BIGEND clear).
        assert_eq!(w, [(0xE801_0014, 4), (0xE801_0017, 1), (0xE801_0016, 1)]);
    }

    #[test]
    fn hw_to_sw_sub_word() {
        // Sub-word tail: one word read at MBW=32, low bytes extracted.
        test::reset();
        test::poke32(0xE801_0014, 0x0403_0201);
        let fifo = FifoPort::cfifo(usb(0));
        let mut buf = [0u8; 3];
        unsafe {
            hw_to_sw_fifo(&fifo, buf.as_mut_ptr(), 3);
        }
        assert_eq!(buf, [0x01, 0x02, 0x03]);
    }
}
