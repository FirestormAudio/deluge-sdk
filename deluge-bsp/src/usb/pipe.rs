//! RUSB1 pipe layer — per-pipe state, allocation, and transfer primitives.
//!
//! This module manages the 16 hardware pipes (DCP + pipes 1-15):
//!
//! - [`PipeConfig`]: describes how a pipe is configured (endpoint, type, MPS,
//!   packet buffer allocation).
//! - [`PipeState`]: runtime transfer state (active buffer pointer, remaining
//!   bytes, waker).
//! - [`PIPE_WAKERS`]: one [`AtomicWaker`] per pipe; the ISR wakes the
//!   appropriate waker when a transfer completes (BRDY/BEMP).
//! - [`BufAllocator`]: bump allocator over the 128 × 64-byte packet buffer.
//!
//! All mutable access to the per-pipe state in IRQ + task must go through
//! critical sections (`critical_section::with`).

use core::sync::atomic::AtomicU16;
use embassy_sync::waitqueue::AtomicWaker;

use super::fifo::{
    fifo_bclr, fifo_bval, fifo_dtln, fifo_for_pipe, fifo_is_ready, fifo_select_pipe,
    fifo_select_recv, hw_to_sw_fifo, sw_to_hw_fifo,
};
use super::regs::{
    PIPEBUF_BUFSIZE_SHIFT, PIPECFG_CNTMD, PIPECFG_DBLB, PIPECFG_DIR, PIPECFG_EPNUM_MASK,
    PIPECFG_SHTNAK, PIPECFG_TYPE_BULK, PIPECFG_TYPE_INTR, PIPECFG_TYPE_ISO, PIPECTR_ACLRM,
    PIPECTR_PID_BUF, PIPECTR_PID_NAK, PIPECTR_PID_STALL, PIPECTR_SQCLR, PIPEMAXP_MXPS_MASK,
    PIPEPERI_IFIS, PKT_BUF_BLOCKS, Rusb1Regs, pipectr_ptr, rd, wr,
};

// ---------------------------------------------------------------------------
// Pipe count
// ---------------------------------------------------------------------------

/// Total number of hardware pipes (DCP + pipes 1-15).
pub const PIPE_COUNT: usize = 16;

// ---------------------------------------------------------------------------
// Per-pipe async wakers
// ---------------------------------------------------------------------------

/// One waker per pipe.  The ISR calls `wake()` when a transfer finishes.
///
/// These are `'static` because ISR context has no lifetime.
pub static PIPE_WAKERS: [AtomicWaker; PIPE_COUNT] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const W: AtomicWaker = AtomicWaker::new();
    [W; PIPE_COUNT]
};

// ---------------------------------------------------------------------------
// Transfer-done / error flags set by the ISR
// ---------------------------------------------------------------------------

/// Bitmask: bit N = pipe N transfer has completed (BRDY/BEMP fired).
pub static PIPE_DONE: AtomicU16 = AtomicU16::new(0);

/// Bitmask: bit N = pipe N encountered a NRDY error.
pub static PIPE_NRDY: AtomicU16 = AtomicU16::new(0);

/// Bitmask: bit N = pipe N is an IN (device→host) pipe.  Set by
/// [`pipe_configure`].  The BEMP ISR uses this to distinguish non-zero IN pipes
/// from OUT pipes because BEMP is also reported for receive-side overflow/error
/// conditions.
pub static PIPE_IS_IN: AtomicU16 = AtomicU16::new(0);

// ---------------------------------------------------------------------------
// ISO OUT packet hook
// ---------------------------------------------------------------------------

/// Pipe number for which the ISO OUT hook is registered.  [`usize::MAX`] means
/// no hook is registered.
static mut ISO_OUT_HOOK_PIPE: usize = usize::MAX;

/// Optional callback invoked by the BRDY ISR immediately after an ISO OUT packet
/// is fully received, before the task waker is signalled.
///
/// `buf` points to the start of the received bytes; `len` is the byte count.
///
/// # Safety
/// Called from IRQ context on single-core ARMv7-A (IRQs already disabled).
static mut ISO_OUT_HOOK: Option<unsafe fn(*const u8, usize)> = None;

/// Register an ISO OUT packet callback for `pipe`.
///
/// The callback is invoked from the BRDY ISR with a pointer to the fully
/// received packet and its byte count, before the task waker is signalled.
/// This lets audio conversion run with zero scheduling latency.
///
/// # Safety
/// Must be called before the pipe is armed and before BRDY interrupts are
/// enabled for it.  Not interrupt-safe — call from task context only.
pub unsafe fn register_iso_out_hook(pipe: usize, cb: unsafe fn(*const u8, usize)) {
    unsafe {
        core::ptr::addr_of_mut!(ISO_OUT_HOOK_PIPE).write(pipe);
        core::ptr::addr_of_mut!(ISO_OUT_HOOK).write(Some(cb));
    }
}

// ---------------------------------------------------------------------------
// Active transfer state (written by task, read by ISR)
// ---------------------------------------------------------------------------

/// Runtime state of one pipe's in-progress transfer.
///
/// Written by the async `write`/`read` futures before they suspend;
/// read and mutated by the ISR during BRDY/BEMP handling.
///
/// Guarded by `critical_section` whenever ISR and task must both access it.
pub struct PipeXferState {
    /// Current position in the user buffer.  `NonNull::dangling()` when idle.
    pub buf: core::ptr::NonNull<u8>,
    /// Total transfer length in bytes.
    pub length: u16,
    /// Bytes remaining (counts down as packets are transferred).
    pub remaining: u16,
    /// Bytes actually transferred so far.  Used to report the final byte count
    /// to the task, because for ISO OUT the `done` path forces `remaining = 0`
    /// early (to allow the ISO guard to fire on re-entry) before `remaining`
    /// naturally reaches zero.
    pub transferred: u16,
    /// Max packet size (cached from PIPEMAXP to avoid register reads in ISR).
    pub mps: u16,
    /// Maximum bytes to commit to the FIFO per fill.  Equals `mps` for ordinary
    /// pipes; for a continuous-mode (CNTMD) bulk IN pipe it is the whole
    /// multi-packet buffer, so one fill stages several packets the SIE then
    /// streams back-to-back.  Must be a multiple of `mps`.
    pub buf_bytes: u16,
    /// Transfer type — needed by the BRDY ISR to distinguish ISO vs bulk behaviour.
    pub xfer_type: XferType,
}

// Safety: single-core bare metal — no actual data races.
// Access must still be protected with critical_section to prevent ISR
// interrupting a multi-step mutation in task context.
unsafe impl Send for PipeXferState {}
unsafe impl Sync for PipeXferState {}

impl PipeXferState {
    const IDLE: Self = Self {
        buf: core::ptr::NonNull::dangling(),
        length: 0,
        remaining: 0,
        transferred: 0,
        mps: 0,
        buf_bytes: 0,
        xfer_type: XferType::Bulk,
    };
}

/// Per-port array of transfer states.  Index 0 = DCP (pipe 0).
pub static PIPE_XFER: [critical_section::Mutex<core::cell::UnsafeCell<PipeXferState>>; PIPE_COUNT] = {
    use core::cell::UnsafeCell;
    use critical_section::Mutex;
    #[allow(clippy::declare_interior_mutable_const)]
    const IDLE: Mutex<UnsafeCell<PipeXferState>> = Mutex::new(UnsafeCell::new(PipeXferState::IDLE));
    [IDLE; PIPE_COUNT]
};

// ---------------------------------------------------------------------------
// Pipe configuration (stored after allocate, used by configure)
// ---------------------------------------------------------------------------

/// Transfer type tag for a pipe.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum XferType {
    Control,
    Bulk,
    Interrupt,
    Isochronous,
}

impl XferType {
    /// PIPECFG TYPE field value (0b00 = control for DCP, not programmed here).
    pub fn pipecfg_type_bits(self) -> u16 {
        match self {
            XferType::Control => 0,
            XferType::Bulk => PIPECFG_TYPE_BULK,
            XferType::Interrupt => PIPECFG_TYPE_INTR,
            XferType::Isochronous => PIPECFG_TYPE_ISO,
        }
    }
}

/// Static configuration for one pipe.
#[derive(Clone, Copy)]
pub struct PipeConfig {
    /// Endpoint number (1-15; 0 = DCP is special).
    pub ep_num: u8,
    /// True if IN (device → host).
    pub is_in: bool,
    /// Transfer type.
    pub xfer_type: XferType,
    /// Max packet size in bytes.
    pub mps: u16,
    /// Start block index in the 8 KB packet buffer (64-byte granularity).
    pub buf_start: u8,
    /// Number of 64-byte blocks allocated for this pipe.  For a continuous-mode
    /// pipe this is the whole multi-packet buffer (not a per-bank size).
    pub buf_blocks: u8,
    /// Enable double-buffering (for bulk OUT and ISO pipes).
    pub double_buf: bool,
    /// Enable continuous transfer mode (CNTMD) — a single buffer larger than
    /// the max packet size over which the SIE streams multiple packets.  Only
    /// valid for bulk pipes 1-5; mutually exclusive with `double_buf`.
    pub continuous: bool,
}

// ---------------------------------------------------------------------------
// Packet buffer allocator
// ---------------------------------------------------------------------------

/// Simple bump allocator over the 128 × 64-byte packet buffer.
///
/// Uses a 128-bit bitmask: bit N = block N is allocated.
/// Blocks 0-3 (0x00–0xFF) are reserved for DCP (pipe 0).
pub struct BufAllocator {
    /// Bitmask: 1 = allocated.
    bits: u128,
}

impl BufAllocator {
    /// Create a new allocator with the first 4 blocks reserved for DCP.
    pub const fn new() -> Self {
        // DCP uses blocks 0-3 (256 bytes = 4 × 64)
        Self { bits: 0b1111 }
    }

    /// Allocate `n_blocks` contiguous blocks.  Returns the start block index,
    /// or `None` if the packet buffer is full.
    pub fn alloc(&mut self, n_blocks: usize) -> Option<u8> {
        // Linear scan for first run of `n_blocks` free bits.
        'outer: for start in 0..=(PKT_BUF_BLOCKS - n_blocks) {
            for i in 0..n_blocks {
                if (self.bits >> (start + i)) & 1 != 0 {
                    continue 'outer;
                }
            }
            // Found a free run — mark as allocated.
            for i in 0..n_blocks {
                self.bits |= 1u128 << (start + i);
            }
            return Some(start as u8);
        }
        None
    }

    /// Number of currently-free 64-byte blocks in the packet buffer.
    pub fn free_blocks(&self) -> usize {
        PKT_BUF_BLOCKS - (self.bits.count_ones() as usize)
    }

    /// Free `n_blocks` blocks starting at `start`.
    pub fn free(&mut self, start: u8, n_blocks: usize) {
        for i in 0..n_blocks {
            self.bits &= !(1u128 << (start as usize + i));
        }
    }
}

impl Default for BufAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Pipe configuration (hardware write)
// ---------------------------------------------------------------------------

/// Write PIPESEL, PIPECFG, PIPEBUF, PIPEMAXP for pipe `n` (1-15).
///
/// Pipe 0 (DCP) is configured separately via DCPCFG / DCPMAXP.
///
/// # Safety
/// `regs` must be a valid pointer to the USB register block with USBE=1.
/// Caller must ensure no transfer is active on this pipe.
pub unsafe fn pipe_configure(regs: *mut Rusb1Regs, n: usize, cfg: &PipeConfig) {
    unsafe {
        debug_assert!((1..=15).contains(&n), "pipe_configure: invalid pipe number");

        // Deactivate the pipe before reconfiguring.
        wr(pipectr_ptr(regs, n), PIPECTR_PID_NAK);

        // Select the pipe and write configuration registers.
        wr(core::ptr::addr_of_mut!((*regs).pipesel), n as u16);

        // Record direction so the BRDY ISR can dispatch IN vs OUT (see PIPE_IS_IN).
        if cfg.is_in {
            PIPE_IS_IN.fetch_or(1u16 << n, core::sync::atomic::Ordering::Release);
        } else {
            PIPE_IS_IN.fetch_and(!(1u16 << n), core::sync::atomic::Ordering::Release);
        }

        let mut pipecfg = cfg.ep_num as u16 & PIPECFG_EPNUM_MASK;
        if cfg.is_in {
            pipecfg |= PIPECFG_DIR;
        }
        pipecfg |= cfg.xfer_type.pipecfg_type_bits();

        match cfg.xfer_type {
            XferType::Bulk => {
                if !cfg.is_in {
                    // SHTNAK: NAK on short packet (OUT), prevents spurious BRDYs.
                    pipecfg |= PIPECFG_SHTNAK;
                    // Double-buffer bulk OUT is explicitly disabled to avoid the
                    // BRDY race described in dcd_rusb1.c comments.
                } else if cfg.continuous {
                    // Continuous transfer mode: one buffer > MaxPacketSize over
                    // which the SIE streams several packets per micro-frame.
                    pipecfg |= PIPECFG_CNTMD;
                } else if cfg.double_buf {
                    pipecfg |= PIPECFG_DBLB;
                }
            }
            XferType::Isochronous => {
                pipecfg |= PIPECFG_DBLB; // ISO always double-buffered
            }
            _ => {}
        }

        wr(core::ptr::addr_of_mut!((*regs).pipecfg), pipecfg);

        // PIPEBUF: BUFNMB (start block) | BUFSIZE ((blocks - 1) in 64-byte units).
        let bufsize_field = ((cfg.buf_blocks as u16).saturating_sub(1)) << PIPEBUF_BUFSIZE_SHIFT;
        let pipebuf = cfg.buf_start as u16 | bufsize_field;
        wr(core::ptr::addr_of_mut!((*regs).pipebuf), pipebuf);

        // Log the actual packet-RAM footprint so overlaps are visible: a
        // double-buffered pipe occupies TWO banks of `buf_blocks` each, i.e.
        // blocks [buf_start, buf_start + banks*buf_blocks).  Two pipes whose
        // ranges intersect will wedge on the hardware.
        let banks: u16 = if cfg.double_buf || cfg.xfer_type == XferType::Isochronous {
            2
        } else {
            1
        };
        let blocks = cfg.buf_blocks as u16 * banks;
        log::debug!(
            "pipe {} ep={:#04x} {} {}: buf blocks [{}, {}) ({} blk x{}buf, mps={})",
            n,
            cfg.ep_num as u16 | if cfg.is_in { 0x80 } else { 0 },
            if cfg.is_in { "IN " } else { "OUT" },
            match cfg.xfer_type {
                XferType::Isochronous => "ISO",
                XferType::Bulk => "BULK",
                XferType::Interrupt => "INT",
                XferType::Control => "CTRL",
            },
            cfg.buf_start,
            cfg.buf_start as u16 + blocks,
            cfg.buf_blocks,
            banks,
            cfg.mps,
        );

        // PIPEMAXP: max packet size.
        wr(
            core::ptr::addr_of_mut!((*regs).pipemaxp),
            cfg.mps & PIPEMAXP_MXPS_MASK,
        );

        // PIPEPERI: set IFIS for ISO IN pipes (function controller mode).
        // IFIS=1 causes the hardware to flush the TX buffer if no IN token arrives
        // within the interval frame, preventing stale audio data from accumulating
        // (TRM §28.4.9(5)).
        let pipeperi = if cfg.xfer_type == XferType::Isochronous && cfg.is_in {
            PIPEPERI_IFIS
        } else {
            0
        };
        wr(core::ptr::addr_of_mut!((*regs).pipeperi), pipeperi);

        // Deselect pipe.
        wr(core::ptr::addr_of_mut!((*regs).pipesel), 0);
    }
}

// ---------------------------------------------------------------------------
// Transfer initiation helpers (called from driver / ISR)
// ---------------------------------------------------------------------------

/// Arm the IN direction of pipe `n` for a transfer (write first packet
/// to FIFO, set PID=BUF).
///
/// Returns `true` if the entire transfer fit in one packet (done immediately).
///
/// # Safety
/// `regs` must be valid.
pub unsafe fn pipe_xfer_in_start(regs: *mut Rusb1Regs, n: usize, mps: u16) -> bool {
    unsafe {
        critical_section::with(|cs| {
            let state = &mut *PIPE_XFER[n].borrow(cs).get();
            if state.remaining == 0 {
                state.buf = core::ptr::NonNull::dangling();
                return true;
            }
            if !fill_in_packet(regs, n, mps as usize, state) {
                return false; // FIFO port not ready — nothing written
            }
            state.remaining == 0
        })
    }
}

/// Write up to one `mps`-sized packet from pipe `n`'s active descriptor into its
/// FIFO.  Returns `true` if a packet (including a short final one) was committed,
/// `false` if the FIFO port was not ready (nothing written).
///
/// Shared by [`pipe_xfer_in_start`] (task context, first packet) and
/// [`pipe_xfer_in_bemp`] (ISR context, continuation packets) so the two paths
/// fill identically.  The caller owns the `state` borrow and interprets
/// `state.remaining` afterward.
///
/// # Safety
/// `regs` must be valid.  The caller must already hold the `PIPE_XFER[n]`
/// critical section (task context) or be running in ISR context.
unsafe fn fill_in_packet(
    regs: *mut Rusb1Regs,
    n: usize,
    mps: usize,
    state: &mut PipeXferState,
) -> bool {
    unsafe {
        let fifo = fifo_for_pipe(regs, n);
        fifo_select_pipe(&fifo, n, true); // ISEL=1 for IN

        if !fifo_is_ready(&fifo, n) {
            return false;
        }

        // Commit up to one whole buffer.  For an ordinary pipe `buf_bytes == mps`
        // so this stages a single packet; for a continuous-mode (CNTMD) pipe it
        // stages several packets at once, which the SIE then streams back-to-back.
        let cap = (state.buf_bytes as usize).max(mps);
        let len = (state.remaining as usize).min(cap);
        sw_to_hw_fifo(&fifo, state.buf.as_ptr(), len);

        // Transmit trigger (RZ/A1 TRM Table 28.11, DIR=1):
        //   (1) a fill that reaches the full buffer-plane size auto-transmits;
        //   (2) any *partial* fill must be committed by writing BVAL.
        // So BVAL whenever we did not exactly fill the buffer.  For an ordinary
        // pipe `cap == mps`, this reduces to "BVAL on a short packet" — the
        // original behaviour.  (Filling a whole number of packets that is still
        // less than the buffer needs BVAL too, or it would never be sent.)
        if len < cap {
            fifo_bval(&fifo);
        }

        state.buf = core::ptr::NonNull::new_unchecked(state.buf.as_ptr().add(len));
        state.remaining = state.remaining.saturating_sub(len as u16);
        true
    }
}

/// Called from BRDY ISR for an OUT pipe.  Reads available data from FIFO
/// into the active transfer buffer.  Returns `true` when the transfer is
/// complete (short packet or remaining == 0).
///
/// For non-ISO pipes the hardware PID is set to NAK before the FIFO is read
/// (preventing the SIE from writing new data while CPU reads), then re-armed
/// to BUF if the transfer is not yet done — matching the C `process_pipe_brdy`
/// / `pipe_xfer_out` reference implementation.
///
/// # Safety
/// `regs` must be valid.  Called from ISR context (critical section not needed
/// on single-core Cortex-A9 with IRQ disabled).
pub unsafe fn pipe_xfer_out_brdy(regs: *mut Rusb1Regs, n: usize) -> bool {
    unsafe {
        let state = &mut *{
            // Access state without critical_section — we're already in IRQ context
            // where the task cannot run (single-core).
            PIPE_XFER[n]
                .borrow(critical_section::CriticalSection::new())
                .get()
        };

        let is_iso = state.xfer_type == XferType::Isochronous;

        if state.remaining == 0 {
            // No active transfer buffer.  For ISO: do NOT BCLR — leave the packet
            // in the FIFO so the task can read it without missing it.  The task's
            // re-arm path (in driver.rs EndpointOut::read) will drain it directly
            // via fifo_is_ready check, or the next BRDY will deliver it once the
            // task sets up the transfer state.
            return false;
        }

        let ctr_ptr = pipectr_ptr(regs, n);

        // For non-ISO: NAK first to prevent the SIE from filling the FIFO while
        // the CPU is reading it (matches C `process_pipe_brdy` NAK-before-read).
        if !is_iso {
            let cur = rd(ctr_ptr);
            wr(
                ctr_ptr,
                (cur & !super::regs::PIPECTR_PID_MASK) | PIPECTR_PID_NAK,
            );
        }

        let fifo = fifo_for_pipe(regs, n);
        // Receiving (OUT) pipe selection.  For bulk pipes on the shared D1FIFO
        // (e.g. CDC OUT pipe 3 sharing the port with IN pipe 4) use the TRM
        // deselect-first CURPIPE-change procedure so this read cannot surface
        // the previously-selected IN pipe's staged data (the IN/OUT
        // cross-corruption).  ISO pipes live on their own D0FIFO and are
        // timing-critical for audio, so keep their original single select.
        if is_iso {
            fifo_select_pipe(&fifo, n, false);
        } else {
            fifo_select_recv(&fifo, n);
        }

        if !fifo_is_ready(&fifo, n) {
            // FIFO port not ready; re-arm and retry next BRDY.
            if !is_iso {
                let cur = rd(ctr_ptr);
                wr(
                    ctr_ptr,
                    (cur & !super::regs::PIPECTR_PID_MASK) | PIPECTR_PID_BUF,
                );
            }
            return false;
        }

        let vld = fifo_dtln(&fifo) as usize;
        let mps = state.mps as usize;
        let rem = state.remaining as usize;
        let len = rem.min(mps).min(vld);
        if len > 0 {
            hw_to_sw_fifo(&fifo, state.buf.as_ptr(), len);
            state.buf = core::ptr::NonNull::new_unchecked(state.buf.as_ptr().add(len));
            state.remaining = state.remaining.saturating_sub(len as u16);
            state.transferred += len as u16;
        }

        // Always BCLR after reading (matches C reference — not just on short packet).
        fifo_bclr(&fifo);

        let done = len < mps || state.remaining == 0;
        if done {
            // Invoke the ISO OUT hook (if registered for this pipe) before
            // resetting state.  `state.buf` has advanced by `transferred` bytes
            // since the transfer started; rewind to recover the packet start.
            if n == core::ptr::addr_of!(ISO_OUT_HOOK_PIPE).read()
                && let Some(hook) = core::ptr::addr_of!(ISO_OUT_HOOK).read()
            {
                let start = state.buf.as_ptr().sub(state.transferred as usize);
                hook(start, state.transferred as usize);
            }
            state.buf = core::ptr::NonNull::dangling();
            // Reset remaining to 0 so the ISO guard at the top of this function
            // fires correctly if BRDY re-enters before the task calls read() again.
            // (For ISO, done can be true via len<mps while remaining is still non-zero.)
            state.remaining = 0;
            return true;
        }

        // Transfer not yet complete; re-arm the pipe so the host can send the
        // next packet (non-ISO only — ISO stays at BUF/auto-managed).
        if !is_iso {
            let cur = rd(ctr_ptr);
            wr(
                ctr_ptr,
                (cur & !super::regs::PIPECTR_PID_MASK) | PIPECTR_PID_BUF,
            );
        }

        false
    }
}

/// Called from the IN completion ISR path.  Writes the next packet to FIFO,
/// or signals completion if the transfer is done.
///
/// Returns `true` when the transfer is fully complete.
///
/// # Safety
/// `regs` must be valid.  Called from ISR context.
pub unsafe fn pipe_xfer_in_bemp(regs: *mut Rusb1Regs, n: usize) -> bool {
    unsafe {
        let state = &mut *{
            PIPE_XFER[n]
                .borrow(critical_section::CriticalSection::new())
                .get()
        };

        if state.remaining == 0 {
            state.buf = core::ptr::NonNull::dangling();
            return true;
        }

        let mps = state.mps as usize;
        // Fill the next packet (shared with the task-context first-fill path).
        // A `false` return means the FIFO port wasn't ready; we report "not yet
        // done" and the next BEMP will retry, matching the prior behaviour.
        fill_in_packet(regs, n, mps, state);
        false
    }
}

// ---------------------------------------------------------------------------
// Pipe enable / disable
// ---------------------------------------------------------------------------

/// Set PID=BUF to allow data transfer on pipe `n`.
///
/// # Safety
/// `regs` must be valid.
pub unsafe fn pipe_enable(regs: *mut Rusb1Regs, n: usize) {
    unsafe {
        let ctr = pipectr_ptr(regs, n);
        let cur = rd(ctr);
        wr(
            ctr,
            (cur & !super::regs::PIPECTR_PID_MASK) | PIPECTR_PID_BUF,
        );
    }
}

/// Set PID=NAK to halt data transfer on pipe `n`.
///
/// # Safety
/// `regs` must be valid.
pub unsafe fn pipe_disable(regs: *mut Rusb1Regs, n: usize) {
    unsafe {
        let ctr = pipectr_ptr(regs, n);
        let cur = rd(ctr);
        wr(
            ctr,
            (cur & !super::regs::PIPECTR_PID_MASK) | PIPECTR_PID_NAK,
        );
    }
}

/// Set PID=STALL on pipe `n`.
///
/// # Safety
/// `regs` must be valid.
pub unsafe fn pipe_stall(regs: *mut Rusb1Regs, n: usize) {
    unsafe {
        let ctr = pipectr_ptr(regs, n);
        let cur = rd(ctr);
        wr(
            ctr,
            (cur & !super::regs::PIPECTR_PID_MASK) | PIPECTR_PID_STALL,
        );
    }
}

/// Clear the data toggle and FIFO on pipe `n`, then set PID=NAK.
///
/// # Safety
/// `regs` must be valid.
pub unsafe fn pipe_reset(regs: *mut Rusb1Regs, n: usize) {
    unsafe {
        let ctr = pipectr_ptr(regs, n);
        // Set ACLRM (auto clear FIFO and toggle) then clear it, per TRM.
        wr(ctr, PIPECTR_PID_NAK | PIPECTR_ACLRM);
        wr(ctr, PIPECTR_PID_NAK);
        wr(ctr, PIPECTR_PID_NAK | PIPECTR_SQCLR);
    }
}

// ---------------------------------------------------------------------------
// BRDY / BEMP interrupt enable helpers
// ---------------------------------------------------------------------------

/// Enable BRDY interrupt for pipe `n`.
///
/// # Safety
/// `regs` must be valid.
pub unsafe fn pipe_brdy_enable(regs: *mut Rusb1Regs, n: usize) {
    unsafe {
        let brdysts = core::ptr::addr_of_mut!((*regs).brdysts);
        let brdyenb = core::ptr::addr_of_mut!((*regs).brdyenb);
        // Clear any pending BRDY for this pipe before enabling.
        wr(brdysts, !((1u16) << n));
        let cur = rd(brdyenb);
        wr(brdyenb, cur | (1u16 << n));
    }
}

/// Disable BRDY interrupt for pipe `n`.
///
/// # Safety
/// `regs` must be valid.
pub unsafe fn pipe_brdy_disable(regs: *mut Rusb1Regs, n: usize) {
    unsafe {
        let brdyenb = core::ptr::addr_of_mut!((*regs).brdyenb);
        let cur = rd(brdyenb);
        wr(brdyenb, cur & !(1u16 << n));
    }
}

/// Enable BEMP interrupt for pipe `n`.
///
/// # Safety
/// `regs` must be valid.
pub unsafe fn pipe_bemp_enable(regs: *mut Rusb1Regs, n: usize) {
    unsafe {
        let bempsts = core::ptr::addr_of_mut!((*regs).bempsts);
        let bempenb = core::ptr::addr_of_mut!((*regs).bempenb);
        wr(bempsts, !((1u16) << n));
        let cur = rd(bempenb);
        wr(bempenb, cur | (1u16 << n));
    }
}

/// Disable BEMP interrupt for pipe `n`.
///
/// # Safety
/// `regs` must be valid.
pub unsafe fn pipe_bemp_disable(regs: *mut Rusb1Regs, n: usize) {
    unsafe {
        let bempenb = core::ptr::addr_of_mut!((*regs).bempenb);
        let cur = rd(bempenb);
        wr(bempenb, cur & !(1u16 << n));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn buf_allocator_basic() {
        let mut alloc = BufAllocator::new();
        // Blocks 0-3 are pre-allocated for DCP.
        let a = alloc.alloc(4).unwrap();
        assert_eq!(a, 4); // first free block is 4
        let b = alloc.alloc(4).unwrap();
        assert_eq!(b, 8);
        alloc.free(a, 4);
        let c = alloc.alloc(2).unwrap();
        assert_eq!(c, 4); // reuse freed blocks
    }

    #[test]
    fn buf_allocator_full() {
        let mut alloc = BufAllocator::new();
        // Fill everything after DCP (blocks 4-127 = 124 blocks).
        let start = alloc.alloc(124).unwrap();
        assert_eq!(start, 4);
        // Now no room for even 1 more block.
        assert!(alloc.alloc(1).is_none());
    }

    #[test]
    fn xfer_type_pipecfg_bits() {
        assert_eq!(XferType::Bulk.pipecfg_type_bits(), PIPECFG_TYPE_BULK);
        assert_eq!(XferType::Interrupt.pipecfg_type_bits(), PIPECFG_TYPE_INTR);
        assert_eq!(XferType::Isochronous.pipecfg_type_bits(), PIPECFG_TYPE_ISO);
        assert_eq!(XferType::Control.pipecfg_type_bits(), 0);
    }
}
