//! RUSB1 USB host-mode driver.
//!
//! ## Architecture
//!
//! ```text
//! application
//!   │  Rusb1HostDriver (implements embassy_usb_driver::host::UsbHostController)
//!   │    ├─ wait_for_device_event() ← Connected(Speed) / Disconnected
//!   │    ├─ bus_reset()             ← async, drives USBRST for ~20 ms
//!   │    └─ allocator()             ← returns Rusb1Allocator
//!   │
//!   │  Rusb1Allocator (implements UsbHostAllocator)
//!   │    └─ alloc_pipe()            ← returns Rusb1Pipe<T, D>
//!   │
//!   │  Rusb1Pipe<T, D> (implements UsbPipe<T, D>)
//!   │    ├─ control_in / control_out  ← setup + data + status ZLP
//!   │    ├─ request_in / request_out  ← non-control IN / OUT
//!   │    └─ reset_data_toggle()       ← back to DATA0
//!   │
//!   └─ hcd_int_handler(port)   ← GIC IRQ 73/74
//!         ├─ INTSTS1: ATTCH/DTCH → HCD_EVENTS + waker
//!         │           SACK/SIGN  → HCD_EVENTS + waker
//!         ├─ INTSTS0 BRDY → pipe0_in_brdy / pipe_brdy_in / pipe_brdy_out
//!         │         BEMP  → pipe0_out_bemp (control OUT continuation)
//!         └─         NRDY → HCD_PIPE_NRDY / HCD_PIPE_STALL + wakers
//! ```
//!
//! ## Pipe allocation (host mode)
//!
//! Host mode uses pipes 0-9 only, as TinyUSB's `hcd_rusb1.c` does.
//!
//! | Pipe | Purpose |
//! |------|---------|
//! | 0    | Control (DCP) |
//! | 1-2  | ISO, Bulk, or Interrupt over 512 bytes (as bulk) |
//! | 3-5  | Bulk, or Interrupt of 65-512 bytes (as bulk, MXPS 512) |
//! | 6-9  | Interrupt of 64 bytes or less |
//!
//! ## FIFO routing (host mode)
//!
//! - Pipe 0 (control): CFIFO
//! - Pipes 1-9 (all others): **D0FIFO** (unlike device mode, host never uses D1FIFO)
//!
//! ## PIPECFG.DIR in host mode
//!
//! DIR is inverted relative to device mode: DIR=1 means the host is sending
//! (USB OUT), DIR=0 means the host is receiving (USB IN), i.e.
//! `(1 ^ dir_in) << 4`.
//!
//! ## Reference
//!
//! TinyUSB `src/portable/renesas/rusb1/hcd_rusb1.c`.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};
use embassy_sync::waitqueue::AtomicWaker;
use embassy_time::Timer;
use embassy_usb_driver::host::{
    DeviceEvent, HostError, PipeError, SplitInfo, SplitSpeed, TimeoutConfig, UsbHostAllocator,
    UsbHostController, UsbPipe, pipe,
};
use embassy_usb_driver::{EndpointInfo, EndpointType, Speed};

use super::fifo::{FifoPort, fifo_bval, hw_to_sw_fifo, sw_to_hw_fifo};
use super::pipe::BufAllocator;
use super::regs::{
    BUSWAIT_VALUE, DCPCFG_DIR, DCPCFG_SHTNAK, DCPCTR_SUREQ, DCPCTR_SUREQCLR, DCPMAXP_DEVSEL_SHIFT,
    DCPMAXP_MXPS_MASK, DEVADD_HUBPORT_SHIFT, DEVADD_UPPHUB_SHIFT, DEVADD_USBSPD_FS,
    DEVADD_USBSPD_HS, DEVADD_USBSPD_LS, DVSTCTR0_RHST, DVSTCTR0_RHST_FS, DVSTCTR0_RHST_HS,
    DVSTCTR0_RHST_LS, DVSTCTR0_UACT, DVSTCTR0_USBRST, FIFOCTR_BCLR, FIFOCTR_BVAL,
    FIFOCTR_DTLN_MASK, FIFOCTR_FRDY, FIFOSEL_CURPIPE_MASK, FIFOSEL_DCLRM, FIFOSEL_DREQE,
    FIFOSEL_ISEL, FIFOSEL_MBW_SHIFT, INTENB0_BEMPE, INTENB0_BRDYE, INTENB0_NRDYE, INTENB1_ATTCHE,
    INTENB1_DTCHE, INTENB1_EOFERRE, INTENB1_SACKE, INTENB1_SIGNE, INTSTS0_BEMP, INTSTS0_BRDY,
    INTSTS0_NRDY, INTSTS1_ATTCH, INTSTS1_DTCH, INTSTS1_EOFERR, INTSTS1_SACK, INTSTS1_SIGN, MBW_32,
    PIPEBUF_BUFNMB_MASK, PIPEBUF_BUFSIZE_MASK, PIPEBUF_BUFSIZE_SHIFT, PIPECFG_DBLB,
    PIPECFG_EPNUM_MASK, PIPECFG_SHTNAK, PIPECFG_TYPE_BULK, PIPECFG_TYPE_INTR, PIPECFG_TYPE_ISO,
    PIPECTR_ACLRM, PIPECTR_BSTS, PIPECTR_PBUSY, PIPECTR_PID_BUF, PIPECTR_PID_MASK, PIPECTR_PID_NAK,
    PIPECTR_SQCLR, PIPECTR_SQSET, PIPEMAXP_DEVSEL_SHIFT, PIPEMAXP_MXPS_MASK, PIPEPERI_IITV_MASK,
    PKT_BUF_BLOCK_SIZE, SUSPMODE_SUSPM, SYSCFG_DCFM, SYSCFG_DPRPU, SYSCFG_DRPD, SYSCFG_HSE,
    SYSCFG_UPLLE, SYSCFG_USBE, SYSSTS0_LNST, pipectr, usb,
};
use crate::pac::usb::Usb;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of hardware pipes used in host mode (DCP=0, pipes 1-9).
const HCD_PIPE_COUNT: usize = 10;

/// Maximum device addresses supported (DEVADD0-5 → indices 0-5).
const HCD_MAX_DEV: usize = 6;

/// True iff `dev_addr` is a device address this driver can target.
///
/// Address 0 is the enumeration default address. The upper bound is
/// [`HCD_MAX_DEV`], which sizes `HcdAlloc::ctl_mps` and `HcdAlloc::ep_to_pipe` —
/// exceeding it would index out of bounds.
pub(crate) const fn dev_addr_supported(dev_addr: u8) -> bool {
    dev_addr > 0 && (dev_addr as usize) < HCD_MAX_DEV
}

// HCD_EVENTS bit layout.
const EVT_ATTACH: u8 = 1 << 0;
const EVT_DETACH: u8 = 1 << 1;
const EVT_SACK: u8 = 1 << 2; // setup ACK from device
const EVT_SIGN: u8 = 1 << 3; // setup ignored (NAK/error)

// FIFO SEL/CTR field layout is shared by CFIFO and DnFIFO and lives in
// `regs.rs` (CURPIPE [3:0], FRDY bit 13, DTLN [11:0] — TRM §28.3.8/28.3.9).

// ---------------------------------------------------------------------------
// Per-pipe ISR ↔ task state
// ---------------------------------------------------------------------------

struct HcdPipeXfer {
    /// User buffer pointer; null when no transfer is active.
    buf: *mut u8,
    /// Total requested byte count.
    length: u16,
    /// Bytes still to transfer.
    remaining: u16,
    /// Endpoint address (bit 7 = IN/OUT direction, bits 3:0 = EP number).
    ep_addr: u8,
    /// USB device address of the pipe's target.
    dev_addr: u8,
}

unsafe impl Send for HcdPipeXfer {}
unsafe impl Sync for HcdPipeXfer {}

impl HcdPipeXfer {
    const IDLE: Self = Self {
        buf: core::ptr::null_mut(),
        length: 0,
        remaining: 0,
        ep_addr: 0,
        dev_addr: 0,
    };
}

static HCD_XFER: [critical_section::Mutex<core::cell::UnsafeCell<HcdPipeXfer>>; HCD_PIPE_COUNT] = {
    use core::cell::UnsafeCell;
    use critical_section::Mutex;
    #[allow(clippy::declare_interior_mutable_const)]
    const IDLE: Mutex<UnsafeCell<HcdPipeXfer>> = Mutex::new(UnsafeCell::new(HcdPipeXfer::IDLE));
    [IDLE; HCD_PIPE_COUNT]
};

/// Bit N set → pipe N transfer completed.
static HCD_PIPE_DONE: AtomicU16 = AtomicU16::new(0);
/// Bit N set → pipe N transfer failed (NRDY/NAK).
static HCD_PIPE_NRDY: AtomicU16 = AtomicU16::new(0);
/// Bit N set → pipe N endpoint STALLed.
static HCD_PIPE_STALL: AtomicU16 = AtomicU16::new(0);
/// Bit N set → pipe N is an isochronous OUT pipe, whose NRDY reports an underrun
/// (a zero-length packet sent for want of data), not a failed transfer.
static HCD_PIPE_ISO_OUT: AtomicU16 = AtomicU16::new(0);
/// Bit N set → pipe N is an isochronous IN pipe: the interrupt handler keeps each packet in
/// [`ISO_IN`] until a read takes it.
static HCD_PIPE_ISO_IN: AtomicU16 = AtomicU16::new(0);
/// Bit N set → pipe N is a bulk pipe carrying an interrupt IN endpoint. Its blocks need not
/// end in a short packet (a 1012-byte endpoint at MXPS 1012), so SHTNAK can't stop the pipe
/// between reads: the completion sets PID to NAK itself, and the next read re-arms it.
static HCD_PIPE_INTR_ON_BULK_IN: AtomicU16 = AtomicU16::new(0);

// ---------------------------------------------------------------------------
// Receive DMA (one interrupt-on-bulk IN pipe per port, on D1FIFO)
// ---------------------------------------------------------------------------
//
// Host mode moves every other pipe's data by CPU through D0FIFO; D1FIFO is free, so one pipe
// (the first interrupt-on-bulk IN pipe opened once a channel is given: an Overbridge audio
// stream) is bound to it for the life of the pipe. The interrupt handler only records the
// packet length (DTLN) and holds the pipe at NAK; the reading task then has the DMAC move
// exactly that many bytes, in 4-byte cycle-steal transfers, into an uncached bounce buffer,
// and copies them out. DCLRM clears the FIFO buffer once the DMAC has read it.

/// DMARS for each port's D1FIFO (MID 0b10_0001 / 0b10_0011, RID 0b11).
const D1FIFO_DMARS: [u32; 2] = [0x87, 0x8F];
/// Longest wait for the DMAC to drain one block (a few µs at most) before giving up.
const RX_DMA_TIMEOUT_MS: u64 = 2;

/// Per port: the DMA channel that drains the D1FIFO pipe.
static RX_DMA: [critical_section::Mutex<core::cell::RefCell<Option<crate::dmac::Dma<'static>>>>;
    2] = [const { critical_section::Mutex::new(core::cell::RefCell::new(None)) }; 2];
/// Per port: the pipe bound to D1FIFO for DMA receive, or 0.
static RX_DMA_PIPE: [AtomicU8; 2] = [const { AtomicU8::new(0) }; 2];
/// No packet is waiting in D1FIFO.
const NO_DTLN: u16 = u16::MAX;
/// Per port: the length (DTLN) of the packet waiting in D1FIFO for the reader to drain, or
/// [`NO_DTLN`]. The interrupt handler records it; one that arrives with no read pending stays
/// in the FIFO until the next read drains it (the controller raises no second BRDY for it).
static RX_DMA_DTLN: [AtomicU16; 2] = [const { AtomicU16::new(NO_DTLN) }; 2];

/// Runs its closure when dropped unless [`defuse`](Self::defuse)d: undoes a step a cancelled
/// future left half done.
struct OnDrop<F: FnMut()>(Option<F>);

impl<F: FnMut()> OnDrop<F> {
    fn defuse(mut self) {
        self.0 = None;
    }
}

impl<F: FnMut()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.as_mut() {
            f();
        }
    }
}

/// Owns the receive channel while a drain runs, and hands it back however the drain ends.
/// Dropped mid-transfer (the reading future was cancelled), it halts the DMA and clears the
/// half-read block from the FIFO, so the channel and the FIFO are ready for the next read.
struct DrainGuard {
    port: u8,
    dma: Option<crate::dmac::Dma<'static>>,
    armed: bool,
}

impl Drop for DrainGuard {
    fn drop(&mut self) {
        let Some(mut dma) = self.dma.take() else {
            return;
        };
        if self.armed {
            dma.halt();
            usb(self.port).d1fifoctr().write_value(FIFOCTR_BCLR);
        }
        critical_section::with(|cs| RX_DMA[self.port as usize].borrow(cs).replace(Some(dma)));
    }
}

/// A 1 KB, cache-line-aligned block buffer.
#[repr(align(32))]
struct Bounce([u32; 256]);
/// Per port: where the DMAC writes a received block. Only ever touched through its uncached
/// alias ([`crate::uncached`]), so no cache maintenance is needed.
static mut RX_BOUNCE: [Bounce; 2] = [const { Bounce([0; 256]) }; 2];

/// The port of USB register block `u`.
fn port_of(u: Usb) -> usize {
    (u.syscfg0().addr() != usb(0).syscfg0().addr()) as usize
}

#[cfg(test)]
fn reset_rx_dma(port: usize) {
    critical_section::with(|cs| RX_DMA[port].borrow(cs).replace(None));
    RX_DMA_PIPE[port].store(0, Ordering::Release);
    RX_DMA_DTLN[port].store(NO_DTLN, Ordering::Release);
}

/// Packets an isochronous IN pipe holds for its reader (1 ms at high speed).
const ISO_IN_SLOTS: usize = 8;
/// Largest isochronous packet kept (the class drivers' packet buffers are this size).
const ISO_IN_SLOT_BYTES: usize = 1024;

/// Received packets of one isochronous IN pipe, oldest at `head`.
struct IsoInRing {
    slots: [[u8; ISO_IN_SLOT_BYTES]; ISO_IN_SLOTS],
    lens: [u16; ISO_IN_SLOTS],
    head: usize,
    count: usize,
}

impl IsoInRing {
    const EMPTY: Self = Self {
        slots: [[0; ISO_IN_SLOT_BYTES]; ISO_IN_SLOTS],
        lens: [0; ISO_IN_SLOTS],
        head: 0,
        count: 0,
    };
}

/// The rings of pipes 1 and 2, the only pipes that do isochronous transfers.
static ISO_IN: [critical_section::Mutex<core::cell::UnsafeCell<IsoInRing>>; 2] = {
    use core::cell::UnsafeCell;
    use critical_section::Mutex;
    #[allow(clippy::declare_interior_mutable_const)]
    const EMPTY: Mutex<UnsafeCell<IsoInRing>> = Mutex::new(UnsafeCell::new(IsoInRing::EMPTY));
    [EMPTY; 2]
};

/// The ring of isochronous pipe `pipe` (1 or 2), inside a critical section.
fn iso_in_ring<R>(pipe: usize, f: impl FnOnce(&mut IsoInRing) -> R) -> R {
    critical_section::with(|cs| f(unsafe { &mut *ISO_IN[pipe - 1].borrow(cs).get() }))
}

static HCD_PIPE_WAKERS: [AtomicWaker; HCD_PIPE_COUNT] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const W: AtomicWaker = AtomicWaker::new();
    [W; HCD_PIPE_COUNT]
};

// ---------------------------------------------------------------------------
// Bus-level event state (ATTCH / DTCH / SACK / SIGN)
// ---------------------------------------------------------------------------

static HCD_EVENTS: AtomicU8 = AtomicU8::new(0);
static HCD_EVENT_WAKER: AtomicWaker = AtomicWaker::new();

/// Per port: the host has started and not yet checked for a device that was already on the bus.
/// Such a device makes no attach edge, so the first event wait probes for it.
static PROBE_PENDING: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];

// ---------------------------------------------------------------------------
// Pipe-allocation + EP→pipe mapping, per port
// ---------------------------------------------------------------------------

/// How an endpoint is carried: the pipes it may use, the MXPS to programme and the transfer
/// type the pipe is configured as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PipeLayout {
    /// Lowest and highest pipe the endpoint may use; searched highest first.
    first: usize,
    last: usize,
    /// PIPEMAXP.MXPS to programme.
    mxps: u16,
    /// The transfer type the pipe is configured as (PIPECFG.TYPE).
    carry: EndpointType,
}

/// The pipe layout for an endpoint of `ep_type` with descriptor `w_max_packet`.
///
/// Interrupt pipes (6-9) stop at 64 bytes, so a larger interrupt endpoint (Overbridge's
/// 368-byte blocks) rides a bulk pipe. On the wire an interrupt and a bulk transaction use the
/// same tokens, handshakes and toggles, and a bulk pipe is polled every microframe the bus has
/// room for, at least as often as `bInterval` 1. Pipes 3-5 take only MXPS 8/16/32/64/512
/// (TRM §28.3.34), so up to 512 bytes the pipe is programmed at 512 and every packet is short;
/// above that only pipes 1-2 take the size. A high-bandwidth endpoint (bits 12:11) can't ride
/// a bulk pipe and is rejected.
fn pipe_layout(ep_type: EndpointType, w_max_packet: u16) -> Result<PipeLayout, HostError> {
    let mps = w_max_packet & PIPEMAXP_MXPS_MASK;
    let layout = |first, last, mxps, carry| {
        Ok(PipeLayout {
            first,
            last,
            mxps,
            carry,
        })
    };
    match ep_type {
        EndpointType::Control => Err(HostError::OutOfPipes), // control uses pipe 0 directly
        EndpointType::Isochronous => layout(1, 2, mps, EndpointType::Isochronous),
        EndpointType::Bulk => layout(1, 5, mps, EndpointType::Bulk),
        EndpointType::Interrupt if mps <= 64 => layout(6, 9, mps, EndpointType::Interrupt),
        EndpointType::Interrupt if w_max_packet >> 11 != 0 || mps > 1024 => Err(HostError::Other(
            "interrupt endpoint is high-bandwidth or over 1024 bytes",
        )),
        EndpointType::Interrupt if mps <= 512 => layout(1, 5, 512, EndpointType::Bulk),
        EndpointType::Interrupt => layout(1, 2, mps, EndpointType::Bulk),
    }
}

/// PIPEPERI.IITV for an interrupt pipe. The controller issues the pipe's token once every
/// 2^IITV frames, counted in microframes for a high-speed device and in 1 ms frames for a
/// full- or low-speed one, including one behind a high-speed hub (TRM §28.4.8 (1)(c)). At high
/// speed bInterval is an exponent (period 2^(bInterval-1) microframes); otherwise it is a
/// period in frames, rounded down here to a power of two. 0 (embassy's hub class) counts as 1.
fn interrupt_iitv(binterval: u8, high_speed: bool) -> u16 {
    let b = binterval.max(1);
    let n = if high_speed {
        b - 1
    } else {
        (u8::BITS - 1 - b.leading_zeros()) as u8
    };
    n.min(7) as u16
}

struct HcdAlloc {
    /// True between DeviceAttached and the next bus_reset().
    need_reset: bool,
    /// Bitmap of in-use pipes: bit N set → pipe N allocated.
    pipes_used: u16,
    /// ep_to_pipe[dev_addr-1][dir_in as usize][ep_num - 1] → pipe number.
    /// 0 = not open.  dev_addr must be 1-5.
    ep_to_pipe: [[[u8; 15]; 2]; 5],
    /// EP0 max-packet-size per device address.
    ctl_mps: [u16; HCD_MAX_DEV],
    /// Packet-buffer block allocator (128 × 64-byte blocks, blocks 0-6 reserved
    /// for DCP + the hardware-fixed PIPE6-8 areas).
    buf: BufAllocator,
    /// Per-pipe packet-buffer reservation, for freeing on close.
    /// `buf_start[n]` = first 64-byte block, `buf_reserved[n]` = block count.
    buf_start: [u8; HCD_PIPE_COUNT],
    buf_reserved: [u8; HCD_PIPE_COUNT],
}

impl HcdAlloc {
    const fn new() -> Self {
        Self {
            need_reset: false,
            pipes_used: 0x0001, // pipe 0 is permanently allocated (DCP)
            ep_to_pipe: [[[0u8; 15]; 2]; 5],
            ctl_mps: [64u16; HCD_MAX_DEV],
            buf: BufAllocator::new(),
            buf_start: [0u8; HCD_PIPE_COUNT],
            buf_reserved: [0u8; HCD_PIPE_COUNT],
        }
    }

    /// Allocate a free pipe in `first..=last`, highest first, as TinyUSB's `find_pipe` does.
    fn alloc_pipe(&mut self, first: usize, last: usize) -> Option<usize> {
        for p in (first..=last).rev() {
            if self.pipes_used & (1 << p) == 0 {
                self.pipes_used |= 1 << p;
                return Some(p);
            }
        }
        None
    }

    fn free_pipe(&mut self, pipe: usize) {
        if pipe != 0 {
            self.pipes_used &= !(1u16 << pipe);
            // Release the pipe's packet-buffer blocks back to the pool.
            if self.buf_reserved[pipe] > 0 {
                self.buf
                    .free(self.buf_start[pipe], self.buf_reserved[pipe] as usize);
                self.buf_start[pipe] = 0;
                self.buf_reserved[pipe] = 0;
            }
        }
    }

    /// Reserve packet-buffer blocks for `pipe` carrying `mps`-byte packets.
    ///
    /// Returns `(buf_start, blocks_per_bank)` for the PIPEBUF write, or `None`
    /// if the packet buffer is full.  `double_buf` reserves two banks (matching
    /// PIPECFG.DBLB).  Pipes 6-8 have hardware-fixed buffers (blocks 4/5/6) and
    /// must NOT be passed here — their PIPEBUF is ignored by the controller.
    fn alloc_pipe_buf(&mut self, pipe: usize, mps: u16, double_buf: bool) -> Option<(u8, u8)> {
        let blocks_per_bank = (mps as usize).div_ceil(PKT_BUF_BLOCK_SIZE).max(1);
        let reserve = blocks_per_bank * if double_buf { 2 } else { 1 };
        let start = self.buf.alloc(reserve)?;
        self.buf_start[pipe] = start;
        self.buf_reserved[pipe] = reserve as u8;
        Some((start, blocks_per_bank as u8))
    }
}

static HCD_ALLOC: [critical_section::Mutex<core::cell::UnsafeCell<HcdAlloc>>; 2] = {
    use core::cell::UnsafeCell;
    use critical_section::Mutex;
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Mutex<UnsafeCell<HcdAlloc>> = Mutex::new(UnsafeCell::new(HcdAlloc::new()));
    [INIT; 2]
};

// ---------------------------------------------------------------------------
// Rusb1HostDriver
// ---------------------------------------------------------------------------

/// USB host-mode driver for one RUSB1 port.
///
/// Implements [`embassy_usb_driver::host::UsbHostController`] so it can be used
/// directly with the `embassy-usb-host` enumeration stack.
///
/// The lower-level async methods (`wait_for_event`, `setup_send`, `xfer_in`, …)
/// are public for use without the Embassy host stack.
pub struct Rusb1HostDriver<'d> {
    port: u8,
    _usb: core::marker::PhantomData<&'d mut ()>,
}

impl<'d> Rusb1HostDriver<'d> {
    /// Initialise the hardware in host mode and return a driver instance.
    ///
    /// # Safety
    ///
    /// - The RUSB1 module clock must be running.
    /// - No device-mode driver may be active on this port simultaneously.
    /// - `hcd_int_handler(port)` must be wired to GIC IRQ 73+port before
    ///   calling any async method.
    pub(crate) unsafe fn new(port: u8) -> Self {
        unsafe {
            debug_assert!(port <= 1);
            let u = usb(port);

            // ── PLL / clock init (TRM §28.4.1 (5)) ─────────────────────────
            // Stop clock supply to this port first.
            u.suspmode().modify(|r| *r &= !SUSPMODE_SUSPM);

            // UPLLE lives in USB0's SYSCFG0 regardless of which port is being
            // initialised, and may only be modified while SUSPM=0 on BOTH
            // channels (TRM §28.3.1).  Only enable it when off: a set UPLLE means
            // the PLL is already locked from the other port's bring-up, so we
            // must not repeat the lock wait or disturb that port's SUSPM.  When
            // off, the PLL is down and neither channel can be running, so
            // clearing the other channel's SUSPM is safe.
            let u0 = usb(0);
            if u0.syscfg0().read() & SYSCFG_UPLLE == 0 {
                let other = usb(if port == 0 { 1 } else { 0 });
                other.suspmode().modify(|r| *r &= !SUSPMODE_SUSPM);
                u0.syscfg0().modify(|r| *r |= SYSCFG_UPLLE);

                // Wait >= 1 ms for PLL lock (~400 K iterations at 400 MHz).
                for _ in 0u32..400_000 {
                    core::hint::spin_loop();
                }
            }

            // CPU-bus wait cycles and clock re-enable.
            u.buswait().write_value(BUSWAIT_VALUE);
            u.suspmode().modify(|r| *r |= SUSPMODE_SUSPM);

            // ── Mode / PHY config ────────────────────────────────────────────
            // Host mode: DCFM=1, DRPD=1 (pull-downs enabled), DPRPU=0 (no pull-up).
            // HSE is NOT set here; TRM requires it to be set after ATTCH and before
            // USBRST — it is written in bus_reset() based on the detected device speed.
            //
            // TRM §28.3.1: DCFM may only be modified while DPRPU and DRPD are
            // both 0.  DRPD's reset value is 1, so the first write must clear it
            // (along with DPRPU/HSE) *while* setting DCFM — otherwise the cold-boot
            // DCFM 0→1 transition violates the precondition.  The second write
            // then raises DRPD (host pull-downs).
            // Start from a stopped controller. A debugger reload is not a chip reset: the
            // previous program may have left SOF running and a SETUP stuck in the sequencer
            // (on hardware every later SETUP then sat pending, never sent). Stop SOF and
            // any reset, then clear USBE, which re-initialises the SIE (TRM Tables 28.3, 28.4).
            u.dvstctr0().write_value(0);
            u.syscfg0().modify(|r| *r &= !SYSCFG_USBE);
            u.syscfg0().modify(|r| {
                *r = (*r & !(SYSCFG_DCFM | SYSCFG_DRPD | SYSCFG_DPRPU | SYSCFG_HSE)) | SYSCFG_DCFM
            });
            u.syscfg0().modify(|r| *r |= SYSCFG_DRPD);

            // NOTE: VBUS power is NOT module-controlled on the RZ/A1L —
            // DVSTCTR0 bits 11:9 are reserved (there is no VBUSEN bit, TRM
            // §28.3.4).  Host-port VBUS must be switched externally (GPIO).

            // TRM §28.3.1 USBE: in host mode, set USBE only "after setting DRPD
            // to 1, eliminating LNST bit chattering, and checking that the USB
            // bus has been settled."  Give the line state a brief settle window
            // after raising DRPD before enabling the SIE.
            for _ in 0u32..10_000 {
                core::hint::spin_loop();
            }

            // Enable the USB module.
            u.syscfg0().modify(|r| *r |= SYSCFG_USBE);

            // Control pipe: always use SHTNAK (NAK on short packet) in host mode.
            u.dcpcfg().write_value(DCPCFG_SHTNAK);
            u.dcpmaxp().write_value(64);

            // Global interrupt enables.
            u.intenb(0)
                .write_value(INTENB0_BRDYE | INTENB0_NRDYE | INTENB0_BEMPE);
            // Drop attach/detach/bus-change status a previous program left latched
            // (RC-W0: writing 0 clears every bit), so the first one the ISR sees is
            // real. Then start watching for device attach (ATTCH); DTCH is enabled
            // once a device is connected.
            u.intsts(1).write_value(0);
            PROBE_PENDING[port as usize].store(true, Ordering::Release);
            u.intenb(1)
                .write_value(INTENB1_SACKE | INTENB1_SIGNE | INTENB1_EOFERRE | INTENB1_ATTCHE);

            // Enable BEMP/NRDY/BRDY for pipe 0 (the DCP).
            u.bempenb().write_value(1);
            u.nrdyenb().write_value(1);
            u.brdyenb().write_value(1);

            // Register the GIC interrupt for this port.
            crate::usb::int_enable(port);

            Self {
                port,
                _usb: core::marker::PhantomData,
            }
        }
    }

    // --- Bus-level API ---

    /// Stop the host controller and leave the hardware in a neutral state,
    /// ready for the device-mode driver to take over.
    ///
    /// TRM §28.3 (3): the DCFM bit may only be changed while DPRPU = 0 *and*
    /// DRPD = 0.  Host mode always has DPRPU = 0, so we clear DRPD first,
    /// then clear DCFM to return to function (device) mode.
    pub async fn stop(&mut self) {
        unsafe {
            let u = usb(self.port);

            // Disable GIC interrupt so the ISR cannot fire mid-teardown.
            crate::usb::int_disable(self.port);

            // Disable all interrupt sources.
            u.intenb(0).write_value(0);
            u.intenb(1).write_value(0);
            u.brdyenb().write_value(0);
            u.nrdyenb().write_value(0);
            u.bempenb().write_value(0);

            // Stop SOF generation.  (VBUS is switched externally on the
            // RZ/A1L — DVSTCTR0 has no VBUSEN bit.)
            u.dvstctr0().modify(|r| *r &= !DVSTCTR0_UACT);
        }
        // Let the bus settle before changing resistor config.
        Timer::after_micros(1).await;
        let u = usb(self.port);
        // Clear DRPD to satisfy the TRM DCFM-change pre-condition
        // (DPRPU = 0 already in host mode; now DRPD = 0 too).
        u.syscfg0().modify(|r| *r &= !SYSCFG_DRPD);
        // Switch to function (device) mode.
        u.syscfg0().modify(|r| *r &= !SYSCFG_DCFM);
    }

    /// Wait for the next raw bus event: device attached or detached.
    ///
    /// For `Connected`, speed is not yet valid — call [`bus_reset`](Self::bus_reset)
    /// first, then read [`port_speed`](Self::port_speed).
    /// `UsbHostController::wait_for_device_event` does both automatically.
    pub async fn wait_for_event(&self) -> DeviceEvent {
        core::future::poll_fn(|cx| {
            HCD_EVENT_WAKER.register(cx.waker());
            // Consume the event atomically.
            let ev = HCD_EVENTS.fetch_and(!(EVT_ATTACH | EVT_DETACH), Ordering::Acquire);
            if ev & EVT_ATTACH != 0 {
                return core::task::Poll::Ready(DeviceEvent::Connected(Speed::Full));
            }
            if ev & EVT_DETACH != 0 {
                return core::task::Poll::Ready(DeviceEvent::Disconnected);
            }
            core::task::Poll::Pending
        })
        .await
    }

    /// Drive a USB bus reset (USBRST for ~20 ms), then re-enable SOF generation.
    ///
    /// Must be called after every [`DeviceEvent::Connected`] before
    /// enumeration.
    ///
    /// Returns the speed the reset handshake resolved to, or `None` if RHST never
    /// resolved (no device answered); SOF is then left off.
    pub async fn bus_reset(&self) -> Option<Speed> {
        // The reset's SE0 and the line's return to J are a detach and an attach to the
        // controller: mask that detection while the reset runs, and drop what it latched
        // after (left armed, the attach it raised made the supervisor reset a bound device).
        let detection = usb(self.port).intenb(1).read() & (INTENB1_ATTCHE | INTENB1_DTCHE);
        usb(self.port)
            .intenb(1)
            .modify(|r| *r &= !(INTENB1_ATTCHE | INTENB1_DTCHE));
        // Cancelled mid-reset (its caller raced it against something else), end the reset and
        // re-arm detection, or the port never reports an attach or a detach again.
        let port = self.port;
        let unwind = OnDrop(Some(move || {
            let u = usb(port);
            u.dvstctr0().modify(|r| *r &= !DVSTCTR0_USBRST);
            u.intsts(1).write_value(!(INTSTS1_ATTCH | INTSTS1_DTCH));
            u.intenb(1).modify(|r| *r |= detection);
        }));
        unsafe {
            let u = usb(self.port);

            // Cancel any in-flight DCP transaction.
            pipectr(u, 0).write_value(PIPECTR_PID_NAK);
            while pipectr(u, 0).read() & PIPECTR_PBUSY != 0 {}

            // Stop SOF and cancel any pending SETUP.
            crate::usb::int_disable(self.port);
            u.dvstctr0().modify(|r| *r &= !DVSTCTR0_UACT);
            let dcpctr = pipectr(u, 0).read();
            if dcpctr & DCPCTR_SUREQ != 0 {
                pipectr(u, 0).write_value((dcpctr & !DCPCTR_SUREQ) | DCPCTR_SUREQCLR)
            }
            crate::usb::int_enable(self.port);

            // Set HSE before asserting USBRST (TRM §28.3 SYSCFG.HSE: "after
            // detecting the ATTCH interrupt and before setting USBRST to 1").
            // LS devices cannot do HS negotiation — HSE must be 0 for them.
            //
            // LS detection: RHST only reports 001 (LS) after a *previous* bus
            // reset, so on the first reset after attach it still reads 000.
            // In that case use the idle line state instead (TRM Table 28.5
            // with DRPD=1): an attached FS/HS device pulls D+ → LNST = 01
            // (full-speed J); an LS device pulls D- → LNST = 10.
            let rhst_now = u.dvstctr0().read() & DVSTCTR0_RHST;
            // Debounce LNST: the TRM requires reading the line state repeatedly
            // until the same value is seen several times running (SYSSTS0
            // chattering).  A single mid-chatter read could flip the HSE choice,
            // running an LS device through the HS handshake (or vice-versa).
            let mut lnst = u.syssts0().read() & SYSSTS0_LNST;
            let mut stable = 0u8;
            for _ in 0..64 {
                let s = u.syssts0().read() & SYSSTS0_LNST;
                if s == lnst {
                    stable += 1;
                    if stable >= 3 {
                        break;
                    }
                } else {
                    lnst = s;
                    stable = 0;
                }
            }
            let is_ls = rhst_now == DVSTCTR0_RHST_LS || (rhst_now == 0 && lnst == 0b10);
            if is_ls {
                u.syscfg0().modify(|r| *r &= !SYSCFG_HSE);
            } else {
                u.syscfg0().modify(|r| *r |= SYSCFG_HSE);
            }
            u.dvstctr0().modify(|r| *r |= DVSTCTR0_USBRST);
        }

        Timer::after_millis(20).await;

        let p = self.port as usize;
        let u = usb(self.port);
        // End the reset and re-enable SOF generation in a single write:
        // TRM §28.3.4 USBRST: "Write 1 to the UACT bit simultaneously with
        // the end of the USB bus reset process (writing 0 to USBRST)."
        let cur = u.dvstctr0().read();
        u.dvstctr0()
            .write_value((cur & !DVSTCTR0_USBRST) | DVSTCTR0_UACT);

        // Wait for the reset handshake to complete before the caller reads the
        // speed.  RHST only latches a determined value (001 LS / 010 FS / 011
        // HS) once SE0 driving finishes; for HS the chirp handshake can take a
        // few ms, during which RHST reads 0b1xx, which port_speed() would
        // misreport as Full.  Poll for up to ~50 ms.
        for _ in 0..50 {
            let rhst = usb(self.port).dvstctr0().read() & DVSTCTR0_RHST;
            if matches!(rhst, DVSTCTR0_RHST_LS | DVSTCTR0_RHST_FS | DVSTCTR0_RHST_HS) {
                break;
            }
            Timer::after_millis(1).await;
        }

        unsafe {
            critical_section::with(|cs| {
                (&mut *HCD_ALLOC[p].borrow(cs).get()).need_reset = false;
            });
        }
        u.intsts(1).write_value(!(INTSTS1_ATTCH | INTSTS1_DTCH));
        u.intenb(1).modify(|r| *r |= detection);
        unwind.defuse();

        let rhst = u.dvstctr0().read() & DVSTCTR0_RHST;
        if matches!(rhst, DVSTCTR0_RHST_LS | DVSTCTR0_RHST_FS | DVSTCTR0_RHST_HS) {
            Some(self.port_speed())
        } else {
            u.dvstctr0().modify(|r| *r &= !DVSTCTR0_UACT);
            None
        }
    }

    /// `Err(Canceled)` when the port is stopped (UACT=0: no device reset yet, or a detach or
    /// EOF error stopped it). A transfer started then would never be sent and never complete.
    fn port_active(&self) -> Result<(), PipeError> {
        if usb(self.port).dvstctr0().read() & DVSTCTR0_UACT == 0 {
            Err(PipeError::Canceled)
        } else {
            Ok(())
        }
    }

    /// Watch for detach instead of attach: a device is on the port.
    fn watch_for_detach(&self) {
        let u = usb(self.port);
        u.intenb(1)
            .modify(|r| *r = (*r & !INTENB1_ATTCHE) | INTENB1_DTCHE);
    }

    /// Watch for attach instead of detach: the port is empty.
    fn watch_for_attach(&self) {
        let u = usb(self.port);
        u.intenb(1)
            .modify(|r| *r = (*r & !INTENB1_DTCHE) | INTENB1_ATTCHE);
    }

    /// Look for a device already on the bus at host start. A device left by the
    /// previous program, or plugged in before boot, makes no attach edge: a
    /// full- or low-speed one holds J or K on the line, and an idle high-speed
    /// one holds no pull-up (SE0) but answers a reset with the high-speed
    /// handshake. So reset the bus and see whether RHST resolves a speed.
    async fn probe_present(&self) -> Option<Speed> {
        let lnst = usb(self.port).syssts0().read() & SYSSTS0_LNST;
        let speed = self.bus_reset().await;
        log::info!(
            "usb{}: probe: line {:02b}, bus reset → {:?}",
            self.port,
            lnst,
            speed
        );
        if speed.is_some() {
            self.watch_for_detach();
            // An attach the device raised as the line came up is this same device.
            HCD_EVENTS.fetch_and(!EVT_ATTACH, Ordering::AcqRel);
        }
        speed
    }

    /// Return the speed of the connected device.
    ///
    /// Only valid after [`bus_reset`](Self::bus_reset) completes.
    pub fn port_speed(&self) -> Speed {
        const RHST_LS: u16 = 0x0001;
        const RHST_FS: u16 = 0x0002;
        const RHST_HS: u16 = 0x0003;
        let u = usb(self.port);
        match u.dvstctr0().read() & 0x0007 {
            RHST_LS => Speed::Low,
            RHST_FS => Speed::Full,
            RHST_HS => Speed::High,
            _ => Speed::Full, // safe fallback
        }
    }

    // --- Device / endpoint management ---

    /// Configure DCPMAXP and DEVADD for a device's control endpoint.
    ///
    /// `hub_addr` and `hub_port` describe where in the topology the device
    /// lives:
    /// - Pass `hub_addr = 0, hub_port = 0` when the device is **directly**
    ///   attached to this USB port.
    /// - Pass the hub's USB address and the hub port number (1-based) when
    ///   the device is connected through a hub.  The hardware will
    ///   automatically generate SSPLIT/CSPLIT split transactions.
    ///
    /// Must be called (and DEVADDn fully configured) **before** starting
    /// any pipe that targets this device (TRM Note 1 for DEVADDn).
    ///
    /// Returns [`HostError::OutOfSlots`] if `dev_addr` is above 5.
    /// Address 0 is permitted: enumeration programmes
    /// the DCP at the default address before `SET_ADDRESS`.
    pub fn device_open(
        &self,
        dev_addr: u8,
        mps: u16,
        speed: Speed,
        hub_addr: u8,
        hub_port: u8,
    ) -> Result<(), HostError> {
        if dev_addr != 0 && !dev_addr_supported(dev_addr) {
            return Err(HostError::OutOfSlots);
        }
        // hub_addr 0 means direct; 1-10 are valid hub USB addresses.
        debug_assert!(hub_addr <= 10);
        // hub_port 0 means direct; 1-7 are valid hub port numbers.
        debug_assert!(hub_port <= 7);
        unsafe {
            let u = usb(self.port);
            let p = self.port as usize;

            // Ensure the DCP is idle before modifying DEVADDn (TRM Note 2).
            pipectr(u, 0).write_value(PIPECTR_PID_NAK);
            while pipectr(u, 0).read() & PIPECTR_PBUSY != 0 {}

            // Program DEVADDn FIRST: UPPHUB | HUBPORT | USBSPD.  TRM §28.3
            // DCPMAXP.DEVSEL: "these bits should be set after setting the DEVADDn
            // register corresponding to the value to be set."  Writing DEVSEL
            // before DEVADDn would latch stale speed/hub fields for this address.
            let usbspd = match speed {
                Speed::Low => DEVADD_USBSPD_LS,
                Speed::High => DEVADD_USBSPD_HS,
                Speed::Full => DEVADD_USBSPD_FS,
            };
            let devadd_val = ((hub_addr as u16) << DEVADD_UPPHUB_SHIFT)
                | ((hub_port as u16) << DEVADD_HUBPORT_SHIFT)
                | usbspd;
            u.devadd(dev_addr as usize).write_value(devadd_val);

            // Then embed the device address (DEVSEL) in DCPMAXP bits [15:12].
            u.dcpmaxp().write_value(
                ((dev_addr as u16) << DCPMAXP_DEVSEL_SHIFT) | (mps & DCPMAXP_MXPS_MASK),
            );

            critical_section::with(|cs| {
                (&mut *HCD_ALLOC[p].borrow(cs).get()).ctl_mps[dev_addr as usize] = mps;
            });
        }
        Ok(())
    }

    /// Free all pipes that were allocated for `dev_addr`.
    ///
    /// Call after a device disconnects to reclaim hardware resources.
    pub fn device_close(&self, dev_addr: u8) {
        debug_assert!(dev_addr > 0 && (dev_addr as usize) < HCD_MAX_DEV);
        let p = self.port as usize;
        unsafe {
            let u = usb(self.port);
            critical_section::with(|cs| {
                let alloc = &mut *HCD_ALLOC[p].borrow(cs).get();
                alloc.ctl_mps[dev_addr as usize] = 64;

                for dir in 0..2usize {
                    for epn in 0..15usize {
                        let pipe = alloc.ep_to_pipe[dev_addr as usize - 1][dir][epn] as usize;
                        if pipe == 0 {
                            continue;
                        }

                        // Disable the pipe.
                        pipectr(u, pipe).write_value(0);
                        u.nrdyenb().modify(|r| *r &= !(1 << pipe));
                        u.brdyenb().modify(|r| *r &= !(1 << pipe));
                        u.pipesel().write_value(pipe as u16);
                        u.pipecfg().write_value(0);
                        u.pipemaxp().write_value(0);
                        u.pipesel().write_value(0);

                        // Clear the HCD_XFER entry.
                        let xfer = &mut *HCD_XFER[pipe].borrow(cs).get();
                        xfer.buf = core::ptr::null_mut();
                        xfer.dev_addr = 0;
                        xfer.ep_addr = 0;

                        alloc.free_pipe(pipe);
                        alloc.ep_to_pipe[dev_addr as usize - 1][dir][epn] = 0;
                    }
                }
            });
        }
    }

    // --- Control transfers ---

    /// Send a SETUP packet to `dev_addr`.
    ///
    /// Awaits SACK (returns `Ok(())`) or SIGN (returns
    /// `Err(PipeError::Canceled)`).  `setup` is the 8-byte packet in
    /// USB byte order (bmRequestType, bRequest, wValue, wIndex, wLength).
    pub async fn setup_send(&self, dev_addr: u8, setup: &[u8; 8]) -> Result<(), PipeError> {
        debug_assert!((dev_addr as usize) < HCD_MAX_DEV);
        self.port_active()?;
        unsafe {
            let u = usb(self.port);
            let p = self.port as usize;

            let ctl_mps = critical_section::with(|cs| {
                (&*HCD_ALLOC[p].borrow(cs).get()).ctl_mps[dev_addr as usize]
            });

            // Ensure the DCP is idle.
            pipectr(u, 0).write_value(PIPECTR_PID_NAK);
            while pipectr(u, 0).read() & PIPECTR_PBUSY != 0 {}

            // Update DCPMAXP with target device address.
            u.dcpmaxp().write_value(
                ((dev_addr as u16) << DCPMAXP_DEVSEL_SHIFT) | (ctl_mps & DCPMAXP_MXPS_MASK),
            );

            // Set DCPCFG.DIR: DIR=0 for IN data stage, DIR=1 for OUT data stage.
            // bmRequestType bit 7: 1 = device-to-host (IN).
            // SHTNAK (auto-NAK at end of a receiving transfer) must be 0 in
            // the transmitting direction (TRM §28.3.28), so set it only when
            // the DCP receives (IN data stage).
            let dir_in = setup[0] & 0x80 != 0;
            let dcpcfg = if dir_in { DCPCFG_SHTNAK } else { DCPCFG_DIR };
            u.dcpcfg().write_value(dcpcfg);

            // Write the 8 setup bytes to hardware registers.
            u.usbreq()
                .write_value(u16::from_le_bytes([setup[0], setup[1]]));
            u.usbval()
                .write_value(u16::from_le_bytes([setup[2], setup[3]]));
            u.usbindx()
                .write_value(u16::from_le_bytes([setup[4], setup[5]]));
            u.usbleng()
                .write_value(u16::from_le_bytes([setup[6], setup[7]]));

            // Clear pipe-0 completion flags.
            HCD_PIPE_DONE.fetch_and(!1, Ordering::Release);
            HCD_PIPE_NRDY.fetch_and(!1, Ordering::Release);
            HCD_PIPE_STALL.fetch_and(!1, Ordering::Release);
            // Clear SACK/SIGN event bits.
            HCD_EVENTS.fetch_and(!(EVT_SACK | EVT_SIGN), Ordering::Release);

            // Issue the SETUP token.
            let ctr = pipectr(u, 0).read();
            pipectr(u, 0).write_value(ctr | DCPCTR_SUREQ);
        }

        // Await SACK or SIGN from the ISR.
        core::future::poll_fn(|cx| {
            HCD_EVENT_WAKER.register(cx.waker());
            let ev = HCD_EVENTS.fetch_and(!(EVT_SACK | EVT_SIGN), Ordering::Acquire);
            if ev & EVT_SACK != 0 {
                return core::task::Poll::Ready(Ok(()));
            }
            if ev & EVT_SIGN != 0 {
                return core::task::Poll::Ready(Err(PipeError::Canceled));
            }
            core::task::Poll::Pending
        })
        .await
    }

    /// Control IN data stage: receive up to `buf.len()` bytes from EP0.
    ///
    /// Returns the number of bytes actually received (may be less than
    /// `buf.len()` if the device sends a short packet).
    pub async fn control_data_in(&self, dev_addr: u8, buf: &mut [u8]) -> Result<usize, PipeError> {
        unsafe {
            let u = usb(self.port);

            // Select CFIFO for reading (ISEL=0: host reads from device).
            // Wait for ISEL to deassert before accessing CFIFO.
            u.cfifosel().write_value(0x0000); // pipe0, ISEL=0
            for _ in 0..1000u32 {
                if u.cfifosel().read() & FIFOSEL_ISEL == 0 {
                    break;
                }
            }

            // Set transfer state.
            critical_section::with(|cs| {
                let xfer = &mut *HCD_XFER[0].borrow(cs).get();
                xfer.buf = buf.as_mut_ptr();
                xfer.length = buf.len() as u16;
                xfer.remaining = buf.len() as u16;
                xfer.ep_addr = 0x80; // EP0 IN
                xfer.dev_addr = dev_addr;
            });

            HCD_PIPE_DONE.fetch_and(!1, Ordering::Release);
            HCD_PIPE_NRDY.fetch_and(!1, Ordering::Release);
            HCD_PIPE_STALL.fetch_and(!1, Ordering::Release);

            // Arm DCP: hardware will issue IN tokens and fire BRDY on receipt.
            pipectr(u, 0).write_value(PIPECTR_PID_BUF);
        }

        poll_pipe(0).await
    }

    /// Control OUT data stage: send `buf` to EP0.
    ///
    /// Pass `&[]` to issue a zero-length packet (status stage ZLP).
    pub async fn control_data_out(&self, dev_addr: u8, buf: &[u8]) -> Result<(), PipeError> {
        unsafe {
            let u = usb(self.port);
            let p = self.port as usize;

            let ctl_mps = critical_section::with(|cs| {
                (&*HCD_ALLOC[p].borrow(cs).get()).ctl_mps[dev_addr as usize]
            }) as usize;

            // Select CFIFO for writing (ISEL=1: host writes to device).
            u.cfifosel()
                .write_value(FIFOSEL_ISEL | (MBW_32 << FIFOSEL_MBW_SHIFT));
            for _ in 0..1000u32 {
                if u.cfifosel().read() & FIFOSEL_ISEL != 0 {
                    break;
                }
            }

            // Initialise transfer state.
            critical_section::with(|cs| {
                let xfer = &mut *HCD_XFER[0].borrow(cs).get();
                xfer.buf = buf.as_ptr() as *mut u8;
                xfer.length = buf.len() as u16;
                xfer.remaining = buf.len() as u16;
                xfer.ep_addr = 0x00; // EP0 OUT
                xfer.dev_addr = dev_addr;
            });

            HCD_PIPE_DONE.fetch_and(!1, Ordering::Release);
            HCD_PIPE_NRDY.fetch_and(!1, Ordering::Release);
            HCD_PIPE_STALL.fetch_and(!1, Ordering::Release);

            if buf.is_empty() {
                // ZLP (TRM §28.4.5): clear the buffer with BCLR, then write
                // BVAL to end the (zero-byte) write and commit the packet.
                u.cfifoctr().write_value(FIFOCTR_BCLR);
                u.cfifoctr().write_value(FIFOCTR_BVAL);
            } else {
                // Wait (bounded) for BSTS=1 (FIFO writable) before the first
                // packet.  If the FIFO never became ready we must not arm the
                // pipe with nothing staged — with PID=BUF and an empty buffer
                // the SIE never issues OUT tokens, no BEMP arrives, and the
                // transfer would hang.
                let mut ready = false;
                for _ in 0..1000u32 {
                    if pipectr(u, 0).read() & PIPECTR_BSTS != 0 {
                        ready = true;
                        break;
                    }
                }
                if !ready {
                    return Err(PipeError::Canceled);
                }
                let len = buf.len().min(ctl_mps);
                let fifo = FifoPort::cfifo(u);
                sw_to_hw_fifo(&fifo, buf.as_ptr(), len);
                if len < ctl_mps {
                    u.cfifoctr().write_value(FIFOCTR_BVAL);
                }
                critical_section::with(|cs| {
                    let xfer = &mut *HCD_XFER[0].borrow(cs).get();
                    xfer.buf = xfer.buf.add(len);
                    xfer.remaining = xfer.remaining.saturating_sub(len as u16);
                });
            }

            // Arm DCP.
            pipectr(u, 0).write_value(PIPECTR_PID_BUF);
        }

        // Completion is signalled by pipe0_out_bemp() in the ISR.
        poll_pipe(0).await.map(|_| ())
    }

    // --- Non-control endpoints ---
    /// Give the driver a DMAC channel to drain one receive pipe: the first interrupt-on-bulk
    /// IN pipe opened afterwards (an Overbridge audio stream) moves its data by DMA through
    /// D1FIFO instead of byte-wise CPU reads. Without a channel, every pipe uses the CPU.
    ///
    /// The channel's completion interrupt must be bound
    /// ([`crate::dmac::Dma::new`] with its [`crate::dmac::InterruptHandler`]).
    pub fn enable_rx_dma(&self, mut dma: crate::dmac::Dma<'static>) {
        let p = self.port as usize;
        let chcfg = crate::dmac::CHCFG_SAD
            | crate::dmac::CHCFG_SDS_32BIT
            | crate::dmac::CHCFG_DDS_32BIT
            | crate::dmac::CHCFG_AM_BURST
            | crate::dmac::CHCFG_LVL
            | crate::dmac::CHCFG_HIEN
            | (dma.number() as u32 & 7);
        let src = usb(self.port).d1fifo().addr() as u32;
        // SAFETY: the source is this port's D1FIFO data register; the channel is owned.
        unsafe { dma.init_register_mode_rx(chcfg, src, D1FIFO_DMARS[p]) };
        critical_section::with(|cs| RX_DMA[p].borrow(cs).replace(Some(dma)));
    }

    /// Close the pipe for `ep_addr` on device `dev_addr` (what dropping its `Rusb1Pipe` does).
    #[cfg(test)]
    pub(crate) fn edpt_close(&self, dev_addr: u8, ep_addr: u8) {
        if let Some(pipe) = self.ep_to_pipe(dev_addr, ep_addr) {
            release_pipe(self.port, pipe, dev_addr, ep_addr);
        }
    }

    /// Move a received block of `n` bytes (the DTLN the interrupt handler recorded) from
    /// D1FIFO to `buf` by DMA. A length the 4-byte DMA can't move, or past the bounce buffer,
    /// is cleared from the FIFO and reported as 0 bytes, which the class counts as a framing
    /// error.
    /// A read on the receive-DMA pipe: drain a packet already waiting in D1FIFO, or arm the
    /// pipe and wait for the handler to record one, then drain it.
    async fn dma_xfer_in(
        &self,
        pipe: usize,
        dev_addr: u8,
        ep_addr: u8,
        buf: &mut [u8],
    ) -> Result<usize, PipeError> {
        let p = self.port as usize;
        let mask = 1u16 << pipe;
        // Check for a waiting packet and publish the read in one critical section, so the
        // handler can't record one in between and leave this read waiting for a BRDY that
        // never comes.
        let waiting = critical_section::with(|cs| unsafe {
            let dtln = RX_DMA_DTLN[p].swap(NO_DTLN, Ordering::AcqRel);
            if dtln == NO_DTLN {
                HCD_PIPE_DONE.fetch_and(!mask, Ordering::Release);
                HCD_PIPE_NRDY.fetch_and(!mask, Ordering::Release);
                HCD_PIPE_STALL.fetch_and(!mask, Ordering::Release);
                let xfer = &mut *HCD_XFER[pipe].borrow(cs).get();
                xfer.buf = buf.as_mut_ptr();
                xfer.length = buf.len() as u16;
                xfer.remaining = buf.len() as u16;
                xfer.ep_addr = ep_addr;
                xfer.dev_addr = dev_addr;
            }
            dtln
        });
        if waiting != NO_DTLN {
            return self.dma_drain(buf, waiting as usize).await;
        }
        let ctr = pipectr(usb(self.port), pipe);
        ctr.write_value((ctr.read() & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF);
        poll_pipe(pipe).await?;
        let dtln = RX_DMA_DTLN[p].swap(NO_DTLN, Ordering::AcqRel);
        if dtln == NO_DTLN {
            return Ok(0);
        }
        self.dma_drain(buf, dtln as usize).await
    }

    async fn dma_drain(&self, buf: &mut [u8], n: usize) -> Result<usize, PipeError> {
        let p = self.port as usize;
        let u = usb(self.port);
        if n == 0 {
            // A zero-length packet: DCLRM has cleared it (TRM Table 28.23).
            return Ok(0);
        }
        if n % 4 != 0 || n > 1024 {
            u.d1fifoctr().write_value(FIFOCTR_BCLR);
            return Ok(0);
        }
        let mut guard = DrainGuard {
            port: self.port,
            dma: critical_section::with(|cs| RX_DMA[p].borrow(cs).take()),
            armed: false,
        };
        let Some(dma) = guard.dma.as_mut() else {
            // No channel: drop the block rather than leave it to stall the pipe.
            u.d1fifoctr().write_value(FIFOCTR_BCLR);
            return Err(PipeError::Canceled);
        };
        // SAFETY: the bounce buffer is this port's, used only here, one block at a time.
        let bounce = crate::uncached(unsafe { core::ptr::addr_of_mut!(RX_BOUNCE[p]) });
        let src = u.d1fifo().addr() as u32;
        // SAFETY: `bounce` is 1 KB and `n` ≤ 1024; nothing else touches it until this returns.
        unsafe { dma.rearm_rx(src, bounce as u32, n as u32) };
        guard.armed = true;
        let done = embassy_futures::select::select(
            dma.wait_transfer_complete(),
            embassy_time::Timer::after_millis(RX_DMA_TIMEOUT_MS),
        )
        .await;
        match done {
            embassy_futures::select::Either::First(()) => {
                guard.armed = false;
                // The whole packet was read (DCLRM cleared it); a caller's shorter buffer gets
                // its first bytes.
                let len = n.min(buf.len());
                // SAFETY: the DMAC has finished writing `n` bytes; read them through the alias.
                unsafe {
                    core::ptr::copy_nonoverlapping(bounce as *const u8, buf.as_mut_ptr(), len)
                };
                Ok(len)
            }
            embassy_futures::select::Either::Second(()) => {
                log::warn!(
                    "usb{}: receive DMA did not finish; block dropped",
                    self.port
                );
                // The guard halts the channel and clears the FIFO.
                Err(PipeError::Timeout)
            }
        }
    }

    /// Allocate a hardware pipe and configure it for `ep_addr` on `dev_addr`.
    ///
    /// `ep_addr` format: bit 7 = direction (1=IN, 0=OUT), bits 3:0 = EP number.
    ///
    /// `binterval` is the endpoint descriptor's bInterval: an exponent for a high-speed
    /// interrupt endpoint, a period in frames otherwise; ignored for bulk and isochronous pipes.
    /// Pass 0 or 1 for the shortest period.
    pub fn edpt_open(
        &self,
        dev_addr: u8,
        ep_addr: u8,
        ep_type: EndpointType,
        mps: u16,
        binterval: u8,
    ) -> Result<(), HostError> {
        debug_assert!(dev_addr > 0 && (dev_addr as usize) < HCD_MAX_DEV);
        let p = self.port as usize;
        let epn = (ep_addr & 0x0F) as usize;
        let dir_in = ep_addr & 0x80 != 0;

        critical_section::with(|cs| {
            let alloc = unsafe { &mut *HCD_ALLOC[p].borrow(cs).get() };
            let layout = pipe_layout(ep_type, mps)?;
            let pipe = alloc
                .alloc_pipe(layout.first, layout.last)
                .ok_or(HostError::OutOfPipes)?;

            // Reserve packet-buffer blocks for programmable pipes.  Pipes 6-8
            // have hardware-fixed buffers (blocks 4/5/6, BUFNMB writes ignored),
            // so they are skipped; all other pipes (bulk/iso 1-5, interrupt 9)
            // MUST have a disjoint PIPEBUF region or their FIFOs overlap at
            // block 0 (the DCP area) and corrupt each other.  DBLB is set for
            // bulk and iso (see PIPECFG below), so those reserve two banks.
            let pipebuf: Option<(u8, u8)> = if (6..=8).contains(&pipe) {
                None
            } else {
                let double_buf =
                    matches!(layout.carry, EndpointType::Bulk | EndpointType::Isochronous);
                match alloc.alloc_pipe_buf(pipe, layout.mxps, double_buf) {
                    Some(v) => Some(v),
                    None => {
                        alloc.free_pipe(pipe);
                        return Err(HostError::OutOfPipes);
                    }
                }
            };

            // Record the EP→pipe mapping.
            // dev_addr is 1-based; array index is 0-based.
            alloc.ep_to_pipe[dev_addr as usize - 1][dir_in as usize][epn - 1] = pipe as u8;

            unsafe {
                let u = usb(self.port);
                let ctr = pipectr(u, pipe);

                // Clear FIFO and toggle sequencer.
                ctr.write_value(PIPECTR_ACLRM | PIPECTR_SQCLR);
                ctr.write_value(0);

                // Select pipe for configuration.
                u.pipesel().write_value(pipe as u16);

                // PIPEMAXP: MPS | (dev_addr << 12).
                u.pipemaxp().write_value(
                    ((dev_addr as u16) << PIPEMAXP_DEVSEL_SHIFT)
                        | (layout.mxps & PIPEMAXP_MXPS_MASK),
                );

                // PIPECFG.DIR: 1 = host sends (OUT), 0 = host receives (IN).
                let dir_field: u16 = if dir_in { 0x0000 } else { 0x0010 };
                let type_field: u16 = match layout.carry {
                    EndpointType::Bulk => PIPECFG_TYPE_BULK | PIPECFG_SHTNAK | PIPECFG_DBLB,
                    EndpointType::Interrupt => PIPECFG_TYPE_INTR,
                    EndpointType::Isochronous => PIPECFG_TYPE_ISO | PIPECFG_DBLB,
                    EndpointType::Control => unreachable!("control EP handled by DCP"),
                };
                u.pipecfg()
                    .write_value(type_field | dir_field | (epn as u16 & PIPECFG_EPNUM_MASK));

                // PIPEBUF: assign a disjoint packet-buffer region.  BUFNMB =
                // start block, BUFSIZE = blocks-per-bank − 1 (the DBLB bit
                // supplies the second bank).  Skipped for hardware-fixed pipes.
                if let Some((buf_start, blocks_per_bank)) = pipebuf {
                    let bufsize = (((blocks_per_bank as u16).saturating_sub(1))
                        << PIPEBUF_BUFSIZE_SHIFT)
                        & PIPEBUF_BUFSIZE_MASK;
                    u.pipebuf()
                        .write_value((buf_start as u16 & PIPEBUF_BUFNMB_MASK) | bufsize);
                }

                // PIPEPERI: IITV for interrupt pipes (6-9); bulk pipes (1-5, including
                // interrupt endpoints carried as bulk) must have IITV=0. The device's speed is
                // the USBSPD that device_open programmed into DEVADDn.
                let iitv: u16 = if layout.carry == EndpointType::Interrupt {
                    let high_speed =
                        u.devadd(dev_addr as usize).read() & DEVADD_USBSPD_HS == DEVADD_USBSPD_HS;
                    interrupt_iitv(binterval, high_speed)
                } else {
                    0
                };
                u.pipeperi().write_value(iitv & PIPEPERI_IITV_MASK);

                u.pipesel().write_value(0);

                // Clear stale interrupt flags then enable per-pipe interrupts.
                u.brdysts().write_value(!(1u16 << pipe));
                u.nrdysts().write_value(!(1u16 << pipe));
                u.brdyenb().modify(|r| *r |= 1 << pipe);
                // An isochronous pipe's NRDY is an underrun (OUT) or an overrun or missed
                // packet (IN), each microframe, never a failed transfer: leave it masked. A
                // detach still fails the pipe (the DTCH handler).
                if ep_type != EndpointType::Isochronous {
                    u.nrdyenb().modify(|r| *r |= 1 << pipe);
                }

                if ep_type == EndpointType::Interrupt
                    && layout.carry == EndpointType::Bulk
                    && dir_in
                {
                    HCD_PIPE_INTR_ON_BULK_IN.fetch_or(1 << pipe, Ordering::Release);
                    // The first such pipe, once a channel is given, is drained by DMA.
                    let has_dma = RX_DMA[p].borrow(cs).borrow().is_some();
                    if has_dma && RX_DMA_PIPE[p].load(Ordering::Acquire) == 0 {
                        u.d1fbcfg().write_value(0); // cycle steal, no TENDE
                        u.d1fifosel().write_value(
                            pipe as u16 | (MBW_32 << FIFOSEL_MBW_SHIFT) | FIFOSEL_DCLRM,
                        );
                        while u.d1fifosel().read() & FIFOSEL_CURPIPE_MASK != pipe as u16 {}
                        u.d1fifosel().modify(|r| *r |= FIFOSEL_DREQE);
                        RX_DMA_PIPE[p].store(pipe as u8, Ordering::Release);
                    }
                } else {
                    HCD_PIPE_INTR_ON_BULK_IN.fetch_and(!(1 << pipe), Ordering::Release);
                }
                if ep_type == EndpointType::Isochronous && !dir_in {
                    HCD_PIPE_ISO_OUT.fetch_or(1 << pipe, Ordering::Release);
                } else {
                    HCD_PIPE_ISO_OUT.fetch_and(!(1 << pipe), Ordering::Release);
                }
                if ep_type == EndpointType::Isochronous && dir_in {
                    let ring = &mut *ISO_IN[pipe - 1].borrow(cs).get();
                    ring.head = 0;
                    ring.count = 0;
                    HCD_PIPE_ISO_IN.fetch_or(1 << pipe, Ordering::Release);
                } else {
                    HCD_PIPE_ISO_IN.fetch_and(!(1 << pipe), Ordering::Release);
                }

                // OUT pipes are armed immediately (host will send on demand);
                // IN pipes wait until xfer_in() is called.
                if !dir_in {
                    ctr.write_value(PIPECTR_PID_BUF);
                }

                // Initialise the saved ep/dev info for the ISR.
                let xfer = &mut *HCD_XFER[pipe].borrow(cs).get();
                xfer.ep_addr = ep_addr;
                xfer.dev_addr = dev_addr;
                xfer.buf = core::ptr::null_mut();
            }

            Ok(())
        })
    }

    /// IN transfer: receive up to `buf.len()` bytes from `ep_addr` on `dev_addr`.
    pub async fn xfer_in(
        &self,
        dev_addr: u8,
        ep_addr: u8,
        buf: &mut [u8],
    ) -> Result<usize, PipeError> {
        let pipe = self
            .ep_to_pipe(dev_addr, ep_addr)
            .ok_or(PipeError::Canceled)?;
        self.port_active()?;
        if HCD_PIPE_ISO_IN.load(Ordering::Acquire) & (1 << pipe) != 0 {
            return self.iso_in_read(pipe, buf).await;
        }
        if RX_DMA_PIPE[self.port as usize].load(Ordering::Acquire) as usize == pipe {
            return self.dma_xfer_in(pipe, dev_addr, ep_addr, buf).await;
        }

        // Clear the pipe's flags, then publish the transfer, in one critical section: the
        // interrupt handler completes a transfer as soon as its buffer is published, and a
        // completion cleared afterwards was lost (on hardware the capture stream stalled
        // within seconds: the read never completed, every later packet was dropped).
        let mask = 1u16 << pipe;
        critical_section::with(|cs| unsafe {
            HCD_PIPE_DONE.fetch_and(!mask, Ordering::Release);
            HCD_PIPE_NRDY.fetch_and(!mask, Ordering::Release);
            HCD_PIPE_STALL.fetch_and(!mask, Ordering::Release);
            let xfer = &mut *HCD_XFER[pipe].borrow(cs).get();
            xfer.buf = buf.as_mut_ptr();
            xfer.length = buf.len() as u16;
            xfer.remaining = buf.len() as u16;
            xfer.ep_addr = ep_addr;
            xfer.dev_addr = dev_addr;
        });

        let u = usb(self.port);
        // Arm pipe to issue IN tokens.
        let ctr = pipectr(u, pipe);
        ctr.write_value((ctr.read() & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF);

        poll_pipe(pipe).await
    }

    /// Take the next packet of isochronous IN pipe `pipe`, waiting for one if none has
    /// arrived. The pipe is armed on the first read and stays armed: the interrupt handler
    /// keeps every packet until it is read.
    async fn iso_in_read(&self, pipe: usize, buf: &mut [u8]) -> Result<usize, PipeError> {
        let mask = 1u16 << pipe;
        HCD_PIPE_NRDY.fetch_and(!mask, Ordering::Release);
        let ctr = pipectr(usb(self.port), pipe);
        if ctr.read() & PIPECTR_PID_MASK != PIPECTR_PID_BUF {
            ctr.write_value((ctr.read() & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF);
        }
        core::future::poll_fn(|cx| {
            HCD_PIPE_WAKERS[pipe].register(cx.waker());
            let popped = iso_in_ring(pipe, |r| {
                (r.count > 0).then(|| {
                    let n = (r.lens[r.head] as usize).min(buf.len());
                    buf[..n].copy_from_slice(&r.slots[r.head][..n]);
                    r.head = (r.head + 1) % ISO_IN_SLOTS;
                    r.count -= 1;
                    n
                })
            });
            if let Some(n) = popped {
                return core::task::Poll::Ready(Ok(n));
            }
            if HCD_PIPE_NRDY.load(Ordering::Acquire) & mask != 0 {
                // The DTCH handler fails every pipe on a detach.
                HCD_PIPE_NRDY.fetch_and(!mask, Ordering::Release);
                return core::task::Poll::Ready(Err(PipeError::BadResponse));
            }
            if self.port_active().is_err() {
                return core::task::Poll::Ready(Err(PipeError::Canceled));
            }
            core::task::Poll::Pending
        })
        .await
    }

    /// OUT transfer: send `buf` to `ep_addr` on `dev_addr`.
    pub async fn xfer_out(&self, dev_addr: u8, ep_addr: u8, buf: &[u8]) -> Result<(), PipeError> {
        let pipe = self
            .ep_to_pipe(dev_addr, ep_addr)
            .ok_or(PipeError::Canceled)?;
        self.port_active()?;

        // Flags cleared before the transfer is published, as in xfer_in.
        let mask = 1u16 << pipe;
        critical_section::with(|cs| unsafe {
            HCD_PIPE_DONE.fetch_and(!mask, Ordering::Release);
            HCD_PIPE_NRDY.fetch_and(!mask, Ordering::Release);
            HCD_PIPE_STALL.fetch_and(!mask, Ordering::Release);
            let xfer = &mut *HCD_XFER[pipe].borrow(cs).get();
            xfer.buf = buf.as_ptr() as *mut u8;
            xfer.length = buf.len() as u16;
            xfer.remaining = buf.len() as u16;
            xfer.ep_addr = ep_addr;
            xfer.dev_addr = dev_addr;
        });

        // D0FIFO is shared with the interrupt handler, which selects it for other pipes
        // (pipe_brdy_in, pipe_brdy_out). An interrupt between selecting the port and
        // deselecting it re-pointed it mid-write: on hardware the playback write landed in
        // the capture pipe and the stream stopped. Hold interrupts off across the port use.
        let iso = HCD_PIPE_ISO_OUT.load(Ordering::Acquire) & mask != 0;
        let queued = critical_section::with(|_| unsafe {
            let u = usb(self.port);

            if buf.is_empty() {
                // ZLP: select D0FIFO, wait FRDY, then BCLR + BVAL (TRM
                // §28.4.5: "to send a zero-length packet, the BCLR bit must be
                // used to clear the buffer and then the BVAL bit is set").
                u.d0fifosel()
                    .write_value(pipe as u16 | (MBW_32 << FIFOSEL_MBW_SHIFT));
                while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != pipe as u16 {}
                wait_frdy(u);
                u.d0fifoctr().write_value(FIFOCTR_BCLR);
                u.d0fifoctr().write_value(FIFOCTR_BVAL);
                u.d0fifosel().write_value(0);
                while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != 0 {}
            } else {
                // Write the first packet to D0FIFO (32-bit access).
                u.d0fifosel()
                    .write_value(pipe as u16 | (MBW_32 << FIFOSEL_MBW_SHIFT));
                while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != pipe as u16 {}
                if iso {
                    // Both banks still full: drop this packet rather than wait for one
                    // with interrupts held off (a burst of packets starved the system).
                    if !frdy_now(u) {
                        u.d0fifosel().write_value(0);
                        while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != 0 {}
                        return false;
                    }
                } else {
                    wait_frdy(u);
                }

                // Read MPS from PIPEMAXP.
                u.pipesel().write_value(pipe as u16);
                let mps = (u.pipemaxp().read() & PIPEMAXP_MXPS_MASK) as usize;
                u.pipesel().write_value(0);

                let len = buf.len().min(mps);
                let fifo = FifoPort::d0fifo(u);
                sw_to_hw_fifo(&fifo, buf.as_ptr(), len);
                if len < mps {
                    fifo_bval(&fifo); // short packet — commit now
                }

                critical_section::with(|cs| {
                    let xfer = &mut *HCD_XFER[pipe].borrow(cs).get();
                    xfer.buf = xfer.buf.add(len);
                    xfer.remaining = xfer.remaining.saturating_sub(len as u16);
                });

                u.d0fifosel().write_value(0);
                while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != 0 {}
            }

            // Arm the pipe.
            let ctr = pipectr(u, pipe);
            ctr.write_value((ctr.read() & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF);
            true
        });
        if !queued {
            // `Timeout`: the packet's slot on the bus came and went (PipeError has no
            // "buffer full"); the class driver counts it as a lost packet.
            critical_section::with(|cs| unsafe {
                (*HCD_XFER[pipe].borrow(cs).get()).buf = core::ptr::null_mut();
            });
            return Err(PipeError::Timeout);
        }

        // An isochronous packet has no handshake: once it is in the FIFO with the pipe
        // armed it is done. Waiting for BRDY hangs: a transmitting pipe's BRDY needs one
        // bank emptied *and* the other written (TRM §28.4.2 (1)), so after an underrun
        // drained both banks, the next single packet never raises one.
        let all_written = critical_section::with(|cs| unsafe {
            let xfer = &mut *HCD_XFER[pipe].borrow(cs).get();
            let done = xfer.remaining == 0;
            if done {
                xfer.buf = core::ptr::null_mut();
            }
            done
        });
        if iso && all_written {
            return Ok(());
        }

        poll_pipe(pipe).await.map(|_| ())
    }

    /// Clear the STALL condition on an endpoint and re-enable it.
    pub fn clear_stall(&self, dev_addr: u8, ep_addr: u8) {
        if let Some(pipe) = self.ep_to_pipe(dev_addr, ep_addr) {
            let u = usb(self.port);
            let ctr = pipectr(u, pipe);
            let pid = ctr.read() & PIPECTR_PID_MASK;
            // Transition sequence: STALL → STALL10 → NAK per manual.
            if pid & 0b10 != 0 {
                ctr.write_value(pid & 0b10); // clear low bit → STALL10
                ctr.write_value(0); // → NAK
            }
            // Reset data toggle.
            ctr.write_value(PIPECTR_SQCLR);
            // Re-arm OUT endpoints immediately.
            if ep_addr & 0x80 == 0 {
                ctr.write_value((ctr.read() & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF);
            }
        }
    }

    // --- Internal helpers ---

    /// Prepare the DCP for a control-transfer **status stage**.
    ///
    /// TRM §28.4.6 (1)(c): the status-stage ZLP travels in the direction
    /// *opposite* to the data stage, so DCPCFG.DIR must be flipped, and its
    /// data PID is always DATA1, so SQSET must be written.  Both DCPCFG and
    /// SQSET may only be modified while PID = NAK (after the BUF→NAK
    /// transition has completed, PBUSY = 0).
    ///
    /// `dir_out` = true → host transmits the ZLP (status of a control read);
    /// false → host receives the ZLP (status of a control write / no-data).
    fn dcp_prepare_status(&self, dir_out: bool) {
        let u = usb(self.port);
        // Halt the DCP and wait for the NAK transition to finish.
        let ctr = pipectr(u, 0).read();
        pipectr(u, 0).write_value((ctr & !PIPECTR_PID_MASK) | PIPECTR_PID_NAK);
        while pipectr(u, 0).read() & PIPECTR_PBUSY != 0 {}
        // Status direction; SHTNAK only in the receiving direction
        // (must be 0 when transmitting, TRM §28.3.28).
        let dcpcfg = if dir_out { DCPCFG_DIR } else { DCPCFG_SHTNAK };
        u.dcpcfg().write_value(dcpcfg);
        // Status-stage ZLP is always DATA1.
        pipectr(u, 0).write_value(PIPECTR_SQSET);
    }

    fn ep_to_pipe(&self, dev_addr: u8, ep_addr: u8) -> Option<usize> {
        if dev_addr == 0 || (dev_addr as usize) >= HCD_MAX_DEV {
            return None;
        }
        let epn = (ep_addr & 0x0F) as usize;
        let dir_in = ep_addr & 0x80 != 0;
        if epn == 0 {
            return Some(0);
        } // EP0 is always pipe 0
        let p = self.port as usize;
        critical_section::with(|cs| {
            let alloc = unsafe { &*HCD_ALLOC[p].borrow(cs).get() };
            let pipe = alloc.ep_to_pipe[dev_addr as usize - 1][dir_in as usize][epn - 1] as usize;
            if pipe == 0 { None } else { Some(pipe) }
        })
    }
}

// ---------------------------------------------------------------------------
// Async poll helper
// ---------------------------------------------------------------------------

/// Wait, bounded, for D0FIFO to be ready (FRDY). A buffer bank frees within a
/// (micro)frame; the bound keeps a stopped port from hanging the CPU with interrupts
/// masked.
fn wait_frdy(u: Usb) {
    for _ in 0..100_000u32 {
        if u.d0fifoctr().read() & FIFOCTR_FRDY != 0 {
            return;
        }
    }
}

/// Whether D0FIFO is ready (FRDY) within a few reads: a check, not a wait.
fn frdy_now(u: Usb) -> bool {
    (0..8).any(|_| u.d0fifoctr().read() & FIFOCTR_FRDY != 0)
}

/// Future that resolves when pipe `n`'s transfer completes or errors.
/// Returns `Ok(bytes_transferred)` or the appropriate `PipeError`.
async fn poll_pipe(n: usize) -> Result<usize, PipeError> {
    core::future::poll_fn(move |cx| {
        HCD_PIPE_WAKERS[n].register(cx.waker());
        let mask = 1u16 << n;
        if HCD_PIPE_DONE.load(Ordering::Acquire) & mask != 0 {
            HCD_PIPE_DONE.fetch_and(!mask, Ordering::Release);
            let (length, remaining) = critical_section::with(|cs| unsafe {
                let x = &*HCD_XFER[n].borrow(cs).get();
                (x.length, x.remaining)
            });
            return core::task::Poll::Ready(Ok((length - remaining) as usize));
        }
        if HCD_PIPE_STALL.load(Ordering::Acquire) & mask != 0 {
            HCD_PIPE_STALL.fetch_and(!mask, Ordering::Release);
            return core::task::Poll::Ready(Err(PipeError::Stall));
        }
        if HCD_PIPE_NRDY.load(Ordering::Acquire) & mask != 0 {
            HCD_PIPE_NRDY.fetch_and(!mask, Ordering::Release);
            return core::task::Poll::Ready(Err(PipeError::BadResponse));
        }
        core::task::Poll::Pending
    })
    .await
}

// ---------------------------------------------------------------------------
// Interrupt handler
// ---------------------------------------------------------------------------

/// USB host-mode interrupt handler.
///
/// Wire to GIC IRQ 73 (port 0) or 74 (port 1).  Must only be called from IRQ
/// context on a single-core bare-metal system.
///
/// # Safety
///
/// Must be called from the hardware IRQ handler for the corresponding port.
pub unsafe fn hcd_int_handler(port: u8) {
    unsafe {
        let u = usb(port);

        let is0 = u.intsts(0).read();
        let is1 = u.intsts(1).read();

        // Clear sticky bits by writing 0 to each bit that was set (RC-W0 protocol).
        const STICKY1: u16 =
            INTSTS1_SACK | INTSTS1_SIGN | INTSTS1_EOFERR | INTSTS1_ATTCH | INTSTS1_DTCH;
        u.intsts(1).write_value(!(STICKY1 & is1));
        const STICKY0: u16 = INTSTS0_BRDY | INTSTS0_NRDY | INTSTS0_BEMP;
        u.intsts(0).write_value(!(STICKY0 & is0));

        // Mask with enable registers so we only handle enabled sources.
        let is1 = is1 & u.intenb(1).read();
        let is0 = is0 & u.intenb(0).read();

        // ── SACK: setup packet was ACKed ──────────────────────────────────────
        if is1 & INTSTS1_SACK != 0 {
            // Advance DATA-stage toggle to DATA1.
            let ctr = pipectr(u, 0).read();
            pipectr(u, 0).write_value(ctr | PIPECTR_SQSET);
            HCD_EVENTS.fetch_or(EVT_SACK, Ordering::Release);
            HCD_EVENT_WAKER.wake();
        }

        // ── SIGN: setup packet was ignored (NAK / error) ──────────────────────
        if is1 & INTSTS1_SIGN != 0 {
            HCD_EVENTS.fetch_or(EVT_SIGN, Ordering::Release);
            HCD_EVENT_WAKER.wake();
        }

        // ── ATTCH: a device connected ─────────────────────────────────────────
        if is1 & INTSTS1_ATTCH != 0 {
            // Do NOT enable SOF (UACT) here: generating SOF to a
            // freshly-attached, un-reset device is non-standard.  `bus_reset`
            // owns UACT (it clears it before asserting USBRST and re-enables it
            // as the reset ends).
            critical_section::with(|cs| {
                (&mut *HCD_ALLOC[port as usize].borrow(cs).get()).need_reset = true;
            });
            // Switch to detecting detach.
            let enb = u.intenb(1).read();
            u.intenb(1)
                .write_value((enb & !INTENB1_ATTCHE) | INTENB1_DTCHE);
            HCD_EVENTS.fetch_or(EVT_ATTACH, Ordering::Release);
            HCD_EVENT_WAKER.wake();
        }

        // ── DTCH: the device was removed; EOFERR: the port was stopped ───────
        // An EOF error (activity past the end of a (micro)frame) makes the
        // controller clear UACT just as a detach does (TRM §28.3.4 UACT): no
        // transfer will complete. Handled the same way, so the device, still on
        // the line, is reset and enumerated again on its next attach.
        if is1 & (INTSTS1_DTCH | INTSTS1_EOFERR) != 0 {
            u.dvstctr0().modify(|r| *r &= !DVSTCTR0_UACT);
            // Cancel any pending SETUP.
            let dcpctr = pipectr(u, 0).read();
            if dcpctr & DCPCTR_SUREQ != 0 {
                pipectr(u, 0).write_value((dcpctr & !DCPCTR_SUREQ) | DCPCTR_SUREQCLR)
            }
            // Switch back to detecting attach.
            let enb = u.intenb(1).read();
            u.intenb(1)
                .write_value((enb & !INTENB1_DTCHE) | INTENB1_ATTCHE);
            // Fail every in-flight transfer.  The controller has idled the port
            // (UACT=0) and will emit no further BRDY/NRDY, so any future
            // awaiting a pipe or a SETUP completion would otherwise hang
            // forever.  Mark all pipes failed + wake them, and post EVT_SIGN so
            // an awaiting setup_send resolves to an error.  (Idle pipes clear
            // their NRDY bit when the next transfer arms them.)
            for (n, waker) in HCD_PIPE_WAKERS.iter().enumerate() {
                HCD_PIPE_NRDY.fetch_or(1u16 << n, Ordering::Release);
                waker.wake();
            }
            // The device may still be on the line with no edge left to raise
            // ATTCH (an EOF error leaves it attached): probe again once the loss
            // has been reported.
            PROBE_PENDING[port as usize].store(true, Ordering::Release);
            HCD_EVENTS.fetch_or(EVT_DETACH | EVT_SIGN, Ordering::Release);
            HCD_EVENT_WAKER.wake();
        }

        // ── BEMP: FIFO transmitted — continue control OUT ────────────────────
        if is0 & INTSTS0_BEMP != 0 {
            let bempsts = u.bempsts().read();
            let bempenb = u.bempenb().read();
            let active = bempsts & bempenb;
            u.bempsts().write_value(!active);
            if active & 1 != 0 {
                pipe0_out_bemp(u);
            }
        }

        // ── NRDY: NAK or STALL received ──────────────────────────────────────
        if is0 & INTSTS0_NRDY != 0 {
            let nrdyenb = u.nrdyenb().read();
            let mut active = u.nrdysts().read() & nrdyenb;
            u.nrdysts().write_value(!active);
            while active != 0 {
                let n = active.trailing_zeros() as usize;
                pipe_nrdy(u, n);
                active &= !(1u16 << n);
            }
        }

        // ── BRDY: data received (IN) or slot ready (OUT) ─────────────────────
        if is0 & INTSTS0_BRDY != 0 {
            let brdyenb = u.brdyenb().read();
            let mut active = u.brdysts().read() & brdyenb;
            u.brdysts().write_value(!active);
            while active != 0 {
                let n = active.trailing_zeros() as usize;
                pipe_brdy(u, n);
                active &= !(1u16 << n);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ISR helpers — all must be called only from IRQ context
// ---------------------------------------------------------------------------

/// BRDY on pipe 0 IN (control data stage): read data from CFIFO.
unsafe fn pipe0_in_brdy(u: Usb) {
    unsafe {
        // SAFETY: called from IRQ only; no concurrent async access to CFIFO.
        let cs = critical_section::CriticalSection::new();
        let xfer = &mut *HCD_XFER[0].borrow(cs).get();

        let mps = (u.dcpmaxp().read() & DCPMAXP_MXPS_MASK) as usize;
        let vld = (u.cfifoctr().read() & FIFOCTR_DTLN_MASK) as usize;
        let rem = xfer.remaining as usize;
        let len = rem.min(mps).min(vld);

        // Byte-level read from CFIFO.  The FIFO port is selected at MBW=8; per
        // TRM Table 28.9 (8-bit access, BIGEND=0) the valid received byte sits
        // on bits[31:24] = CPU byte-address base+3, and each MBW=8 access pops
        // one byte and advances the FIFO.  Base+0 is the prohibited lane
        // (TinyUSB's `pipe_read_packet` reads it).
        if len > 0 && !xfer.buf.is_null() {
            let cfifo_byte = u.cfifo_b(3);
            for i in 0..len {
                *xfer.buf.add(i) = cfifo_byte.read();
            }
            xfer.buf = xfer.buf.add(len);
            xfer.remaining = xfer.remaining.saturating_sub(len as u16);
        }

        // BCLR after a short packet to release the FIFO buffer.
        if len < mps {
            u.cfifoctr().write_value(FIFOCTR_BCLR);
        }

        let done = (len < mps) || (xfer.remaining == 0);
        if done {
            xfer.buf = core::ptr::null_mut();
            // NAK to stop receiving further IN tokens.
            pipectr(u, 0).write_value(PIPECTR_PID_NAK);
            HCD_PIPE_DONE.fetch_or(1, Ordering::Release);
            HCD_PIPE_WAKERS[0].wake();
        } else {
            // Re-arm for the next IN packet.
            pipectr(u, 0).write_value(PIPECTR_PID_BUF);
        }
    }
}

/// BEMP on pipe 0 OUT (control data stage): write next chunk to CFIFO.
unsafe fn pipe0_out_bemp(u: Usb) {
    unsafe {
        let cs = critical_section::CriticalSection::new();
        let xfer = &mut *HCD_XFER[0].borrow(cs).get();

        let rem = xfer.remaining as usize;
        if rem == 0 || xfer.buf.is_null() {
            xfer.buf = core::ptr::null_mut();
            HCD_PIPE_DONE.fetch_or(1, Ordering::Release);
            HCD_PIPE_WAKERS[0].wake();
            return;
        }

        let mps = (u.dcpmaxp().read() & DCPMAXP_MXPS_MASK) as usize;
        let len = rem.min(mps);
        let fifo = FifoPort::cfifo(u);
        sw_to_hw_fifo(&fifo, xfer.buf, len);
        if len < mps {
            u.cfifoctr().write_value(FIFOCTR_BVAL);
        }
        xfer.buf = xfer.buf.add(len);
        xfer.remaining = xfer.remaining.saturating_sub(len as u16);
        // BEMP will fire again after this chunk is transmitted.
    }
}

/// BRDY on a non-control IN pipe: read data from D0FIFO.
unsafe fn pipe_brdy_in(u: Usb, n: usize) {
    if RX_DMA_PIPE[port_of(u)].load(Ordering::Acquire) as usize == n {
        unsafe { dma_brdy_in(u, n) };
        return;
    }
    if HCD_PIPE_ISO_IN.load(Ordering::Acquire) & (1 << n) != 0 {
        unsafe { iso_in_brdy(u, n) };
        return;
    }
    unsafe {
        let cs = critical_section::CriticalSection::new();
        let xfer = &mut *HCD_XFER[n].borrow(cs).get();

        if xfer.buf.is_null() {
            // No buffer — discard and BCLR.
            u.d0fifosel().write_value(n as u16);
            while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != n as u16 {}
            u.d0fifoctr().write_value(FIFOCTR_BCLR);
            u.d0fifosel().write_value(0);
            return;
        }

        // Select D0FIFO on this pipe for reading.
        u.d0fifosel().write_value(n as u16);
        while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != n as u16 {}
        while u.d0fifoctr().read() & FIFOCTR_FRDY == 0 {}

        // PIPEMAXP is gated by PIPESEL, not by D0FIFOSEL.CURPIPE — select pipe explicitly.
        u.pipesel().write_value(n as u16);
        let mps = (u.pipemaxp().read() & PIPEMAXP_MXPS_MASK) as usize;
        u.pipesel().write_value(0);
        let vld = (u.d0fifoctr().read() & FIFOCTR_DTLN_MASK) as usize;
        let rem = xfer.remaining as usize;
        let len = rem.min(mps).min(vld);

        // Byte-level read from D0FIFO.  Selected at MBW=8; per TRM Table 28.9
        // (8-bit access, BIGEND=0) the valid byte is on bits[31:24] = base+3,
        // and each access pops one byte and advances the FIFO.  Base+0 is the
        // prohibited lane.
        if len > 0 {
            let d0fifo_byte = u.d0fifo_b(3);
            for i in 0..len {
                *xfer.buf.add(i) = d0fifo_byte.read();
            }
            xfer.buf = xfer.buf.add(len);
            xfer.remaining = xfer.remaining.saturating_sub(len as u16);
        }

        if len < mps {
            u.d0fifoctr().write_value(FIFOCTR_BCLR);
        }

        // Deselect D0FIFO.
        u.d0fifosel().write_value(0);
        while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != 0 {}

        let done = (len < mps) || (xfer.remaining == 0);
        if done {
            xfer.buf = core::ptr::null_mut();
            if HCD_PIPE_INTR_ON_BULK_IN.load(Ordering::Acquire) & (1 << n) != 0 {
                // One block per read: hold the next in the device until it is asked for.
                let ctr = pipectr(u, n);
                ctr.write_value((ctr.read() & !PIPECTR_PID_MASK) | PIPECTR_PID_NAK);
            }
            HCD_PIPE_DONE.fetch_or(1u16 << n, Ordering::Release);
            HCD_PIPE_WAKERS[n].wake();
        }
        // Otherwise the hardware keeps issuing IN tokens until PID changes from BUF.
    }
}

/// BRDY on the receive-DMA pipe `n`: record the packet's length and complete the transfer,
/// holding the pipe at NAK; the reading task moves the data by DMA (`dma_drain`).
unsafe fn dma_brdy_in(u: Usb, n: usize) {
    unsafe {
        let cs = critical_section::CriticalSection::new();
        let xfer = &mut *HCD_XFER[n].borrow(cs).get();
        let dtln = u.d1fifoctr().read() & FIFOCTR_DTLN_MASK;
        RX_DMA_DTLN[port_of(u)].store(dtln, Ordering::Release);
        let ctr = pipectr(u, n);
        ctr.write_value((ctr.read() & !PIPECTR_PID_MASK) | PIPECTR_PID_NAK);
        if xfer.buf.is_null() {
            // No read pending: the packet waits in the FIFO, its length recorded, for the next.
            return;
        }
        xfer.remaining = 0;
        xfer.buf = core::ptr::null_mut();
        HCD_PIPE_DONE.fetch_or(1u16 << n, Ordering::Release);
        HCD_PIPE_WAKERS[n].wake();
    }
}

/// BRDY on isochronous IN pipe `n`: keep the packet in the pipe's ring, read 32 bits at a
/// time. A packet that finds the ring full is dropped (the reader is over a millisecond
/// behind); one that finds no read pending is kept, which is the point.
unsafe fn iso_in_brdy(u: Usb, n: usize) {
    unsafe {
        let fifo = FifoPort::d0fifo(u);
        // MBW=32 with the pipe, then wait for FRDY, which also absorbs the "first read after
        // an MBW write" hazard (see fifo::hw_to_sw_fifo).
        u.d0fifosel()
            .write_value(n as u16 | (MBW_32 << FIFOSEL_MBW_SHIFT));
        while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != n as u16 {}
        if !frdy_now(u) {
            // No packet behind this BRDY (one follows a BCLR): nothing to read.
            u.d0fifosel().write_value(0);
            while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != 0 {}
            return;
        }
        let len = ((u.d0fifoctr().read() & FIFOCTR_DTLN_MASK) as usize).min(ISO_IN_SLOT_BYTES);
        let kept = iso_in_ring(n, |r| {
            if r.count == ISO_IN_SLOTS {
                return false;
            }
            let slot = (r.head + r.count) % ISO_IN_SLOTS;
            hw_to_sw_fifo(&fifo, r.slots[slot].as_mut_ptr(), len);
            r.lens[slot] = len as u16;
            r.count += 1;
            true
        });
        // PIPEMAXP is gated by PIPESEL, not by D0FIFOSEL.CURPIPE.
        u.pipesel().write_value(n as u16);
        let mps = (u.pipemaxp().read() & PIPEMAXP_MXPS_MASK) as usize;
        u.pipesel().write_value(0);
        if !kept || len < mps {
            // Drop the packet, or release a short one's buffer once read: the receive
            // buffer of a packet shorter than the maximum is not released by reading it,
            // and without BCLR no further packet arrives.
            u.d0fifoctr().write_value(FIFOCTR_BCLR);
        }
        u.d0fifosel().write_value(0);
        while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != 0 {}
        HCD_PIPE_WAKERS[n].wake();
    }
}

/// BRDY on a non-control OUT pipe: write next chunk to D0FIFO.
unsafe fn pipe_brdy_out(u: Usb, n: usize) {
    unsafe {
        let cs = critical_section::CriticalSection::new();
        let xfer = &mut *HCD_XFER[n].borrow(cs).get();

        let rem = xfer.remaining as usize;
        if rem == 0 || xfer.buf.is_null() {
            xfer.buf = core::ptr::null_mut();
            HCD_PIPE_DONE.fetch_or(1u16 << n, Ordering::Release);
            HCD_PIPE_WAKERS[n].wake();
            return;
        }

        // Select at MBW=32 to match sw_to_hw_fifo's access width, so its internal
        // MBW assertion is a same-value write (no 8→32 port-switch re-trigger).
        u.d0fifosel()
            .write_value(n as u16 | (MBW_32 << FIFOSEL_MBW_SHIFT));
        while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != n as u16 {}
        while u.d0fifoctr().read() & FIFOCTR_FRDY == 0 {}

        // PIPEMAXP is gated by PIPESEL, not by D0FIFOSEL.CURPIPE — select pipe explicitly.
        u.pipesel().write_value(n as u16);
        let mps = (u.pipemaxp().read() & PIPEMAXP_MXPS_MASK) as usize;
        u.pipesel().write_value(0);
        let len = rem.min(mps);
        let fifo = FifoPort::d0fifo(u);
        sw_to_hw_fifo(&fifo, xfer.buf, len);
        if len < mps {
            fifo_bval(&fifo);
        }
        xfer.buf = xfer.buf.add(len);
        xfer.remaining = xfer.remaining.saturating_sub(len as u16);

        u.d0fifosel().write_value(0);
        while u.d0fifosel().read() & FIFOSEL_CURPIPE_MASK != 0 {}
        // BRDY fires again when the SIE has consumed this chunk.
    }
}

/// Dispatch a BRDY event for pipe `n`.
unsafe fn pipe_brdy(u: Usb, n: usize) {
    unsafe {
        let ep_addr = {
            let cs = critical_section::CriticalSection::new();
            (*HCD_XFER[n].borrow(cs).get()).ep_addr
        };
        let dir_in = ep_addr & 0x80 != 0;

        if n == 0 {
            if dir_in {
                pipe0_in_brdy(u);
            }
            // Pipe-0 OUT is handled via BEMP, not BRDY.
        } else if dir_in {
            pipe_brdy_in(u, n);
        } else {
            pipe_brdy_out(u, n);
        }
    }
}

/// Handle NRDY (NAK or STALL) for pipe `n`.
unsafe fn pipe_nrdy(u: Usb, n: usize) {
    // An isochronous OUT pipe's NRDY means its turn came with nothing to send, and a
    // zero-length packet went instead (TRM §28.4.2 (3)): an underrun, not a failure of the
    // transfer queued now.
    if HCD_PIPE_ISO_OUT.load(Ordering::Acquire) & (1 << n) != 0 {
        return;
    }
    let pid = pipectr(u, n).read() & PIPECTR_PID_MASK;
    // PID 0b10 or 0b11 = STALL; otherwise = NAK / general failure.
    if pid & 0b10 != 0 {
        HCD_PIPE_STALL.fetch_or(1u16 << n, Ordering::Release);
    } else {
        HCD_PIPE_NRDY.fetch_or(1u16 << n, Ordering::Release);
    }
    HCD_PIPE_WAKERS[n].wake();
}

// ---------------------------------------------------------------------------
// embassy-usb-host: UsbHostController + Rusb1Allocator + Rusb1Pipe
// ---------------------------------------------------------------------------

/// An active USB pipe allocated by [`Rusb1Allocator::alloc_pipe`].
///
/// On drop the hardware pipe is freed and the PIPECFG/PIPEMAXP registers are
/// cleared (control pipes that use pipe 0 are exempt — the DCP is permanent).
pub struct Rusb1Pipe<T: pipe::Type, D: pipe::Direction> {
    port: u8,
    /// Hardware pipe number.  0 = control (DCP).
    pipe: u8,
    /// USB device address this pipe targets.
    dev_addr: u8,
    /// Endpoint info (for Drop).
    ep_info: EndpointInfo,
    _phantom: core::marker::PhantomData<(T, D)>,
}

impl<T: pipe::Type, D: pipe::Direction> Drop for Rusb1Pipe<T, D> {
    fn drop(&mut self) {
        // Pipe 0 (DCP / control) is shared and permanently allocated.
        // The hardware pipe slot 0 is never freed.
        if self.pipe == 0 {
            return;
        }
        release_pipe(
            self.port,
            self.pipe as usize,
            self.dev_addr,
            u8::from(self.ep_info.addr),
        );
    }
}

/// Stop and free `pipe`, which carries `ep_addr` of device `dev_addr`.
fn release_pipe(port: u8, pipe: usize, dev_addr: u8, ep_addr: u8) {
    let p = port as usize;
    let epn = (ep_addr & 0x0F) as usize;
    let dir_in = ep_addr & 0x80 != 0;

    let u = usb(port);
    pipectr(u, pipe).write_value(0);
    HCD_PIPE_ISO_OUT.fetch_and(!(1 << pipe), Ordering::Release);
    HCD_PIPE_ISO_IN.fetch_and(!(1 << pipe), Ordering::Release);
    HCD_PIPE_INTR_ON_BULK_IN.fetch_and(!(1 << pipe), Ordering::Release);
    if RX_DMA_PIPE[p].load(Ordering::Acquire) as usize == pipe {
        // DREQE off before CURPIPE changes (TRM §28.3.8 DREQE).
        u.d1fifosel().modify(|r| *r &= !FIFOSEL_DREQE);
        u.d1fifosel().write_value(0);
        RX_DMA_PIPE[p].store(0, Ordering::Release);
        RX_DMA_DTLN[p].store(NO_DTLN, Ordering::Release);
    }
    u.nrdyenb().modify(|r| *r &= !(1 << pipe));
    u.brdyenb().modify(|r| *r &= !(1 << pipe));
    u.pipesel().write_value(pipe as u16);
    u.pipecfg().write_value(0);
    u.pipemaxp().write_value(0);
    u.pipesel().write_value(0);
    critical_section::with(|cs| {
        let alloc = unsafe { &mut *HCD_ALLOC[p].borrow(cs).get() };
        alloc.free_pipe(pipe);
        // Clear the mapping only if it is still this pipe's: a device re-enumerated at the same
        // address may already have the endpoint on another pipe.
        if dev_addr > 0 && (dev_addr as usize) < HCD_MAX_DEV && epn > 0 && epn <= 15 {
            let slot = &mut alloc.ep_to_pipe[dev_addr as usize - 1][dir_in as usize][epn - 1];
            if *slot as usize == pipe {
                *slot = 0;
            }
        }
    });
}

// UsbHostController ───────────────────────────────────────────────────────────

impl<'d> UsbHostController<'d> for Rusb1HostDriver<'d> {
    type Allocator = Rusb1Allocator;

    fn allocator(&self) -> Self::Allocator {
        Rusb1Allocator { port: self.port }
    }

    async fn wait_for_device_event(&mut self) -> DeviceEvent {
        loop {
            // Probe for a device already on the bus (host start, or after a detach or
            // EOF error), but only once any pending detach has been reported.
            // Either step consumes its trigger before it awaits a bus reset; cancelled during
            // the reset, it restores the trigger so the next wait handles it.
            let port = self.port as usize;
            if HCD_EVENTS.load(Ordering::Acquire) & EVT_DETACH == 0
                && PROBE_PENDING[port].swap(false, Ordering::AcqRel)
            {
                let rearm = OnDrop(Some(|| PROBE_PENDING[port].store(true, Ordering::Release)));
                let speed = self.probe_present().await;
                rearm.defuse();
                if let Some(speed) = speed {
                    return DeviceEvent::Connected(speed);
                }
            }

            // Wait for either attach or detach.
            let ev = core::future::poll_fn(|cx| {
                HCD_EVENT_WAKER.register(cx.waker());
                let ev = HCD_EVENTS.fetch_and(!(EVT_ATTACH | EVT_DETACH), Ordering::Acquire);
                if ev & (EVT_ATTACH | EVT_DETACH) != 0 {
                    return core::task::Poll::Ready(ev);
                }
                core::task::Poll::Pending
            })
            .await;

            if ev & EVT_DETACH != 0 {
                return DeviceEvent::Disconnected;
            }

            if ev & EVT_ATTACH != 0 {
                // RHST only reports the device speed after a bus reset.
                let rearm = OnDrop(Some(|| {
                    HCD_EVENTS.fetch_or(EVT_ATTACH, Ordering::AcqRel);
                }));
                let speed = Rusb1HostDriver::bus_reset(&*self).await;
                rearm.defuse();
                match speed {
                    Some(speed) => return DeviceEvent::Connected(speed),
                    None => {
                        log::error!(
                            "usb{}: attach, but the bus reset resolved no speed; waiting for the next attach",
                            self.port
                        );
                        self.watch_for_attach();
                    }
                }
            }
        }
    }

    async fn bus_reset(&mut self) {
        let _ = Rusb1HostDriver::bus_reset(&*self).await;
    }
}

// UsbHostAllocator ────────────────────────────────────────────────────────────

/// Pipe allocator for one RUSB1 host port.
///
/// Cheaply clonable — all controller state lives in module statics, so the
/// allocator only carries the port index.
#[derive(Clone)]
pub struct Rusb1Allocator {
    port: u8,
}

impl<'d> UsbHostAllocator<'d> for Rusb1Allocator {
    type Pipe<T: pipe::Type, D: pipe::Direction> = Rusb1Pipe<T, D>;

    fn alloc_pipe<T: pipe::Type, D: pipe::Direction>(
        &self,
        addr: u8,
        endpoint: &EndpointInfo,
        split: Option<SplitInfo>,
    ) -> Result<Self::Pipe<T, D>, HostError> {
        let drv = Rusb1HostDriver {
            port: self.port,
            _usb: core::marker::PhantomData,
        };

        // Decode the bus topology.  A directly attached device uses hub 0 /
        // port 0 at the root-port speed; a split (hub-attached) device carries
        // the location of its hub's Transaction Translator and its own
        // low/full speed.
        let (hub_addr, hub_port, speed) = match split {
            Some(info) => (
                info.hub_addr(),
                info.port(),
                match info.device_speed() {
                    SplitSpeed::Low => Speed::Low,
                    SplitSpeed::Full => Speed::Full,
                },
            ),
            None => (0, 0, drv.port_speed()),
        };

        match T::ep_type() {
            EndpointType::Control => {
                // Pipe 0 (DCP).  Programme DCPMAXP + DEVADD for this device.
                drv.device_open(addr, endpoint.max_packet_size, speed, hub_addr, hub_port)?;
                Ok(Rusb1Pipe {
                    port: self.port,
                    pipe: 0,
                    dev_addr: addr,
                    ep_info: *endpoint,
                    _phantom: core::marker::PhantomData,
                })
            }
            other => {
                // DEVADDn (hub topology + speed) was programmed when the
                // control pipe for this address was allocated; only the pipe's
                // PIPEMAXP/PIPECFG need configuring here.
                let ep_addr = u8::from(endpoint.addr);
                drv.edpt_open(
                    addr,
                    ep_addr,
                    other,
                    endpoint.max_packet_size,
                    endpoint.interval_ms,
                )?;
                let pipe = drv.ep_to_pipe(addr, ep_addr).ok_or(HostError::OutOfPipes)? as u8;
                Ok(Rusb1Pipe {
                    port: self.port,
                    pipe,
                    dev_addr: addr,
                    ep_info: *endpoint,
                    _phantom: core::marker::PhantomData,
                })
            }
        }
    }
}

// UsbPipe ─────────────────────────────────────────────────────────────────────

impl<T: pipe::Type, D: pipe::Direction> UsbPipe<T, D> for Rusb1Pipe<T, D> {
    /// Control IN: SETUP (IN direction) → DATA IN → STATUS (OUT ZLP).
    async fn control_in(&mut self, setup: &[u8; 8], buf: &mut [u8]) -> Result<usize, PipeError>
    where
        T: pipe::IsControl,
        D: pipe::IsIn,
    {
        let drv = Rusb1HostDriver {
            port: self.port,
            _usb: core::marker::PhantomData,
        };
        drv.setup_send(self.dev_addr, setup).await?;
        let n = drv.control_data_in(self.dev_addr, buf).await?;
        // Status stage: OUT ZLP, opposite direction to the data stage, DATA1.
        drv.dcp_prepare_status(true);
        drv.control_data_out(self.dev_addr, &[]).await?;
        Ok(n)
    }

    /// Control OUT: SETUP (OUT direction) → [DATA OUT] → STATUS (IN ZLP).
    async fn control_out(&mut self, setup: &[u8; 8], buf: &[u8]) -> Result<(), PipeError>
    where
        T: pipe::IsControl,
        D: pipe::IsOut,
    {
        let drv = Rusb1HostDriver {
            port: self.port,
            _usb: core::marker::PhantomData,
        };
        drv.setup_send(self.dev_addr, setup).await?;
        if !buf.is_empty() {
            drv.control_data_out(self.dev_addr, buf).await?;
        }
        // Status stage: IN ZLP, opposite direction to the data stage, DATA1.
        drv.dcp_prepare_status(false);
        let mut dummy = [];
        drv.control_data_in(self.dev_addr, &mut dummy).await?;
        Ok(())
    }

    /// Non-control IN transfer on this pipe.
    async fn request_in(&mut self, buf: &mut [u8]) -> Result<usize, PipeError>
    where
        D: pipe::IsIn,
    {
        let drv = Rusb1HostDriver {
            port: self.port,
            _usb: core::marker::PhantomData,
        };
        let ep_addr = u8::from(self.ep_info.addr);
        drv.xfer_in(self.dev_addr, ep_addr, buf).await
    }

    /// Non-control OUT transfer on this pipe.
    async fn request_out(
        &mut self,
        buf: &[u8],
        _ensure_transaction_end: bool,
    ) -> Result<(), PipeError>
    where
        D: pipe::IsOut,
    {
        let drv = Rusb1HostDriver {
            port: self.port,
            _usb: core::marker::PhantomData,
        };
        let ep_addr = u8::from(self.ep_info.addr);
        drv.xfer_out(self.dev_addr, ep_addr, buf).await
    }

    /// Configure transfer timeouts for this pipe.
    ///
    /// The RUSB1 peripheral does not have a hardware timeout counter; this
    /// configuration is accepted but not acted upon.
    fn set_timeout(&mut self, _timeout: TimeoutConfig)
    where
        T: pipe::IsControl,
    {
    }

    /// Reset the host-side data toggle on this bulk/interrupt pipe to DATA0.
    fn reset_data_toggle(&mut self)
    where
        T: pipe::IsBulkOrInterrupt,
    {
        // The DCP (pipe 0) carries no bulk/interrupt toggle.
        if self.pipe == 0 {
            return;
        }
        let dir_in = u8::from(self.ep_info.addr) & 0x80 != 0;
        let u = usb(self.port);
        let ctr = pipectr(u, self.pipe as usize);
        // SQCLR (reset toggle to DATA0) may only be written while the pipe
        // is halted: NAK it and wait for the SIE to go idle first.
        ctr.write_value((ctr.read() & !PIPECTR_PID_MASK) | PIPECTR_PID_NAK);
        while ctr.read() & PIPECTR_PBUSY != 0 {}
        ctr.write_value(PIPECTR_SQCLR);
        // OUT pipes are re-armed immediately; IN pipes wait for request_in().
        if !dir_in {
            ctr.write_value((ctr.read() & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF);
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    /// The attach tests share the module's event statics; run them one at a time.
    static ATTACH_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Poll `f` to completion, advancing the mock clock 1 ms per pending poll, for at most
    /// `max_ms`. `None` if it is still pending then.
    fn drive<F: core::future::Future>(f: F, max_ms: u64) -> Option<F::Output> {
        let mut f = core::pin::pin!(f);
        let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
        for _ in 0..max_ms {
            if let core::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return Some(v);
            }
            embassy_time::MockDriver::get().advance(embassy_time::Duration::from_millis(1));
        }
        None
    }

    /// Host port 0 brought up with the PLL already running, `lnst` on the lines and `rhst` as
    /// the speed a bus reset resolves to.
    fn host_with_bus(lnst: u16, rhst: u16) -> Rusb1HostDriver<'static> {
        use crate::mmio::test;
        let u = usb(0);
        test::reset();
        test::rcw0(u.intsts(1).addr());
        test::poke16(u.syscfg0().addr(), SYSCFG_UPLLE);
        HCD_EVENTS.store(0, Ordering::Relaxed);
        reset_rx_dma(0);
        let h = unsafe { Rusb1HostDriver::new(0) };
        // The bus state the hardware would report once the controller runs.
        test::poke16(u.syssts0().addr(), lnst);
        test::poke16(u.dvstctr0().addr(), rhst);
        h
    }

    /// An EOF error (activity past the end of a (micro)frame) stops the port: the controller
    /// clears UACT, so no transfer completes and a pending SETUP is never sent (on hardware,
    /// right after a high-speed reset, enumeration hung). It is handled as a detach: in-flight
    /// transfers fail, the event wait reports it, and attach detection is re-armed so the
    /// device, still on the line, is reset and enumerated again.
    #[test]
    fn an_eof_error_is_handled_as_a_detach() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS);
        let u = usb(0);
        assert_ne!(
            u.intenb(1).read() & INTENB1_EOFERRE,
            0,
            "EOFERR enabled at host start"
        );
        h.watch_for_detach();
        HCD_EVENTS.store(0, Ordering::Relaxed);
        HCD_PIPE_NRDY.store(0, Ordering::Relaxed);
        test::poke16(u.intsts(1).addr(), INTSTS1_EOFERR);
        unsafe { hcd_int_handler(0) };
        let ev = HCD_EVENTS.load(Ordering::Acquire);
        assert_ne!(ev & EVT_DETACH, 0, "reported as a detach");
        assert_ne!(ev & EVT_SIGN, 0, "a pending SETUP fails");
        assert_ne!(
            HCD_PIPE_NRDY.load(Ordering::Acquire) & 1,
            0,
            "in-flight transfers fail"
        );
        assert_eq!(
            u.intenb(1).read() & (INTENB1_ATTCHE | INTENB1_DTCHE),
            INTENB1_ATTCHE,
            "watching for attach again"
        );
        assert_eq!(
            test::peek16(u.intsts(1).addr()) & INTSTS1_EOFERR,
            0,
            "status cleared"
        );
    }

    /// A debugger reload is not a chip reset: host start stops SOF and any reset the previous
    /// program left running, then clears USBE to re-initialise the sequencer, before setting
    /// host mode.
    #[test]
    fn host_start_stops_the_controller_first() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let u = usb(0);
        test::reset();
        test::poke16(u.syscfg0().addr(), SYSCFG_UPLLE | SYSCFG_USBE | SYSCFG_DCFM);
        test::poke16(u.dvstctr0().addr(), DVSTCTR0_UACT | DVSTCTR0_RHST_HS);
        let _ = unsafe { Rusb1HostDriver::new(0) };
        let w = test::writes();
        let dv = w
            .iter()
            .position(|&(a, v)| a == u.dvstctr0().addr() && v == 0)
            .expect("SOF stopped");
        let off = w
            .iter()
            .position(|&(a, v)| a == u.syscfg0().addr() && v as u16 & SYSCFG_USBE == 0)
            .expect("USBE cleared");
        let on = w
            .iter()
            .rposition(|&(a, v)| a == u.syscfg0().addr() && v as u16 & SYSCFG_USBE != 0)
            .expect("USBE set again");
        assert!(
            dv < off && off < on,
            "SOF stopped, then USBE cleared, then host mode enabled"
        );
    }

    /// Cancelling a pending SETUP writes SUREQCLR without SUREQ: writing SUREQ=1 requests a
    /// SETUP (TRM §28.3.21), so echoing the read-back value re-requested the one being cancelled.
    #[test]
    fn cancelling_a_pending_setup_does_not_request_another() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b01, DVSTCTR0_RHST_FS | DVSTCTR0_UACT);
        let u = usb(0);
        h.watch_for_detach();
        test::poke16(pipectr(u, 0).addr(), DCPCTR_SUREQ);
        test::poke16(u.intsts(1).addr(), INTSTS1_DTCH);
        let start = test::log().len();
        unsafe { hcd_int_handler(0) };
        let clr = test::log()[start..]
            .iter()
            .find(|a| {
                a.write && a.addr == pipectr(u, 0).addr() && a.val as u16 & DCPCTR_SUREQCLR != 0
            })
            .copied()
            .expect("the detach cancels the pending SETUP");
        assert_eq!(
            clr.val as u16 & DCPCTR_SUREQ,
            0,
            "SUREQ not written with SUREQCLR"
        );
    }

    /// The device the probe finds may also have raised ATTCH (the line came up at J while
    /// attach detection was armed). That attach is the same device: once the probe reports it,
    /// the next event wait must not report it again (it would reset the bound device).
    #[test]
    fn the_attach_a_probe_answers_is_not_reported_again() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut h = host_with_bus(0b01, DVSTCTR0_RHST_FS);
        HCD_EVENTS.store(EVT_ATTACH, Ordering::Relaxed);
        assert_eq!(
            drive(h.wait_for_device_event(), 200),
            Some(DeviceEvent::Connected(Speed::Full))
        );
        assert_eq!(
            drive(h.wait_for_device_event(), 200),
            None,
            "no second Connected"
        );
    }

    /// An isochronous OUT packet is done once it is in the FIFO with the pipe armed: there is no
    /// handshake to wait for. Waiting for a BRDY hung the stream: a transmitting pipe's BRDY
    /// needs one bank emptied *and* the other written (TRM §28.4.2 (1)), so after an underrun
    /// had drained both banks the next single packet never raised one.
    #[test]
    fn an_isochronous_out_packet_completes_without_an_interrupt() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        let u = usb(0);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(1, 0x03, EndpointType::Isochronous, 56, 1)
            .unwrap();
        test::poke16(u.d0fifoctr().addr(), FIFOCTR_FRDY);
        assert_eq!(drive(h.xfer_out(1, 0x03, &[0u8; 48]), 3), Some(Ok(())));
    }

    /// An isochronous IN pipe is opened with its packet ring empty, then `packet` simulates one
    /// arriving (BRDY with `len` bytes of the repeated `word` in D0FIFO).
    fn iso_in_pipe(h: &Rusb1HostDriver<'_>, ep: u8) -> usize {
        use crate::mmio::test;
        let u = usb(0);
        test::rcw0(u.intsts(0).addr());
        test::rcw0(u.brdysts().addr());
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(1, ep, EndpointType::Isochronous, 56, 1)
            .unwrap();
        h.ep_to_pipe(1, ep).unwrap()
    }

    fn packet(pipe: usize, word: u32, len: u16) {
        use crate::mmio::test;
        let u = usb(0);
        test::poke32(u.d0fifo().addr(), word);
        test::poke16(u.d0fifoctr().addr(), FIFOCTR_FRDY | len);
        test::poke16(u.brdysts().addr(), 1 << pipe);
        test::poke16(u.intsts(0).addr(), INTSTS0_BRDY);
        unsafe { hcd_int_handler(0) };
    }

    /// Capture packets arrive every microframe whether or not a read is waiting. They are kept
    /// in order until read: dropping the ones that found no read pending lost ~4.5% of a
    /// high-speed Digitakt stream. They are read from the FIFO 32 bits at a time.
    #[test]
    fn isochronous_in_packets_are_kept_until_read() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        let pipe = iso_in_pipe(&h, 0x83);
        let start = test::log().len();
        packet(pipe, 0x4433_2211, 8);
        packet(pipe, 0x8877_6655, 4);
        let mut buf = [0u8; 56];
        assert_eq!(drive(h.xfer_in(1, 0x83, &mut buf), 2), Some(Ok(8)));
        assert_eq!(buf[..8], [0x11, 0x22, 0x33, 0x44, 0x11, 0x22, 0x33, 0x44]);
        assert_eq!(drive(h.xfer_in(1, 0x83, &mut buf), 2), Some(Ok(4)));
        assert_eq!(buf[..4], [0x55, 0x66, 0x77, 0x88]);
        let u = usb(0);
        let fifo = u.d0fifo().addr();
        assert!(
            !test::log()[start..]
                .iter()
                .any(|a| !a.write && a.width == 1 && (fifo..fifo + 4).contains(&a.addr)),
            "no byte-wide FIFO reads"
        );
        let bclrs = test::log()[start..]
            .iter()
            .filter(|a| {
                a.write && a.addr == u.d0fifoctr().addr() && a.val as u16 & FIFOCTR_BCLR != 0
            })
            .count();
        assert_eq!(
            bclrs, 2,
            "a short packet's buffer is released (BCLR) once read"
        );
    }

    /// An isochronous OUT packet that finds both FIFO banks full is dropped at once (a lost
    /// packet, as isochronous allows), not waited for: the write holds interrupts off, and a
    /// burst of packets spun there until the system starved.
    #[test]
    fn an_isochronous_out_packet_is_dropped_when_the_fifo_is_full() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        let u = usb(0);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(1, 0x03, EndpointType::Isochronous, 56, 1)
            .unwrap();
        test::poke16(u.d0fifoctr().addr(), 0); // FRDY clear: both banks full
        let start = test::log().len();
        assert_eq!(
            drive(h.xfer_out(1, 0x03, &[0u8; 48]), 2),
            Some(Err(PipeError::Timeout))
        );
        let reads = test::log()[start..]
            .iter()
            .filter(|a| !a.write && a.addr == u.d0fifoctr().addr())
            .count();
        assert!(reads < 100, "checked briefly, not spun on: {reads} reads");
        assert!(
            !test::log()[start..]
                .iter()
                .any(|a| a.write && a.addr == u.d0fifo().addr()),
            "nothing written"
        );
    }

    /// A BRDY whose FIFO is not ready (no packet behind it, as after a BCLR) queues nothing and
    /// is not waited on: the handler spun on FRDY and queued empty packets, and the stream
    /// starved.
    #[test]
    fn a_spurious_isochronous_brdy_queues_nothing() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        let pipe = iso_in_pipe(&h, 0x83);
        let u = usb(0);
        let start = test::log().len();
        test::poke16(u.d0fifoctr().addr(), 0); // FRDY clear
        test::poke16(u.brdysts().addr(), 1 << pipe);
        test::poke16(u.intsts(0).addr(), INTSTS0_BRDY);
        unsafe { hcd_int_handler(0) };
        let reads = test::log()[start..]
            .iter()
            .filter(|a| !a.write && a.addr == u.d0fifoctr().addr())
            .count();
        assert!(reads < 100, "checked briefly, not spun on: {reads} reads");
        let mut buf = [0u8; 56];
        assert_eq!(
            drive(h.xfer_in(1, 0x83, &mut buf), 2),
            None,
            "no packet queued"
        );
    }

    /// Isochronous pipes raise no NRDY interrupts: an OUT pipe's NRDY is an underrun report and
    /// an IN pipe's an overrun or a missed packet, each microframe, neither a failed transfer
    /// (about 8000 interrupts a second for nothing). A bulk pipe's NRDY is a failure.
    #[test]
    fn isochronous_pipes_raise_no_nrdy_interrupts() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(1, 0x03, EndpointType::Isochronous, 56, 1)
            .unwrap();
        h.edpt_open(1, 0x83, EndpointType::Isochronous, 56, 1)
            .unwrap();
        h.edpt_open(1, 0x02, EndpointType::Bulk, 512, 0).unwrap();
        let enb = usb(0).nrdyenb().read();
        assert_eq!(enb & (1 << h.ep_to_pipe(1, 0x03).unwrap()), 0);
        assert_eq!(enb & (1 << h.ep_to_pipe(1, 0x83).unwrap()), 0);
        assert_ne!(enb & (1 << h.ep_to_pipe(1, 0x02).unwrap()), 0);
    }

    /// A detach still ends a capture read that is waiting for a packet.
    #[test]
    fn a_detach_ends_a_waiting_isochronous_read() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        let _pipe = iso_in_pipe(&h, 0x83);
        h.watch_for_detach();
        let mut buf = [0u8; 56];
        let mut f = core::pin::pin!(h.xfer_in(1, 0x83, &mut buf));
        let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
        assert!(
            f.as_mut().poll(&mut cx).is_pending(),
            "waiting for a packet"
        );
        test::poke16(usb(0).intsts(1).addr(), INTSTS1_DTCH);
        unsafe { hcd_int_handler(0) };
        assert_eq!(
            f.as_mut().poll(&mut cx),
            core::task::Poll::Ready(Err(PipeError::BadResponse))
        );
    }

    /// A bus reset ends with the line returning from SE0 to J: the attach condition, so ATTCH
    /// latches. Left armed, that raised a phantom attach after the device was bound, and the
    /// supervisor reset the device out from under its drivers. The reset masks attach/detach
    /// detection, clears what it latched, and restores detection after.
    #[test]
    fn a_bus_reset_does_not_raise_an_attach() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b01, DVSTCTR0_RHST_FS);
        let u = usb(0);
        let armed = u.intenb(1).read();
        assert_ne!(armed & INTENB1_ATTCHE, 0);
        let mut f = core::pin::pin!(h.bus_reset());
        let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
        assert!(f.as_mut().poll(&mut cx).is_pending(), "reset under way");
        assert_eq!(
            u.intenb(1).read() & (INTENB1_ATTCHE | INTENB1_DTCHE),
            0,
            "attach/detach detection masked during the reset"
        );
        test::poke16(u.intsts(1).addr(), INTSTS1_ATTCH); // latched as the line returns to J
        HCD_EVENTS.store(0, Ordering::Relaxed);
        let speed = loop {
            if let core::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                break v;
            }
            embassy_time::MockDriver::get().advance(embassy_time::Duration::from_millis(1));
        };
        assert_eq!(speed, Some(Speed::Full));
        assert_eq!(
            test::peek16(u.intsts(1).addr()) & INTSTS1_ATTCH,
            0,
            "latch cleared"
        );
        assert_eq!(u.intenb(1).read(), armed, "detection restored");
        assert_eq!(
            HCD_EVENTS.load(Ordering::Acquire) & EVT_ATTACH,
            0,
            "no attach event"
        );
    }

    /// A SETUP on a stopped port (UACT=0, as a detach or EOF error leaves it) would never be
    /// sent and never complete. It fails at once, so a caller's retry cannot hang either.
    #[test]
    fn a_setup_on_a_stopped_port_fails_at_once() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b01, 0); // UACT clear
        let setup = [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x08, 0x00];
        assert_eq!(
            drive(h.setup_send(0, &setup), 5),
            Some(Err(PipeError::Canceled))
        );
    }

    /// The same for a data transfer: on a stopped port it fails at once instead of waiting
    /// for an interrupt that will not come.
    #[test]
    fn a_data_transfer_on_a_stopped_port_fails_at_once() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b01, 0); // UACT clear
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(1, 0x81, EndpointType::Bulk, 512, 0).unwrap();
        let mut buf = [0u8; 64];
        assert_eq!(
            drive(h.xfer_in(1, 0x81, &mut buf), 5),
            Some(Err(PipeError::Canceled))
        );
    }

    /// After a detach or EOF error the device may still be on the line, holding it at J with
    /// no edge to raise ATTCH. The host reports the loss, then probes again: the device is
    /// found and enumerated anew (an empty bus probes to nothing and waits for an attach).
    #[test]
    fn after_an_eof_error_the_host_reports_it_then_finds_the_device_again() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut h = host_with_bus(0b01, DVSTCTR0_RHST_HS);
        assert!(
            drive(h.wait_for_device_event(), 200).is_some(),
            "first found at start"
        );
        let u = usb(0);
        test::poke16(u.intsts(1).addr(), INTSTS1_EOFERR);
        unsafe { hcd_int_handler(0) };
        assert_eq!(
            drive(h.wait_for_device_event(), 200),
            Some(DeviceEvent::Disconnected)
        );
        assert_eq!(
            drive(h.wait_for_device_event(), 200),
            Some(DeviceEvent::Connected(Speed::High)),
            "re-probed and found"
        );
    }

    /// An isochronous OUT pipe raises NRDY in each microframe it has nothing to send (TRM
    /// §28.4.2 (3): it sends a zero-length packet and sets OVRN). That reports an underrun, not a
    /// failed transfer: a packet still queued in the other buffer bank must not be reported
    /// lost. On a bulk OUT pipe NRDY (three errors, or a STALL) does fail the transfer.
    #[test]
    fn nrdy_on_an_isochronous_out_pipe_does_not_fail_its_transfer() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, 0);
        let u = usb(0);
        test::rcw0(u.intsts(0).addr());
        test::rcw0(u.nrdysts().addr());
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        h.edpt_open(1, 0x01, EndpointType::Isochronous, 400, 1)
            .unwrap();
        h.edpt_open(1, 0x02, EndpointType::Bulk, 512, 0).unwrap();
        let iso = h.ep_to_pipe(1, 0x01).unwrap();
        let bulk = h.ep_to_pipe(1, 0x02).unwrap();
        HCD_PIPE_NRDY.store(0, Ordering::Relaxed);
        test::poke16(u.intsts(0).addr(), INTSTS0_NRDY);
        test::poke16(u.nrdysts().addr(), (1 << iso) | (1 << bulk));
        unsafe { hcd_int_handler(0) };
        let nrdy = HCD_PIPE_NRDY.load(Ordering::Acquire);
        assert_eq!(
            nrdy & (1 << iso),
            0,
            "isochronous OUT underrun is not a failure"
        );
        assert_ne!(
            nrdy & (1 << bulk),
            0,
            "bulk OUT NRDY still fails its transfer"
        );
    }

    /// A previous program's latched ATTCH/DTCH must not survive host start: the first real
    /// attach or detach is then the only one the ISR sees.
    #[test]
    fn host_start_clears_stale_attach_and_detach_status() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let u = usb(0);
        test::reset();
        test::rcw0(u.intsts(1).addr());
        test::poke16(u.syscfg0().addr(), SYSCFG_UPLLE);
        test::poke16(u.intsts(1).addr(), INTSTS1_ATTCH | INTSTS1_DTCH | 0x4000);
        let _ = unsafe { Rusb1HostDriver::new(0) };
        assert_eq!(test::peek16(u.intsts(1).addr()), 0);
    }

    /// A device already on the bus when the host starts (plugged in before boot, or left there
    /// by the firmware that ran before) makes no attach edge. The line already reads J, and the
    /// first event wait reports it.
    #[test]
    fn a_device_already_on_the_bus_is_reported_without_an_attach_interrupt() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut h = host_with_bus(0b01, DVSTCTR0_RHST_FS);
        let ev = drive(h.wait_for_device_event(), 200);
        assert_eq!(ev, Some(DeviceEvent::Connected(Speed::Full)));
        let enb = usb(0).intenb(1).read();
        assert_eq!(
            enb & (INTENB1_ATTCHE | INTENB1_DTCHE),
            INTENB1_DTCHE,
            "now watching for detach"
        );
    }

    /// A high-speed device a previous host left idle holds no pull-up, so the line reads SE0.
    /// A probing reset finds it: the handshake resolves to high speed.
    #[test]
    fn an_idle_high_speed_device_is_found_by_a_probing_reset() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut h = host_with_bus(0b00, DVSTCTR0_RHST_HS);
        let ev = drive(h.wait_for_device_event(), 200);
        assert_eq!(ev, Some(DeviceEvent::Connected(Speed::High)));
    }

    /// Nothing on the bus: the probing reset resolves no speed, so there is no event, SOF is
    /// off, and attach detection stays armed.
    #[test]
    fn an_empty_bus_keeps_waiting_for_an_attach() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut h = host_with_bus(0b00, 0);
        assert_eq!(drive(h.wait_for_device_event(), 200), None);
        let u = usb(0);
        assert_eq!(
            u.dvstctr0().read() & DVSTCTR0_UACT,
            0,
            "no SOF on an empty bus"
        );
        assert_ne!(
            u.intenb(1).read() & INTENB1_ATTCHE,
            0,
            "attach detection armed"
        );
    }

    /// An attach whose bus reset never resolves a speed is not a full-speed device: the host
    /// reports nothing, and watches for the next attach.
    #[test]
    fn an_attach_whose_reset_resolves_no_speed_is_not_reported() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut h = host_with_bus(0b00, 0);
        HCD_EVENTS.store(EVT_ATTACH, Ordering::Relaxed);
        assert_eq!(drive(h.wait_for_device_event(), 300), None);
        let u = usb(0);
        assert_eq!(
            u.intenb(1).read() & (INTENB1_ATTCHE | INTENB1_DTCHE),
            INTENB1_ATTCHE,
            "attach detection re-armed"
        );
    }

    /// Host bring-up on port 1 with the PLL off: both channels' SUSPM drop before UPLLE (in
    /// USB0's SYSCFG0) rises, and port 1's DCFM rises while DRPD, DPRPU and HSE are 0 (TRM §28.3.1),
    /// before DRPD and then USBE.
    #[test]
    fn host_bring_up_orders_the_pll_and_mode_writes() {
        use crate::mmio::test;
        const SYSCFG0_0: usize = 0xE801_0000;
        const SYSCFG0_1: usize = 0xE820_7000;
        const SUSPMODE_0: usize = 0xE801_0102;
        const SUSPMODE_1: usize = 0xE820_7102;
        test::reset();
        test::poke16(SYSCFG0_1, SYSCFG_DRPD | SYSCFG_DPRPU | SYSCFG_HSE);
        test::poke16(SUSPMODE_0, SUSPMODE_SUSPM);
        test::poke16(SUSPMODE_1, SUSPMODE_SUSPM);
        let _ = unsafe { Rusb1HostDriver::new(1) };
        let w: Vec<_> = test::writes()
            .into_iter()
            .filter(|(a, _)| [SYSCFG0_0, SYSCFG0_1, SUSPMODE_0, SUSPMODE_1].contains(a))
            .collect();
        assert_eq!(
            w,
            [
                (SUSPMODE_1, 0),
                (SUSPMODE_0, 0),
                (SYSCFG0_0, SYSCFG_UPLLE as u32),
                (SUSPMODE_1, SUSPMODE_SUSPM as u32),
                // USBE cleared first: the controller starts stopped.
                (SYSCFG0_1, (SYSCFG_DRPD | SYSCFG_DPRPU | SYSCFG_HSE) as u32),
                (SYSCFG0_1, SYSCFG_DCFM as u32),
                (SYSCFG0_1, (SYSCFG_DCFM | SYSCFG_DRPD) as u32),
                (SYSCFG0_1, (SYSCFG_DCFM | SYSCFG_DRPD | SYSCFG_USBE) as u32),
            ]
        );
    }

    #[test]
    fn dev_addr_zero_is_not_supported() {
        // Address 0 is the enumeration default address, never a target.
        assert!(!dev_addr_supported(0));
    }

    #[test]
    fn dev_addr_in_range_is_supported() {
        for addr in 1..HCD_MAX_DEV as u8 {
            assert!(dev_addr_supported(addr), "addr {addr} should be supported");
        }
    }

    #[test]
    fn dev_addr_at_or_above_max_is_not_supported() {
        // HCD_MAX_DEV is exclusive: ctl_mps and ep_to_pipe are sized from it.
        assert!(!dev_addr_supported(HCD_MAX_DEV as u8));
        assert!(!dev_addr_supported(127));
    }

    #[test]
    fn host_pipe_buffers_are_disjoint_and_clear_of_the_dcp() {
        // Without an explicit PIPEBUF every bulk/iso pipe would default to
        // block 0 (the DCP region). Two 512-byte double-buffered bulk pipes
        // must get disjoint regions, both clear of the reserved blocks 0-6.
        let mut a = HcdAlloc::new();
        let p1 = a.alloc_pipe(1, 5).unwrap();
        let (s1, n1) = a.alloc_pipe_buf(p1, 512, true).unwrap();
        let p2 = a.alloc_pipe(1, 5).unwrap();
        let (s2, n2) = a.alloc_pipe_buf(p2, 512, true).unwrap();

        assert_eq!(n1, 8, "512 bytes = 8 blocks per bank");
        assert_eq!(n2, 8);
        assert!(s1 >= 7, "must not touch reserved blocks 0-6, got {s1}");
        assert!(s2 >= 7, "must not touch reserved blocks 0-6, got {s2}");
        // Each reserves two banks (DBLB); ranges must not intersect.
        let (e1, e2) = (s1 as usize + 2 * n1 as usize, s2 as usize + 2 * n2 as usize);
        let disjoint = (e1 <= s2 as usize) || (e2 <= s1 as usize);
        assert!(disjoint, "pipe buffers overlap: [{s1},{e1}) vs [{s2},{e2})");

        // Freeing the pipe returns its blocks to the pool.
        a.free_pipe(p1);
        assert_eq!(a.buf_reserved[p1], 0);
    }

    /// The Digitakt's Overbridge IN endpoint (interrupt, 368 bytes) is too big for an
    /// interrupt pipe (6-9 stop at 64), so it rides a bulk pipe. Pipes 3-5 only take MXPS
    /// 8/16/32/64/512, so it is programmed at 512 and every block arrives as a short packet.
    #[test]
    fn an_interrupt_endpoint_over_64_bytes_is_carried_on_a_bulk_pipe_at_mxps_512() {
        assert_eq!(
            pipe_layout(EndpointType::Interrupt, 368),
            Ok(PipeLayout {
                first: 1,
                last: 5,
                mxps: 512,
                carry: EndpointType::Bulk
            })
        );
        assert_eq!(
            pipe_layout(EndpointType::Interrupt, 88),
            Ok(PipeLayout {
                first: 1,
                last: 5,
                mxps: 512,
                carry: EndpointType::Bulk
            })
        );
    }

    /// Over 512 bytes (a Digitakt II's 1012-byte IN endpoint) only pipes 1-2 take the size.
    #[test]
    fn an_interrupt_endpoint_over_512_bytes_needs_pipe_1_or_2() {
        assert_eq!(
            pipe_layout(EndpointType::Interrupt, 1012),
            Ok(PipeLayout {
                first: 1,
                last: 2,
                mxps: 1012,
                carry: EndpointType::Bulk
            })
        );
        assert_eq!(
            pipe_layout(EndpointType::Interrupt, 1024),
            Ok(PipeLayout {
                first: 1,
                last: 2,
                mxps: 1024,
                carry: EndpointType::Bulk
            })
        );
    }

    /// MIDI and hub status endpoints keep the interrupt pipes.
    #[test]
    fn an_interrupt_endpoint_of_64_bytes_or_less_keeps_an_interrupt_pipe() {
        for mps in [1, 8, 64] {
            assert_eq!(
                pipe_layout(EndpointType::Interrupt, mps),
                Ok(PipeLayout {
                    first: 6,
                    last: 9,
                    mxps: mps,
                    carry: EndpointType::Interrupt
                })
            );
        }
    }

    /// A high-bandwidth interrupt endpoint (wMaxPacketSize bits 12:11 = extra transactions per
    /// microframe) can't ride a bulk pipe, and one over 1024 bytes isn't valid USB.
    #[test]
    fn a_high_bandwidth_or_oversized_interrupt_endpoint_is_rejected() {
        assert!(pipe_layout(EndpointType::Interrupt, (1 << 11) | 368).is_err());
        assert!(pipe_layout(EndpointType::Interrupt, (2 << 11) | 1024).is_err());
        assert!(pipe_layout(EndpointType::Interrupt, 1025).is_err());
    }

    /// Bulk and isochronous endpoints are laid out as before.
    #[test]
    fn bulk_and_isochronous_layouts_are_unchanged() {
        assert_eq!(
            pipe_layout(EndpointType::Bulk, 512),
            Ok(PipeLayout {
                first: 1,
                last: 5,
                mxps: 512,
                carry: EndpointType::Bulk
            })
        );
        assert_eq!(
            pipe_layout(EndpointType::Bulk, 64),
            Ok(PipeLayout {
                first: 1,
                last: 5,
                mxps: 64,
                carry: EndpointType::Bulk
            })
        );
        assert_eq!(
            pipe_layout(EndpointType::Isochronous, 56),
            Ok(PipeLayout {
                first: 1,
                last: 2,
                mxps: 56,
                carry: EndpointType::Isochronous
            })
        );
        assert_eq!(
            pipe_layout(EndpointType::Control, 64),
            Err(HostError::OutOfPipes)
        );
    }

    /// Open one endpoint on device 1 (high speed) of a fresh port-0 allocator, and return the
    /// pipe with the PIPECFG/PIPEMAXP/PIPEPERI values `edpt_open` wrote.
    fn open_on_fresh_port(
        ep: u8,
        ty: EndpointType,
        mps: u16,
        binterval: u8,
    ) -> (usize, u16, u16, u16) {
        use crate::mmio::test;
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        test::poke16(usb(0).devadd(1).addr(), DEVADD_USBSPD_HS);
        h.edpt_open(1, ep, ty, mps, binterval).unwrap();
        let pipe = h.ep_to_pipe(1, ep).unwrap();
        let u = usb(0);
        (
            pipe,
            u.pipecfg().read(),
            u.pipemaxp().read(),
            u.pipeperi().read(),
        )
    }

    fn reserved_blocks(pipe: usize) -> u8 {
        critical_section::with(|cs| unsafe { (*HCD_ALLOC[0].borrow(cs).get()).buf_reserved[pipe] })
    }

    #[test]
    fn a_368_byte_interrupt_in_endpoint_opens_as_a_double_buffered_bulk_pipe() {
        use crate::usb::regs::{PIPECFG_DBLB, PIPECFG_DIR, PIPECFG_SHTNAK, PIPECFG_TYPE_MASK};
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (pipe, cfg, maxp, peri) = open_on_fresh_port(0x83, EndpointType::Interrupt, 368, 1);
        assert!((3..=5).contains(&pipe), "pipe {pipe}");
        assert_eq!(cfg & PIPECFG_TYPE_MASK, PIPECFG_TYPE_BULK);
        assert_ne!(cfg & PIPECFG_DBLB, 0);
        assert_ne!(cfg & PIPECFG_SHTNAK, 0);
        assert_eq!(cfg & PIPECFG_DIR, 0, "IN: host receives");
        assert_eq!(maxp & PIPEMAXP_MXPS_MASK, 512);
        assert_eq!(peri & PIPEPERI_IITV_MASK, 0, "IITV must be 0 on pipes 3-5");
        assert_eq!(reserved_blocks(pipe), 16, "2 banks x 512 bytes");
    }

    #[test]
    fn an_88_byte_interrupt_out_endpoint_opens_as_a_bulk_pipe() {
        use crate::usb::regs::{PIPECFG_DIR, PIPECFG_TYPE_MASK};
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (pipe, cfg, maxp, _) = open_on_fresh_port(0x03, EndpointType::Interrupt, 88, 1);
        assert!((3..=5).contains(&pipe), "pipe {pipe}");
        assert_eq!(cfg & PIPECFG_TYPE_MASK, PIPECFG_TYPE_BULK);
        assert_ne!(cfg & PIPECFG_DIR, 0, "OUT: host sends");
        assert_eq!(maxp & PIPEMAXP_MXPS_MASK, 512);
    }

    #[test]
    fn a_1012_byte_interrupt_endpoint_opens_on_pipe_1_or_2_at_its_own_size() {
        use crate::usb::regs::PIPECFG_TYPE_MASK;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (pipe, cfg, maxp, _) = open_on_fresh_port(0x83, EndpointType::Interrupt, 1012, 1);
        assert!((1..=2).contains(&pipe), "pipe {pipe}");
        assert_eq!(cfg & PIPECFG_TYPE_MASK, PIPECFG_TYPE_BULK);
        assert_eq!(maxp & PIPEMAXP_MXPS_MASK, 1012);
        assert_eq!(reserved_blocks(pipe), 32, "2 banks x 16 blocks");
    }

    #[test]
    fn a_64_byte_interrupt_endpoint_keeps_pipes_6_to_9() {
        use crate::usb::regs::PIPECFG_TYPE_MASK;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (pipe, cfg, maxp, _) = open_on_fresh_port(0x82, EndpointType::Interrupt, 64, 1);
        assert!((6..=9).contains(&pipe), "pipe {pipe}");
        assert_eq!(cfg & PIPECFG_TYPE_MASK, PIPECFG_TYPE_INTR);
        assert_eq!(maxp & PIPEMAXP_MXPS_MASK, 64);
    }

    /// A Digitakt opens its two Overbridge endpoints and its two USB-MIDI bulk endpoints: four
    /// of the five bulk-capable pipes. One isochronous stream (a USB audio interface) still
    /// fits; a second doesn't, and fails cleanly.
    #[test]
    fn a_digitakt_leaves_one_isochronous_pipe() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        crate::mmio::test::poke16(usb(0).devadd(1).addr(), DEVADD_USBSPD_HS);
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        h.edpt_open(1, 0x03, EndpointType::Interrupt, 88, 1)
            .unwrap();
        h.edpt_open(1, 0x81, EndpointType::Bulk, 512, 0).unwrap();
        h.edpt_open(1, 0x01, EndpointType::Bulk, 512, 0).unwrap();
        let mut pipes: Vec<usize> = [0x83, 0x03, 0x81, 0x01]
            .iter()
            .map(|&ep| h.ep_to_pipe(1, ep).unwrap())
            .collect();
        pipes.sort();
        pipes.dedup();
        assert_eq!(pipes.len(), 4, "distinct pipes");
        assert!(pipes.iter().all(|p| (1..=5).contains(p)));

        h.edpt_open(2, 0x84, EndpointType::Isochronous, 196, 1)
            .unwrap();
        assert_eq!(
            h.edpt_open(2, 0x05, EndpointType::Isochronous, 196, 1),
            Err(HostError::OutOfPipes)
        );
    }

    /// Characterization: this behaviour exists before the change, and the test pins it.
    /// When the packet buffer can't hold a pipe, the open fails and the pipe goes back.
    #[test]
    fn a_full_packet_buffer_fails_the_open_and_frees_the_pipe() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe {
            let a = &mut *HCD_ALLOC[0].borrow(cs).get();
            *a = HcdAlloc::new();
            // Leave fewer than the 16 blocks a 512-byte double-buffered pipe needs.
            while a.buf.alloc(1).is_some() {}
        });
        assert_eq!(
            h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1),
            Err(HostError::OutOfPipes)
        );
        let used =
            critical_section::with(|cs| unsafe { (*HCD_ALLOC[0].borrow(cs).get()).pipes_used });
        assert_eq!(used, 0x0001, "only the DCP stays allocated");
    }

    #[test]
    fn a_high_speed_device_s_interrupt_pipe_counts_microframes() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (_, _, _, peri) = open_on_fresh_port(0x81, EndpointType::Interrupt, 8, 4);
        assert_eq!(peri & PIPEPERI_IITV_MASK, 3, "2^3 microframes = 1 ms");
    }

    #[test]
    fn a_full_speed_device_s_interrupt_pipe_counts_frames() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        // A full-speed device behind a high-speed hub: DEVADDn says full speed.
        test::poke16(
            usb(0).devadd(1).addr(),
            DEVADD_USBSPD_FS | (1 << DEVADD_HUBPORT_SHIFT),
        );
        h.edpt_open(1, 0x81, EndpointType::Interrupt, 8, 4).unwrap();
        assert_eq!(
            usb(0).pipeperi().read() & PIPEPERI_IITV_MASK,
            2,
            "2^2 frames = 4 ms"
        );
    }

    /// High speed: bInterval is an exponent, period 2^(bInterval-1) microframes, and the
    /// controller counts microframes for a high-speed device (TRM §28.4.8 (1)(c)). IITV tops
    /// out at 7 (16 ms).
    #[test]
    fn interrupt_iitv_at_high_speed_is_binterval_minus_one() {
        assert_eq!(interrupt_iitv(1, true), 0);
        assert_eq!(interrupt_iitv(4, true), 3);
        assert_eq!(interrupt_iitv(8, true), 7);
        assert_eq!(interrupt_iitv(12, true), 7);
        assert_eq!(interrupt_iitv(16, true), 7);
    }

    /// Full and low speed (including a full-speed device behind a high-speed hub, which the
    /// controller counts in 1 ms frames): bInterval is in frames, rounded down to a power of 2.
    #[test]
    fn interrupt_iitv_at_full_speed_rounds_binterval_down_to_a_power_of_two() {
        assert_eq!(interrupt_iitv(1, false), 0);
        assert_eq!(interrupt_iitv(2, false), 1);
        assert_eq!(interrupt_iitv(10, false), 3);
        assert_eq!(interrupt_iitv(255, false), 7);
    }

    /// embassy-usb-host's hub class opens its status endpoint with interval 0.
    #[test]
    fn interrupt_iitv_treats_zero_as_one() {
        assert_eq!(interrupt_iitv(0, true), 0);
        assert_eq!(interrupt_iitv(0, false), 0);
    }

    /// An interrupt endpoint carried on a bulk pipe at its full size (a Digitakt II's
    /// 1012-byte blocks on pipe 1-2) gets no short packet, so SHTNAK never stops the pipe.
    /// The completion must set PID to NAK itself, or the controller keeps polling and
    /// discards every block that arrives before the next read.
    #[test]
    fn a_full_size_interrupt_on_bulk_in_transfer_leaves_the_pipe_at_nak() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        test::poke16(usb(0).devadd(1).addr(), DEVADD_USBSPD_HS);
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 1012, 1)
            .unwrap();
        h.edpt_open(1, 0x81, EndpointType::Bulk, 512, 0).unwrap();
        for (ep, len) in [(0x83u8, 1012u16), (0x81, 512)] {
            let pipe = h.ep_to_pipe(1, ep).unwrap();
            let mut buf = [0u8; 1024];
            let u = usb(0);
            pipectr(u, pipe).write_value(PIPECTR_PID_BUF);
            u.pipesel().write_value(pipe as u16);
            u.pipemaxp().write_value((1 << PIPEMAXP_DEVSEL_SHIFT) | len);
            u.pipesel().write_value(0);
            critical_section::with(|cs| unsafe {
                let x = &mut *HCD_XFER[pipe].borrow(cs).get();
                x.buf = buf.as_mut_ptr();
                x.length = len;
                x.remaining = len;
            });
            test::poke16(u.d0fifoctr().addr(), FIFOCTR_FRDY | len);
            unsafe { pipe_brdy_in(u, pipe) };
            assert_ne!(
                HCD_PIPE_DONE.load(Ordering::Acquire) & (1 << pipe),
                0,
                "EP {ep:#x}: transfer completes"
            );
            let pid = pipectr(u, pipe).read() & PIPECTR_PID_MASK;
            if ep == 0x83 {
                assert_eq!(
                    pid, PIPECTR_PID_NAK,
                    "interrupt-on-bulk stops after one block"
                );
            } else {
                assert_eq!(pid, PIPECTR_PID_BUF, "an ordinary bulk pipe is unchanged");
            }
        }
    }

    fn dma_host() -> Rusb1HostDriver<'static> {
        use crate::mmio::test;
        let h = host_with_bus(0b00, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        critical_section::with(|cs| unsafe { *HCD_ALLOC[0].borrow(cs).get() = HcdAlloc::new() });
        reset_rx_dma(0);
        test::poke16(usb(0).devadd(1).addr(), DEVADD_USBSPD_HS);
        h.enable_rx_dma(unsafe { crate::dmac::Dma::steal::<crate::peripherals::DMA_CH1>() });
        h
    }

    #[test]
    fn enabling_rx_dma_routes_usb0_d1fifo_to_the_channel() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let _h = dma_host();
        let chcfg = crate::pac::DMAC.ch(1).chcfg().read();
        let want = crate::dmac::CHCFG_SAD
            | crate::dmac::CHCFG_SDS_32BIT
            | crate::dmac::CHCFG_DDS_32BIT
            | crate::dmac::CHCFG_AM_BURST
            | crate::dmac::CHCFG_LVL
            | crate::dmac::CHCFG_HIEN
            | 1;
        assert_eq!(chcfg, want);
        assert_eq!(
            crate::pac::DMAC.dmars(0).read() >> 16,
            0x87,
            "channel 1 is DMARS0's odd half"
        );
        assert_eq!(
            crate::pac::DMAC.ch(1).n0sa().read(),
            usb(0).d1fifo().addr() as u32
        );
    }

    #[test]
    fn an_interrupt_on_bulk_in_pipe_takes_d1fifo_dma() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        let pipe = h.ep_to_pipe(1, 0x83).unwrap() as u16;
        let u = usb(0);
        assert_eq!(
            u.d1fifosel().read(),
            pipe | (MBW_32 << FIFOSEL_MBW_SHIFT) | FIFOSEL_DCLRM | FIFOSEL_DREQE
        );
        assert_eq!(u.d1fbcfg().read(), 0, "cycle steal, no TENDE");
    }

    #[test]
    fn only_one_pipe_takes_the_dma() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        h.edpt_open(2, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        h.edpt_open(1, 0x81, EndpointType::Bulk, 512, 0).unwrap();
        let first = h.ep_to_pipe(1, 0x83).unwrap() as u16;
        assert_eq!(usb(0).d1fifosel().read() & FIFOSEL_CURPIPE_MASK, first);
    }

    #[test]
    fn closing_the_dma_pipe_releases_d1fifo() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        h.edpt_close(1, 0x83);
        assert_eq!(usb(0).d1fifosel().read(), 0);
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        assert_ne!(
            usb(0).d1fifosel().read() & FIFOSEL_DREQE,
            0,
            "the next pipe can claim it"
        );
    }

    /// BRDY on the DMA pipe reads nothing: it records DTLN, holds the pipe at NAK and completes.
    #[test]
    fn brdy_on_the_dma_pipe_records_the_length_without_reading() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        let pipe = h.ep_to_pipe(1, 0x83).unwrap();
        let u = usb(0);
        let mut buf = [0u8; 368];
        pipectr(u, pipe).write_value(PIPECTR_PID_BUF);
        critical_section::with(|cs| unsafe {
            let x = &mut *HCD_XFER[pipe].borrow(cs).get();
            x.buf = buf.as_mut_ptr();
            x.length = 368;
            x.remaining = 368;
        });
        test::poke16(u.d1fifoctr().addr(), FIFOCTR_FRDY | 368);
        let mark = test::log().len();
        unsafe { pipe_brdy_in(u, pipe) };
        let fifos = [u.d1fifo().addr(), u.d0fifo().addr()];
        assert!(
            !test::log()[mark..]
                .iter()
                .any(|a| fifos.iter().any(|f| (*f..*f + 4).contains(&a.addr))),
            "the handler must not touch the FIFO data port"
        );
        let remaining =
            critical_section::with(|cs| unsafe { (*HCD_XFER[pipe].borrow(cs).get()).remaining });
        assert_eq!(remaining, 0);
        assert_ne!(HCD_PIPE_DONE.load(Ordering::Acquire) & (1 << pipe), 0);
        assert_eq!(pipectr(u, pipe).read() & PIPECTR_PID_MASK, PIPECTR_PID_NAK);
    }

    /// The drain DMAs DTLN bytes into the bounce buffer, then copies them out.
    #[test]
    fn the_drain_copies_what_the_dma_moved() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        // The DMA "ran": the bounce buffer holds the block, and CHSTAT.TC is set.
        unsafe {
            let b = &mut *core::ptr::addr_of_mut!(RX_BOUNCE[0]);
            for (i, w) in b.0.iter_mut().enumerate() {
                *w = i as u32;
            }
        }
        crate::mmio::test::poke32(
            crate::pac::DMAC.ch(1).chstat().addr(),
            crate::dmac::CHSTAT_TC,
        );
        let mut out = [0u8; 368];
        let n = drive(h.dma_drain(&mut out, 368), 10).unwrap().unwrap();
        assert_eq!(n, 368);
        assert_eq!(&out[..8], &[0, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(crate::pac::DMAC.ch(1).n0tb().read(), 368);
    }

    #[test]
    fn a_misaligned_length_is_dropped_not_dmaed() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        let mut out = [0u8; 368];
        assert_eq!(drive(h.dma_drain(&mut out, 7), 10).unwrap(), Ok(0));
        assert_ne!(usb(0).d1fifoctr().read() & FIFOCTR_BCLR, 0);
    }

    #[test]
    fn a_zero_length_packet_needs_no_dma() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        let mut out = [0u8; 368];
        assert_eq!(drive(h.dma_drain(&mut out, 0), 10).unwrap(), Ok(0));
    }

    #[test]
    fn a_dma_that_never_ends_times_out() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        let mut out = [0u8; 368];
        assert_eq!(
            drive(h.dma_drain(&mut out, 368), 50).unwrap(),
            Err(PipeError::Timeout)
        );
    }

    /// Review C1: a block that arrives with no read pending stays in D1FIFO with its length
    /// recorded, and the next read drains it at once instead of waiting for a BRDY that the
    /// controller won't raise again.
    #[test]
    fn a_block_that_arrives_between_reads_is_drained_by_the_next_read() {
        use crate::mmio::test;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        let pipe = h.ep_to_pipe(1, 0x83).unwrap();
        let u = usb(0);
        critical_section::with(|cs| unsafe {
            (*HCD_XFER[pipe].borrow(cs).get()).buf = core::ptr::null_mut()
        });
        test::poke16(u.d1fifoctr().addr(), FIFOCTR_FRDY | 368);
        unsafe { pipe_brdy_in(u, pipe) };
        test::poke32(
            crate::pac::DMAC.ch(1).chstat().addr(),
            crate::dmac::CHSTAT_TC,
        );
        let mut out = [0u8; 368];
        let n = drive(h.xfer_in(1, 0x83, &mut out), 10)
            .expect("the read must not wait for another BRDY");
        assert_eq!(n, Ok(368));
    }

    /// Review I1: dropping a drain mid-transfer halts the DMA, clears the FIFO and keeps the
    /// channel, so the next block still uses DMA.
    #[test]
    fn a_cancelled_drain_keeps_the_channel() {
        use core::future::Future;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        let mut out = [0u8; 368];
        {
            let mut drain = core::pin::pin!(h.dma_drain(&mut out, 368));
            let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
            assert!(drain.as_mut().poll(&mut cx).is_pending());
        }
        assert!(
            critical_section::with(|cs| RX_DMA[0].borrow(cs).borrow().is_some()),
            "channel kept"
        );
        assert_ne!(
            usb(0).d1fifoctr().read() & FIFOCTR_BCLR,
            0,
            "the half-read block is cleared"
        );
    }

    #[test]
    fn the_drain_targets_the_bounce_buffer() {
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = dma_host();
        h.edpt_open(1, 0x83, EndpointType::Interrupt, 368, 1)
            .unwrap();
        crate::mmio::test::poke32(
            crate::pac::DMAC.ch(1).chstat().addr(),
            crate::dmac::CHSTAT_TC,
        );
        let mut out = [0u8; 368];
        drive(h.dma_drain(&mut out, 368), 10).unwrap().unwrap();
        let bounce = crate::uncached(unsafe { core::ptr::addr_of_mut!(RX_BOUNCE[0]) }) as u32;
        assert_eq!(crate::pac::DMAC.ch(1).n0da().read(), bounce);
    }

    /// Review I2: a bus reset whose future is dropped mid-reset (the supervisor raced it
    /// against a host-audio mode change) still ends USBRST and re-arms attach/detach
    /// detection, or the port would never report an unplug again.
    #[test]
    fn a_cancelled_bus_reset_ends_the_reset_and_rearms_detection() {
        use core::future::Future;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let h = host_with_bus(0b01, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        let u = usb(0);
        u.intenb(1).write_value(INTENB1_ATTCHE | INTENB1_DTCHE);
        {
            let mut reset = core::pin::pin!(h.bus_reset());
            let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
            assert!(
                reset.as_mut().poll(&mut cx).is_pending(),
                "inside the 20 ms reset"
            );
            assert_ne!(u.dvstctr0().read() & DVSTCTR0_USBRST, 0);
        }
        assert_eq!(u.dvstctr0().read() & DVSTCTR0_USBRST, 0, "USBRST released");
        assert_eq!(
            u.intenb(1).read() & (INTENB1_ATTCHE | INTENB1_DTCHE),
            INTENB1_ATTCHE | INTENB1_DTCHE,
            "detection re-armed"
        );
    }

    /// Review I2: an event wait cancelled while it resets an attached device leaves the attach
    /// pending, so the next wait handles it.
    #[test]
    fn a_cancelled_attach_wait_leaves_the_attach_pending() {
        use core::future::Future;
        let _g = ATTACH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut h = host_with_bus(0b01, DVSTCTR0_RHST_HS | DVSTCTR0_UACT);
        PROBE_PENDING[0].store(false, Ordering::Release);
        HCD_EVENTS.store(EVT_ATTACH, Ordering::Release);
        {
            let mut wait = core::pin::pin!(UsbHostController::wait_for_device_event(&mut h));
            let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
            assert!(wait.as_mut().poll(&mut cx).is_pending(), "inside the reset");
        }
        assert_ne!(HCD_EVENTS.load(Ordering::Acquire) & EVT_ATTACH, 0);
    }
}
