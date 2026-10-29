//! RUSB1 USB device-mode driver — implements `embassy_usb_driver::Driver`.
//!
//! ## Architecture
//!
//! ```text
//! embassy-usb ──┐
//!               │  Driver<'a> trait
//!               ▼
//!          Rusb1Driver
//!         /          \
//!   Rusb1Bus    Rusb1ControlPipe
//!         \          /
//!        dcd_int_handler   ← GIC IRQ 73/74
//!              |
//!         pipe::{pipe_xfer_in_start, pipe_xfer_out_brdy, pipe_xfer_in_bemp}
//!              |
//!         fifo::{hw_to_sw_fifo, sw_to_hw_fifo}
//! ```
//!
//! ## Interrupt handling
//!
//! The ISR `dcd_int_handler` reads INTSTS0 / BRDYSTS / BEMPSTS each time
//! hardware fires the **GIC** interrupt and:
//!
//! - **VBINT** (VBUS change): queues a `BUS_EVENT_VBUS` bit.
//! - **DVST** (device-state change): queues reset / suspend / resume events.
//! - **CTRT** (control-transfer ready): records CTSQ and queues event.
//! - **BRDY**: for OUT pipes, calls `pipe_xfer_out_brdy`; for pipe 0, wakes
//!   control write data-stage waiters.
//! - **BEMP**: for IN pipes, calls `pipe_xfer_in_bemp` to queue the next packet
//!   or complete the transfer; pipe 0 handles control-IN completion.
//!
//! ## Register-interrupt caveat (RZ/A1L §28.3.8)
//!
//! `INTENB0`, `BEMPENB`, and `BRDYENB` must not be written before VBUS rises.
//! This driver enables them inside `Rusb1Bus::enable()` which is called after
//! `Driver::start()` once embassy-usb has confirmed VBUS.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, Ordering};
use core::task::Poll;

use embassy_sync::waitqueue::AtomicWaker;
use embassy_time::Timer;
use embassy_usb_driver::{
    Bus, ControlPipe, Direction, Driver, EndpointAddress, EndpointAllocError, EndpointError,
    EndpointIn, EndpointInfo, EndpointOut, EndpointType, Event, Unsupported,
};

use super::fifo::{
    FifoPort, fifo_bclr, fifo_bval, fifo_dtln, fifo_is_ready, fifo_select_pipe, hw_to_sw_fifo,
    sw_to_hw_fifo,
};
use super::pipe::{
    BufAllocator, PIPE_COUNT, PIPE_DONE, PIPE_IS_IN, PIPE_NRDY, PIPE_WAKERS, PIPE_XFER, PipeConfig,
    XferType, pipe_bemp_disable, pipe_bemp_enable, pipe_brdy_disable,
    pipe_brdy_enable, pipe_configure, pipe_disable, pipe_enable, pipe_iso_in_activate, pipe_reset,
    pipe_stall, pipe_xfer_in_bemp, pipe_xfer_in_brdy, pipe_xfer_in_start, pipe_xfer_out_brdy,
};
use super::regs::{
    BUSWAIT_VALUE, DCPMAXP_MXPS_MASK, DVSQ_DEFAULT, DVSQ_SUSP0, DVSTCTR0_RHST, DVSTCTR0_RHST_FS,
    DVSTCTR0_RHST_HS, FIFOCTR_BCLR, INTENB0_BEMPE, INTENB0_BRDYE, INTENB0_CTRE, INTENB0_DVSE,
    INTENB0_RSME, INTENB0_VBSE, INTSTS0_BEMP, INTSTS0_BRDY, INTSTS0_CTRT, INTSTS0_CTSQ_MASK,
    INTSTS0_DVSQ_MASK, INTSTS0_DVSQ_SHIFT, INTSTS0_DVST, INTSTS0_RESM, INTSTS0_VALID,
    INTSTS0_VBINT, INTSTS0_VBSTS, PIPECTR_ACLRM, PIPECTR_CCPL, PIPECTR_PID_BUF, PIPECTR_PID_MASK,
    PIPECTR_PID_STALL10, PIPECTR_PID_STALL11, PIPECTR_SQCLR, PKT_BUF_BLOCK_SIZE, SUSPMODE_SUSPM,
    SYSCFG_DCFM, SYSCFG_DPRPU, SYSCFG_DRPD, SYSCFG_HSE, SYSCFG_UPLLE, SYSCFG_USBE, pipectr, usb,
};

// ---------------------------------------------------------------------------
// Global ISR-facing state
// ---------------------------------------------------------------------------

/// Bus-level events queued by the ISR and consumed by [`Rusb1Bus::poll`].
/// Bit layout (per port, index 0 = port 0):
///
/// - bit 0: VBUS rose
/// - bit 1: VBUS fell
/// - bit 2: USB bus reset
/// - bit 3: suspend
/// - bit 4: resume
static BUS_EVENTS: [AtomicU16; 2] = [AtomicU16::new(0), AtomicU16::new(0)];

/// Waker for [`Rusb1Bus::poll`] per port.
static BUS_WAKERS: [AtomicWaker; 2] = [AtomicWaker::new(), AtomicWaker::new()];

const BUS_EVT_VBUS_ON: u16 = 1 << 0;
const BUS_EVT_VBUS_OFF: u16 = 1 << 1;
const BUS_EVT_RESET: u16 = 1 << 2;
const BUS_EVT_SUSPEND: u16 = 1 << 3;
const BUS_EVT_RESUME: u16 = 1 << 4;

/// Pending CTSQ value delivered by the last CTRT interrupt (per port).
static CTRL_STAGE: [AtomicU8; 2] = [AtomicU8::new(0xFF), AtomicU8::new(0xFF)];

/// Waker for [`Rusb1ControlPipe::setup`] and [`Rusb1ControlPipe::data_out`] per port.
static CTRL_WAKERS: [AtomicWaker; 2] = [AtomicWaker::new(), AtomicWaker::new()];

/// Set by [`process_bus_reset`] to abort an in-flight control (pipe 0)
/// data/status stage. Unlike bulk pipes, an ep0 transfer is awaited *outside*
/// embassy-usb's `bus.poll()` select, so a host reset mid-control-transfer
/// would otherwise hang `device.run()` forever (the BRDY/BEMP it waits on never
/// arrives). `data_in`/`data_out` observe this flag and return an error, letting
/// the device task fall back to `bus.poll()` and handle the reset.
static CTRL_ABORT: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

/// Target number of max packets to buffer for a continuous-mode bulk IN pipe.
/// 4 × 512 B = 2 KiB, the largest a single pipe buffer can be (PIPEBUF BUFSIZE
/// caps at 32 × 64-byte blocks).  Lets the SIE stream up to 4 packets per
/// micro-frame between CPU refills instead of one.
const CONT_BULK_PACKETS: usize = 4;

/// Free-block headroom required before a bulk IN pipe takes a large continuous
/// buffer, so later pipes of multi-class firmwares still fit in the 8 KB RAM.
const CONT_BULK_MARGIN: usize = 24;

/// Set to 1 by the ISR when a BRDY fires on pipe 0 (control write data arrived).
/// Consumed by [`Rusb1ControlPipe::data_out`].
static PIPE0_BRDY: [AtomicU8; 2] = [AtomicU8::new(0), AtomicU8::new(0)];

/// Setup packet bytes captured from USBREQ/USBVAL/USBINDX/USBLENG per port.
static SETUP_PKT: [critical_section::Mutex<core::cell::UnsafeCell<[u8; 8]>>; 2] = {
    use core::cell::UnsafeCell;
    use critical_section::Mutex;
    #[allow(clippy::declare_interior_mutable_const)]
    const EMPTY: Mutex<UnsafeCell<[u8; 8]>> = Mutex::new(UnsafeCell::new([0u8; 8]));
    [EMPTY; 2]
};

// Sentinel meaning "no pending CTSQ".
const CTRL_STAGE_NONE: u8 = 0xFF;

// ---------------------------------------------------------------------------
// Per-port allocation bookkeeping (guarded by critical section)
// ---------------------------------------------------------------------------

struct PortAlloc {
    /// Which pipes are allocated: bit N = pipe N used (pipe 0 = DCP always used).
    pipes_used: u16,
    /// Endpoint → pipe mapping.  Index = ep_num, value = pipe number (0 = unused).
    ep_to_pipe: [[u8; 16]; 2], // [0]=OUT map, [1]=IN map
    /// Packet buffer block allocator.
    buf: BufAllocator,
    /// Pipe configuration (for re-configure on reset).
    pipe_cfg: [Option<PipeConfig>; PIPE_COUNT],
}

impl PortAlloc {
    const fn new() -> Self {
        Self {
            pipes_used: 0x0001, // pipe 0 pre-allocated
            ep_to_pipe: [[0u8; 16]; 2],
            buf: BufAllocator::new(),
            pipe_cfg: [None; PIPE_COUNT],
        }
    }

    /// Free `pipe`, its packet-buffer reservation and its configuration.
    fn release(&mut self, pipe: usize) {
        if let Some(c) = self.pipe_cfg[pipe].take() {
            let banks = if c.double_buf { 2 } else { 1 };
            self.buf.free(c.buf_start, c.buf_blocks as usize * banks);
        }
        self.pipes_used &= !(1u16 << pipe);
    }

    fn alloc_pipe(&mut self, xfer_type: XferType, wide: bool) -> Option<usize> {
        // RZ/A1 RUSB pipe transfer-type constraints are fixed in hardware
        // (TRM §28.4.7/§28.4.8; TinyUSB dcd_rusb1.c allocates the same way):
        //   PIPE1-2  : isochronous or bulk — reserved here for ISO audio.
        //   PIPE3-5  : bulk only.
        //   PIPE6-8  : interrupt only (64-byte fixed single buffer).
        //   PIPE9-15 : bulk (function-controller mode).
        // Bulk endpoints MUST skip pipes 6-8: assigning a bulk transfer to an
        // interrupt-only pipe silently never raises BRDY, and (because all
        // bulk/interrupt pipes share the D1FIFO port) it stalls the other
        // bulk pipes too.
        let (start, end) = match xfer_type {
            XferType::Control => return Some(0),
            XferType::Isochronous => (1, 3),  // pipes 1-2
            XferType::Bulk if wide => (1, 3), // over 512 bytes: only pipes 1-2 take it
            XferType::Bulk => (3, 16),        // pipes 3-5, 9-15 (6-8 skipped below)
            XferType::Interrupt => (6, 9),    // pipes 6-8
        };
        for n in start..end {
            // Pipes 6-8 are interrupt-only; never allocate them for bulk.
            if xfer_type == XferType::Bulk && (6..=8).contains(&n) {
                continue;
            }
            if self.pipes_used & (1 << n) == 0 {
                self.pipes_used |= 1 << n;
                return Some(n);
            }
        }
        None
    }
}

static PORT_ALLOC: [critical_section::Mutex<core::cell::UnsafeCell<PortAlloc>>; 2] = {
    use core::cell::UnsafeCell;
    use critical_section::Mutex;
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Mutex<UnsafeCell<PortAlloc>> = Mutex::new(UnsafeCell::new(PortAlloc::new()));
    [INIT; 2]
};

// ---------------------------------------------------------------------------
// Rusb1Driver
// ---------------------------------------------------------------------------

/// The top-level USB device-mode driver for one RUSB1 port (0 or 1).
///
/// It borrows its port for `'d` ([`init_device_mode`](super::init_device_mode)), so only one
/// exists per port at a time.
pub struct Rusb1Driver<'d> {
    port: u8,
    _usb: core::marker::PhantomData<&'d mut ()>,
}

impl<'d> Rusb1Driver<'d> {
    /// Create a driver for `port` (0 or 1).
    ///
    /// # Safety
    /// Caller must ensure this is the only active driver for this port, and
    /// that `crate::usb::module_clock_enable(port)` has already been called.
    pub(crate) unsafe fn new(port: u8) -> Self {
        unsafe {
            debug_assert!(port <= 1);
            // A fresh device-mode bring-up re-enumerates from scratch and the
            // embassy-usb Builder re-allocates every endpoint.  Reset this
            // port's pipe / packet-buffer allocator so repeated bring-ups
            // don't leak pipes until `alloc_endpoint` fails.
            critical_section::with(|cs| {
                let alloc = &mut *PORT_ALLOC[port as usize].borrow(cs).get();
                *alloc = PortAlloc::new();
            });
            // A re-attach after `disconnect` (switching device faces) starts from a clean
            // slate: no bus event or control stage left from the previous session.
            let p = port as usize;
            BUS_EVENTS[p].store(0, Ordering::Release);
            CTRL_STAGE[p].store(CTRL_STAGE_NONE, Ordering::Release);
            CTRL_ABORT[p].store(false, Ordering::Release);
            PIPE0_BRDY[p].store(0, Ordering::Release);
            // Set BUSWAIT early so register accesses after USBE=1 are safe.
            let u = usb(port);
            u.buswait().write_value(BUSWAIT_VALUE);
            Self {
                port,
                _usb: core::marker::PhantomData,
            }
        }
    }
}

impl<'d> Driver<'d> for Rusb1Driver<'d> {
    type EndpointOut = Rusb1EndpointOut;
    type EndpointIn = Rusb1EndpointIn;
    type ControlPipe = Rusb1ControlPipe;
    type Bus = Rusb1Bus;

    fn alloc_endpoint_out(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        _interval_ms: u8,
    ) -> Result<Self::EndpointOut, EndpointAllocError> {
        let (xfer_type, mxps, wide) = device_layout(ep_type, max_packet_size);
        critical_section::with(|cs| {
            let alloc = unsafe { &mut *PORT_ALLOC[self.port as usize].borrow(cs).get() };

            // If a specific address is requested and it's already allocated to a pipe,
            // reuse that pipe for this alternate setting (same endpoint, different MPS).
            // This matches the UAC2 pattern of sharing an endpoint across alt settings
            // (e.g. alt1=16-bit, alt2=24-bit on the same ISO OUT endpoint).
            if let Some(addr) = ep_addr {
                let ep_num = addr.index() as u8;
                let existing_pipe = alloc.ep_to_pipe[0][ep_num as usize] as usize;
                // A shared endpoint whose larger alt setting needs another kind of pipe
                // (Overbridge's 32-byte alt 1 on an interrupt pipe, then 88 bytes) moves:
                // release the old pipe and allocate afresh below. Earlier alts' endpoint
                // handles keep the old pipe number; a class streams through the handle from
                // its largest alt setting.
                let fits = existing_pipe != 0
                    && alloc.pipe_cfg[existing_pipe]
                        .as_ref()
                        .is_some_and(|c| c.xfer_type == xfer_type && (!wide || existing_pipe <= 2));
                if existing_pipe != 0 && !fits {
                    alloc.release(existing_pipe);
                    alloc.ep_to_pipe[0][ep_num as usize] = 0;
                }
                if fits {
                    // Keep the larger MPS so the hardware buffer covers both alt
                    // settings.  Growing in place would overrun the next pipe's
                    // blocks, so free the old reservation and reserve the larger
                    // one.  The scalar fields are copied out first to release the
                    // pipe_cfg borrow before touching alloc.buf.
                    let cur = alloc.pipe_cfg[existing_pipe]
                        .as_ref()
                        .map(|c| (c.mps, c.double_buf, c.buf_start, c.buf_blocks));
                    if let Some((cur_mps, dbl, cur_start, cur_blocks)) = cur
                        && mxps > cur_mps
                    {
                        let mult = if dbl { 2 } else { 1 };
                        let new_blocks = mps_to_blocks(mxps);
                        alloc.buf.free(cur_start, cur_blocks as usize * mult);
                        let new_start = alloc
                            .buf
                            .alloc(new_blocks * mult)
                            .ok_or(EndpointAllocError)?;
                        if let Some(c) = alloc.pipe_cfg[existing_pipe].as_mut() {
                            c.mps = mxps;
                            c.buf_blocks = new_blocks as u8;
                            c.buf_start = new_start;
                        }
                    }
                    let info = EndpointInfo {
                        addr: EndpointAddress::from_parts(ep_num as usize, Direction::Out),
                        ep_type,
                        max_packet_size,
                        interval_ms: _interval_ms,
                    };
                    return Ok(Rusb1EndpointOut {
                        port: self.port,
                        pipe: existing_pipe as u8,
                        info,
                    });
                }
            }

            let pipe = alloc
                .alloc_pipe(xfer_type, wide)
                .ok_or(EndpointAllocError)?;
            let buf_blocks = mps_to_blocks(mxps);
            // ISO pipes are hardware double-buffered (PIPECFG.DBLB), so the SIE
            // uses TWO consecutive banks of `buf_blocks` each.  Reserve both, or
            // the second bank silently overlaps the next pipe's buffer — and a
            // bulk pipe whose PIPEBUF overlaps an active ISO pipe never gets a
            // free buffer from the SIE and NAKs forever.
            let double_buf = xfer_type == XferType::Isochronous;
            let reserve = buf_blocks * if double_buf { 2 } else { 1 };
            let buf_start = alloc.buf.alloc(reserve).ok_or(EndpointAllocError)?;

            // Use requested ep address or pick a free one.
            let ep_num = if let Some(addr) = ep_addr {
                addr.index() as u8
            } else {
                (1u8..16)
                    .find(|&e| alloc.ep_to_pipe[0][e as usize] == 0)
                    .ok_or(EndpointAllocError)?
            };

            alloc.ep_to_pipe[0][ep_num as usize] = pipe as u8;
            alloc.pipe_cfg[pipe] = Some(PipeConfig {
                ep_num,
                is_in: false,
                xfer_type,
                mps: mxps,
                buf_start,
                buf_blocks: buf_blocks as u8,
                double_buf,
                continuous: false,
            });

            let info = EndpointInfo {
                addr: EndpointAddress::from_parts(ep_num as usize, Direction::Out),
                ep_type,
                max_packet_size,
                interval_ms: _interval_ms,
            };
            Ok(Rusb1EndpointOut {
                port: self.port,
                pipe: pipe as u8,
                info,
            })
        })
    }

    fn alloc_endpoint_in(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        _interval_ms: u8,
    ) -> Result<Self::EndpointIn, EndpointAllocError> {
        let (xfer_type, mxps, wide) = device_layout(ep_type, max_packet_size);
        critical_section::with(|cs| {
            let alloc = unsafe { &mut *PORT_ALLOC[self.port as usize].borrow(cs).get() };

            // Reuse existing pipe if this ep_addr is already allocated (shared-endpoint alt setting).
            if let Some(addr) = ep_addr {
                let ep_num = addr.index() as u8;
                let existing_pipe = alloc.ep_to_pipe[1][ep_num as usize] as usize;
                // A shared endpoint whose larger alt setting needs another kind of pipe
                // (Overbridge's 32-byte alt 1 on an interrupt pipe, then 88 bytes) moves:
                // release the old pipe and allocate afresh below. Earlier alts' endpoint
                // handles keep the old pipe number; a class streams through the handle from
                // its largest alt setting.
                let fits = existing_pipe != 0
                    && alloc.pipe_cfg[existing_pipe]
                        .as_ref()
                        .is_some_and(|c| c.xfer_type == xfer_type && (!wide || existing_pipe <= 2));
                if existing_pipe != 0 && !fits {
                    alloc.release(existing_pipe);
                    alloc.ep_to_pipe[1][ep_num as usize] = 0;
                }
                if fits {
                    // Grow the shared buffer to cover the larger alt setting by
                    // re-reserving (see alloc_endpoint_out for the rationale — an
                    // in-place grow would overrun the next pipe's blocks).
                    let cur = alloc.pipe_cfg[existing_pipe]
                        .as_ref()
                        .map(|c| (c.mps, c.double_buf, c.buf_start, c.buf_blocks));
                    if let Some((cur_mps, dbl, cur_start, cur_blocks)) = cur
                        && mxps > cur_mps
                    {
                        let mult = if dbl { 2 } else { 1 };
                        let new_blocks = mps_to_blocks(mxps);
                        alloc.buf.free(cur_start, cur_blocks as usize * mult);
                        let new_start = alloc
                            .buf
                            .alloc(new_blocks * mult)
                            .ok_or(EndpointAllocError)?;
                        if let Some(c) = alloc.pipe_cfg[existing_pipe].as_mut() {
                            c.mps = mxps;
                            c.buf_blocks = new_blocks as u8;
                            c.buf_start = new_start;
                        }
                    }
                    let info = EndpointInfo {
                        addr: EndpointAddress::from_parts(ep_num as usize, Direction::In),
                        ep_type,
                        max_packet_size,
                        interval_ms: _interval_ms,
                    };
                    return Ok(Rusb1EndpointIn {
                        port: self.port,
                        pipe: existing_pipe as u8,
                        info,
                        buf_bytes: max_packet_size,
                    });
                }
            }

            let pipe = alloc
                .alloc_pipe(xfer_type, wide)
                .ok_or(EndpointAllocError)?;
            let mps_blocks = mps_to_blocks(mxps);
            let double_buf = xfer_type == XferType::Isochronous
                || (xfer_type == XferType::Bulk && ep_type == EndpointType::Interrupt);

            // For a bulk IN pipe on a buffer-capable pipe (1-5), try to grab a
            // multi-packet buffer and run it in continuous transfer mode (CNTMD)
            // so the SIE streams several packets per micro-frame instead of one.
            // Adaptive: fall back to fewer packets — and finally to a single
            // packet — if the 8 KB packet RAM is tight, so multi-class firmwares
            // still fit.  BUFSIZE maxes at 32 blocks (2 KiB = 4 × 512 B packets).
            let mut continuous = false;
            let mut buf_blocks = mps_blocks;
            let mut buf_start_opt = None;
            if ep_type == EndpointType::Bulk && (1..=5).contains(&pipe) {
                for mult in [CONT_BULK_PACKETS, 2] {
                    let want = mps_blocks * mult;
                    // Only grab a large buffer if it leaves headroom for the
                    // remaining pipes of multi-class firmwares (ISO+CDC+MIDI),
                    // so their later allocations don't fail.  A lone-MSC config
                    // has plenty free and always qualifies.
                    if alloc.buf.free_blocks() < want + CONT_BULK_MARGIN {
                        continue;
                    }
                    if let Some(s) = alloc.buf.alloc(want) {
                        buf_blocks = want;
                        continuous = true;
                        buf_start_opt = Some(s);
                        break;
                    }
                }
            }
            let buf_start = match buf_start_opt {
                Some(s) => s,
                None => {
                    // Reserve both banks for double-buffered (ISO) pipes.
                    let reserve = mps_blocks * if double_buf { 2 } else { 1 };
                    alloc.buf.alloc(reserve).ok_or(EndpointAllocError)?
                }
            };
            // One fill stages the whole buffer for a continuous pipe; a single
            // max packet otherwise.
            let buf_bytes = if continuous {
                buf_blocks as u16 * PKT_BUF_BLOCK_SIZE as u16
            } else {
                max_packet_size
            };

            let ep_num = if let Some(addr) = ep_addr {
                addr.index() as u8
            } else {
                (1u8..16)
                    .find(|&e| alloc.ep_to_pipe[1][e as usize] == 0)
                    .ok_or(EndpointAllocError)?
            };

            alloc.ep_to_pipe[1][ep_num as usize] = pipe as u8;
            alloc.pipe_cfg[pipe] = Some(PipeConfig {
                ep_num,
                is_in: true,
                xfer_type,
                mps: mxps,
                buf_start,
                buf_blocks: buf_blocks as u8,
                double_buf,
                continuous,
            });

            let info = EndpointInfo {
                addr: EndpointAddress::from_parts(ep_num as usize, Direction::In),
                ep_type,
                max_packet_size,
                interval_ms: _interval_ms,
            };
            Ok(Rusb1EndpointIn {
                port: self.port,
                pipe: pipe as u8,
                info,
                buf_bytes,
            })
        })
    }

    fn start(self, control_max_packet_size: u16) -> (Self::Bus, Self::ControlPipe) {
        debug!(
            "usb{}: start (ctrl mps={})",
            self.port,
            control_max_packet_size
        );
        unsafe {
            let u = usb(self.port);

            // ── PLL / clock init (TRM §28.4.1 (5)) ─────────────────────────
            // Stop clock supply to this port while touching the PLL.
            u.suspmode().modify(|r| *r &= !SUSPMODE_SUSPM);
            trace!("usb{}: SUSPMODE cleared for PLL init", self.port);

            // UPLLE lives in USB0's SYSCFG0 regardless of which port is being
            // initialised, and may only be modified while SUSPM=0 on BOTH
            // channels (TRM §28.3.1).
            // Only enable it when it is currently off: a set UPLLE means the PLL
            // is already locked from the other port's bring-up, so we must
            // neither repeat the lock wait nor disturb the other channel's SUSPM.
            // When it is off, the PLL is down and neither channel can be running,
            // so clearing the other channel's SUSPM is safe.
            let u0 = usb(0);
            if u0.syscfg0().read() & SYSCFG_UPLLE == 0 {
                let other = usb(if self.port == 0 { 1 } else { 0 });
                other.suspmode().modify(|r| *r &= !SUSPMODE_SUSPM);
                u0.syscfg0().modify(|r| *r |= SYSCFG_UPLLE);

                // Wait >= 1 ms for PLL lock (at ~400 MHz ≈ 400 K spin iterations).
                for _ in 0u32..400_000 {
                    core::hint::spin_loop();
                }
            }

            // CPU-bus wait cycles and clock re-enable.
            u.buswait().write_value(BUSWAIT_VALUE);
            u.suspmode().modify(|r| *r |= SUSPMODE_SUSPM);

            // ── Mode / PHY config ────────────────────────────────────────────
            // HSE=1: module auto-negotiates HS or FS during the reset handshake.
            // Must be set while DPRPU=0 (TRM §28.3 SYSCFG.HSE).
            u.syscfg0().modify(|r| *r |= SYSCFG_HSE);

            // DCFM=0 (function/device mode), DRPD=0 (no pull-downs in device mode).
            u.syscfg0().modify(|r| *r &= !(SYSCFG_DCFM | SYSCFG_DRPD));

            // Enable USB module.
            u.syscfg0().modify(|r| *r |= SYSCFG_USBE);

            // Leave the D+ pull-up OFF here.  Per USB 2.0 §7.1.5 a device —
            // especially a self-powered one like the Deluge — must not assert
            // its pull-up until VBUS is present.  Strict USB-C hosts (e.g.
            // Apple-Silicon Macs) never register a clean attach from a device
            // that connects before the port is powered, and power-cycle the port
            // indefinitely.  The connect (DPRPU=1) happens in `enable()`, which
            // the device stack calls only after VBUS is detected
            // (Bus::poll → PowerDetected).
            //
            // Clearing DPRPU also drives SE0 immediately, so a warm reset that
            // left DPRPU=1 presents a clean disconnect and the host re-enumerates
            // from scratch, discarding stale pipe FIFO contents.
            u.syscfg0().modify(|r| *r &= !SYSCFG_DPRPU);
            debug!(
                "usb{}: SYSCFG0={:#06x} (DPRPU off — connect deferred to VBUS/enable)",
                self.port,
                u.syscfg0().read()
            );

            // DCPMAXP (control endpoint max packet size).
            u.dcpmaxp()
                .write_value(control_max_packet_size & DCPMAXP_MXPS_MASK);

            // Enable VBUS interrupt so Bus::poll() can detect connection.
            u.intenb(0).write_value(INTENB0_VBSE);

            // Register the ISR with the GIC.
            crate::usb::int_enable(self.port);

            // If VBUS is already asserted (cable was plugged in before init),
            // the rising-edge VBINT will never fire.  Synthesise the event now
            // so that Bus::poll() doesn't wait forever.
            let intsts = u.intsts(0).read();
            debug!(
                "usb{}: post-init INTSTS0={:#06x} VBSTS={}",
                self.port,
                intsts,
                intsts & INTSTS0_VBSTS != 0
            );
            if intsts & INTSTS0_VBSTS != 0 {
                debug!(
                    "usb{}: VBUS already present — synthesising PowerDetected",
                    self.port
                );
                BUS_EVENTS[self.port as usize].fetch_or(BUS_EVT_VBUS_ON, Ordering::Release);
                BUS_WAKERS[self.port as usize].wake();
            }
        }
        (
            Rusb1Bus {
                port: self.port,
                connected: false,
            },
            Rusb1ControlPipe {
                port: self.port,
                mps: control_max_packet_size,
            },
        )
    }
}

// ---------------------------------------------------------------------------
// Rusb1Bus
// ---------------------------------------------------------------------------

/// SETUP packets port n has received: each starts a new control request.
static SETUP_GEN: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];

/// Per port and pipe: the `SETUP_GEN` in which `endpoint_set_enabled(.., true)` last enabled
/// the pipe (0: never, or forgotten by a bus reset).
static ENABLED_GEN: [[AtomicU32; PIPE_COUNT]; 2] =
    [const { [const { AtomicU32::new(0) }; PIPE_COUNT] }; 2];

/// Forget which pipes the current request enabled (a bus reset).
fn forget_enables(port: usize) {
    for g in &ENABLED_GEN[port] {
        g.store(0, Ordering::Release);
    }
}

pub struct Rusb1Bus {
    port: u8,
    /// Whether we currently believe VBUS is present.
    connected: bool,
}

impl Bus for Rusb1Bus {
    async fn poll(&mut self) -> Event {
        core::future::poll_fn(|cx| {
            let port = self.port as usize;
            BUS_WAKERS[port].register(cx.waker());
            // Pick the highest-priority pending event and clear ONLY that bit, so
            // a SUSPEND+RESET (or RESET+RESUME) pair delivers both events.
            let ev = BUS_EVENTS[port].load(Ordering::Acquire);
            let (bit, event, name) = if ev & BUS_EVT_RESET != 0 {
                (BUS_EVT_RESET, Event::Reset, "Reset")
            } else if ev & BUS_EVT_SUSPEND != 0 {
                (BUS_EVT_SUSPEND, Event::Suspend, "Suspend")
            } else if ev & BUS_EVT_RESUME != 0 {
                (BUS_EVT_RESUME, Event::Resume, "Resume")
            } else if ev & BUS_EVT_VBUS_ON != 0 {
                self.connected = true;
                (BUS_EVT_VBUS_ON, Event::PowerDetected, "PowerDetected")
            } else if ev & BUS_EVT_VBUS_OFF != 0 {
                self.connected = false;
                (BUS_EVT_VBUS_OFF, Event::PowerRemoved, "PowerRemoved")
            } else {
                return Poll::Pending;
            };
            BUS_EVENTS[port].fetch_and(!bit, Ordering::AcqRel);
            // If other events are still pending, ensure we get polled again.
            if BUS_EVENTS[port].load(Ordering::Acquire) != 0 {
                BUS_WAKERS[port].wake();
            }
            debug!("usb{}: Bus::poll → {}", self.port, name);
            Poll::Ready(event)
        })
        .await
    }

    async fn enable(&mut self) {
        debug!(
            "usb{}: Bus::enable — waiting 30 ms for module settle",
            self.port
        );
        // RZ/A1L quirk: INTENB0, BEMPENB and BRDYENB are not writable
        // immediately after USBE=1 — the write is silently dropped.  The module
        // needs roughly 25 ms to settle; wait 30 ms before arming IRQs.
        embassy_time::Timer::after_millis(30).await;

        unsafe {
            let u = usb(self.port);
            let intenb = INTENB0_VBSE | INTENB0_DVSE | INTENB0_CTRE | INTENB0_BRDYE | INTENB0_BEMPE;

            // Write and readback-verify; retry up to 3× with 10 ms increments
            // if the register isn't ready yet.
            for attempt in 0..3u32 {
                u.intenb(0).write_value(intenb);
                // Pipe 0 BEMPENB bit must be set so data_in() receives BEMP.
                // Pipe 0 BRDYENB bit must be set for control OUT / setup data.
                u.bempenb().write_value(0x0001);
                u.brdyenb().write_value(0x0001);

                let rb = u.intenb(0).read();
                if rb == intenb {
                    debug!(
                        "usb{}: INTENB0={:#06x} verified (attempt {})",
                        self.port,
                        rb,
                        attempt
                    );
                    break;
                }
                warn!(
                    "usb{}: INTENB0 write failed attempt {} (wrote {:#06x}, read {:#06x}), retrying…",
                    self.port,
                    attempt + 1,
                    intenb,
                    rb
                );
                embassy_time::Timer::after_millis(10).await;
            }

            // Configure allocated pipes on the hardware.
            configure_all_pipes(self.port);

            // Signal connection: VBUS has been detected (enable() runs only
            // after Bus::poll → PowerDetected) and the SIE is fully armed, so
            // assert the D+ pull-up (see `start()` for why it waits until now).
            u.syscfg0().modify(|r| *r |= SYSCFG_DPRPU);
            debug!(
                "usb{}: DPRPU set — D+ pull-up active (connected), SYSCFG0={:#06x}",
                self.port,
                u.syscfg0().read()
            );
            debug!("usb{}: Bus::enable done", self.port);
        }
    }

    async fn disable(&mut self) {
        // TRM §28.3 (3): when disconnection from the USB host is recognised
        // (device mode), DPRPU and DCFM must be toggled in this exact sequence
        // before the bus can be reused (e.g. switching to host mode).
        //
        // Step 1: clear DPRPU — pull-up off; host sees SE0 (disconnect).
        let u = usb(self.port);
        u.syscfg0().modify(|r| *r &= !SYSCFG_DPRPU);
        // Step 2: wait ≥ 1 µs.
        Timer::after_micros(1).await;
        let u = usb(self.port);
        // Step 3: pulse DCFM to 1 (resets PHY data-line state machine).
        u.syscfg0().modify(|r| *r |= SYSCFG_DCFM);
        // Step 4: wait ≥ 200 ns (timer minimum is 1 µs — safe to over-wait).
        Timer::after_micros(1).await;
        let u = usb(self.port);
        // Step 5: clear DCFM back to function (device) mode.
        u.syscfg0().modify(|r| *r &= !SYSCFG_DCFM);
        // Disable all interrupts.
        u.intenb(0).write_value(0);
        u.brdyenb().write_value(0);
        u.bempenb().write_value(0);
    }

    fn endpoint_set_enabled(&mut self, ep_addr: EndpointAddress, enabled: bool) {
        let pipe = ep_addr_to_pipe(self.port, ep_addr);
        if let Some(p) = pipe {
            let port = self.port as usize;
            let generation = SETUP_GEN[port].load(Ordering::Acquire);
            // One SET_INTERFACE walks every alt's endpoints in descriptor order. Alts that share
            // an endpoint address share its pipe (UAC2 streaming alts 1 and 2), so an unselected
            // alt's disable can follow the selected alt's enable: keep the enable.
            if !enabled
                && generation != 0
                && ENABLED_GEN[port][p].load(Ordering::Acquire) == generation
            {
                debug!("usb{} P{} disable skipped: this request's selected alt enabled it", port, p);
                return;
            }
            if enabled {
                ENABLED_GEN[port][p].store(generation, Ordering::Release);
            }
            debug!(
                "usb{} P{} endpoint_set_enabled({}) ep={:#04x} dir={:?} — resets FIFO+toggle (SQCLR)",
                self.port,
                p,
                enabled,
                ep_addr.index() as u8 | ((ep_addr.direction() == Direction::In) as u8) << 7,
                ep_addr.direction(),
            );
            unsafe {
                let u = usb(self.port);
                if enabled {
                    // Clear the FIFO and reset the data toggle to DATA0 BEFORE
                    // arming the pipe.  After SET_CONFIGURATION / SET_INTERFACE
                    // the host resets its bulk/interrupt toggle to DATA0; the
                    // device must do the same or every IN packet goes out with
                    // the wrong DATAx phase and the host ACKs but discards it as
                    // a duplicate.  pipe_reset leaves PID=NAK.
                    if super::iso::in_bound(self.port, p) && ep_addr.direction() == Direction::In {
                        // Continuous ISO IN (mic/capture): activate in IFIS mode so
                        // the SIE flushes the IN buffer each microframe, generating
                        // the BRDY that drives the hook refill (as TinyUSB's
                        // dcd_edpt_iso_activate does).  Does its own reset + PID=BUF.
                        pipe_iso_in_activate(u, p);
                    } else {
                        pipe_reset(u, p); // leaves PID=NAK
                        pipe_enable(u, p);
                        if ep_addr.direction() == Direction::Out {
                            pipe_brdy_enable(u, p);
                        }
                        // Other IN pipes arm their completion interrupt per-transfer
                        // in `write()`, so nothing to enable here.
                    }
                } else {
                    pipe_disable(u, p);
                    pipe_bemp_disable(u, p);
                    pipe_brdy_disable(u, p);
                    // Pulse ACLRM to clear the FIFO buffer contents and the
                    // double-buffer toggle state (TRM §28.4.4 Table 28.13).
                    // This prevents leftover data in the off-bank from causing
                    // a spurious BRDY when the endpoint is re-enabled.
                    // Requires PID=NAK (already set by pipe_disable above).
                    pipe_reset(u, p);
                    // Signal any task blocked in read() that the endpoint is
                    // now disabled, so it returns Err(EndpointError::Disabled)
                    // rather than blocking forever.
                    PIPE_NRDY.fetch_or(1u16 << p, Ordering::Release);
                }
            }
            // Wake any task waiting in wait_enabled() or read() so it can observe
            // the new pipe state (PID changed to BUF/NAK, or PIPE_NRDY set).
            PIPE_WAKERS[p].wake();
        }
    }

    fn endpoint_set_stalled(&mut self, ep_addr: EndpointAddress, stalled: bool) {
        let pipe = ep_addr_to_pipe(self.port, ep_addr);
        if let Some(p) = pipe {
            unsafe {
                let u = usb(self.port);
                if stalled {
                    pipe_stall(u, p);
                } else {
                    pipe_reset(u, p);
                    pipe_enable(u, p);
                }
            }
        }
    }

    fn endpoint_is_stalled(&mut self, ep_addr: EndpointAddress) -> bool {
        let pipe = ep_addr_to_pipe(self.port, ep_addr);
        if let Some(p) = pipe {
            let u = usb(self.port);
            let pid = pipectr(u, p).read() & super::regs::PIPECTR_PID_MASK;
            // The controller may sit in either STALL encoding (10 firmware-set,
            // 11 hardware-set on babble/sequence error); both mean "halted".
            pid == PIPECTR_PID_STALL10 || pid == PIPECTR_PID_STALL11
        } else {
            false
        }
    }

    async fn remote_wakeup(&mut self) -> Result<(), Unsupported> {
        Err(Unsupported)
    }
}

// ---------------------------------------------------------------------------
// Rusb1ControlPipe
// ---------------------------------------------------------------------------

pub struct Rusb1ControlPipe {
    port: u8,
    mps: u16,
}

impl ControlPipe for Rusb1ControlPipe {
    fn max_packet_size(&self) -> usize {
        self.mps as usize
    }

    async fn setup(&mut self) -> [u8; 8] {
        trace!("usb{}: setup() waiting", self.port);
        let pkt = core::future::poll_fn(|cx| {
            CTRL_WAKERS[self.port as usize].register(cx.waker());
            let stage = CTRL_STAGE[self.port as usize].swap(CTRL_STAGE_NONE, Ordering::Acquire);
            // Accept any non-idle stage: CTSQ_READ_DATA (1), CTSQ_WRITE_DATA (3),
            // and CTSQ_WRITE_ZLP (5) all indicate a new SETUP packet was received.
            // (The ISR only sets CTRL_STAGE when INTSTS0.VALID=1.)
            if stage != CTRL_STAGE_NONE {
                let pkt = critical_section::with(|cs| unsafe {
                    *SETUP_PKT[self.port as usize].borrow(cs).get()
                });
                return Poll::Ready(pkt);
            }
            Poll::Pending
        })
        .await;
        // A fresh SETUP marks the start of a new control transfer: discard any
        // stale reset-abort flag here, ONCE per transfer. Do NOT clear it in
        // data_in/data_out — those are called once per packet, so clearing there
        // would race a reset that lands between packets of a multi-packet
        // transfer (the next packet would wipe the flag, then hang on a
        // BRDY/BEMP the re-enumerating host never produces).
        CTRL_ABORT[self.port as usize].store(false, Ordering::Release);
        debug!("usb{}: setup() → {:?}", self.port, crate::fmt::Bytes(&pkt));
        pkt
    }

    async fn data_out(
        &mut self,
        buf: &mut [u8],
        _first: bool,
        _last: bool,
    ) -> Result<usize, EndpointError> {
        let p = self.port as usize;
        trace!("usb{}: data_out buf_len={}", self.port, buf.len());

        // Set PID=BUF so hardware ACKs the host's OUT token and receives the
        // data-stage payload into the CFIFO.  PID was forced to NAK by hardware
        // when the preceding SETUP packet was received (TRM §28.3.30 PID[1:0]
        // function-controller note).  Without this, every host OUT token is
        // met with NAK and no data ever arrives.
        //
        // Also re-enable BRDY0, clear any stale BRDYSTS bit 0, and discard any
        // previously latched PIPE0_BRDY signal.  Otherwise a zero-length OUT ACK
        // from the host to the preceding status stage (fired while BRDYENB was
        // still set) makes the await below return immediately with 0 bytes.
        let u = usb(self.port);
        // Clear BRDYSTS bit 0 (write-0-to-clear, all other bits kept 1).
        u.brdysts().write_value(!(1u16));
        // Re-enable BRDY0 interrupt (may have been disabled by accept()).
        u.brdyenb().modify(|r| *r |= 0x0001);
        // Discard any stale software flag set by a spurious BRDY0 above.
        PIPE0_BRDY[p].store(0, Ordering::Release);

        let ctr = pipectr(u, 0).read();
        pipectr(u, 0).write_value((ctr & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF);

        // The reset-abort flag is cleared once per transfer in setup(); a reset
        // that fires now (or between packets) must stay latched so this and any
        // subsequent packet abort rather than hang.

        // Wait for the BRDY interrupt on pipe 0 signalling that the host's
        // OUT data packet has been received and is ready to read, or for a bus
        // reset to abort the transfer.
        let aborted = core::future::poll_fn(|cx| {
            CTRL_WAKERS[p].register(cx.waker());
            if CTRL_ABORT[p].load(Ordering::Acquire) {
                return Poll::Ready(true);
            }
            if PIPE0_BRDY[p].swap(0, Ordering::Acquire) != 0 {
                Poll::Ready(false)
            } else {
                Poll::Pending
            }
        })
        .await;
        if aborted {
            return Err(EndpointError::Disabled);
        }

        // Read from CFIFO (pipe 0 OUT).
        unsafe {
            let u = usb(self.port);
            let fifo = FifoPort::cfifo(u);
            fifo_select_pipe(&fifo, 0, false);
            let vld = fifo_dtln(&fifo) as usize;
            let len = buf.len().min(vld);
            if len > 0 {
                hw_to_sw_fifo(&fifo, buf.as_mut_ptr(), len);
            }
            fifo_bclr(&fifo);
            trace!(
                "usb{}: data_out read {} bytes (fifo had {})",
                self.port,
                len,
                vld
            );
            Ok(len)
        }
    }

    async fn data_in(
        &mut self,
        data: &[u8],
        _first: bool,
        last: bool,
    ) -> Result<(), EndpointError> {
        trace!("usb{}: data_in len={} last={}", self.port, data.len(), last);
        unsafe {
            let u = usb(self.port);

            // Clear BEMPSTS bit 0, re-enable the BEMP0 interrupt, and discard any
            // stale PIPE_DONE bit 0 latched by a previous control-write
            // status-stage BEMP (fired while BEMPENB was still set before
            // accept() was called).  Otherwise the poll_fn below resolves
            // immediately, before the IN data has actually been sent.
            u.bempsts().write_value(!(1u16));
            u.bempenb().modify(|r| *r |= 0x0001);
            PIPE_DONE.fetch_and(!(1u16), Ordering::Release);
            // The reset-abort flag is cleared once per transfer in setup(); leave
            // it latched here so a reset between packets aborts rather than hangs.

            let fifo = FifoPort::cfifo(u);
            fifo_select_pipe(&fifo, 0, true); // ISEL=1 selects IN direction

            // Wait (bounded) for the ISEL bit to take effect (CFIFOSEL latency).
            let mut ready = false;
            for _ in 0..1000u32 {
                let sel = u.cfifosel().read();
                if sel & super::regs::FIFOSEL_ISEL != 0 {
                    ready = true;
                    break;
                }
            }
            trace!(
                "usb{}: CFIFO ISEL ready={} frdy={}",
                self.port,
                ready,
                fifo_is_ready(&fifo, 0)
            );
            if !ready || !fifo_is_ready(&fifo, 0) {
                warn!(
                    "usb{}: data_in CFIFO not ready — returning Disabled",
                    self.port
                );
                return Err(EndpointError::Disabled);
            }

            sw_to_hw_fifo(&fifo, data.as_ptr(), data.len());

            if last || data.len() < self.mps as usize {
                fifo_bval(&fifo);
            }

            // Arm DCP for IN transfers by setting PID=BUF (TRM §28.4.6.2(b)).
            // PID was left at NAK by hardware after the SETUP packet was received.
            // Without this the hardware NAKs every IN token from the host and
            // the data stage never completes.
            let ctr = pipectr(u, 0).read();
            pipectr(u, 0).write_value((ctr & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF);

            // Wait for BEMP on pipe 0 (data transmitted to host), or for a bus
            // reset to abort the transfer.
            let aborted = core::future::poll_fn(|cx| {
                PIPE_WAKERS[0].register(cx.waker());
                if CTRL_ABORT[self.port as usize].load(Ordering::Acquire) {
                    return Poll::Ready(true);
                }
                let done = PIPE_DONE.fetch_and(!(1u16), Ordering::Acquire);
                if done & 1 != 0 {
                    Poll::Ready(false)
                } else {
                    Poll::Pending
                }
            })
            .await;
            if aborted {
                return Err(EndpointError::Disabled);
            }

            // CCPL (status-stage ACK for control reads) is set by the ISR's
            // non-VALID CTRT ctsq=2 handler.
        }
        Ok(())
    }

    async fn accept(&mut self) {
        // Called by embassy-usb only for control OUT transfers (data from host).
        // The device sends a zero-length IN to acknowledge (status stage).
        // Disable BEMP0/BRDY0 BEFORE setting CCPL so the status-ZLP BEMP/BRDY
        // don't set stale flags for the next transfer.
        trace!("usb{}: accept (control-write status stage)", self.port);
        let u = usb(self.port);
        u.bempenb().modify(|r| *r &= !0x0001);
        u.brdyenb().modify(|r| *r &= !0x0001);
        let ctr = pipectr(u, 0).read();
        pipectr(u, 0).write_value((ctr & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF | PIPECTR_CCPL);
    }

    async fn reject(&mut self) {
        trace!("usb{}: reject (STALL)", self.port);
        unsafe {
            let u = usb(self.port);
            // State-aware STALL on the DCP (NAK→STALL10, BUF→STALL11) via RMW:
            // a bare write clobbers SQMON etc., and a direct STALL(10) write from
            // PID=BUF may not engage the protocol STALL.  `pipe_stall` picks the
            // correct encoding and preserves the other DCPCTR bits.
            pipe_stall(u, 0);
        }
    }

    async fn accept_set_address(&mut self, addr: u8) {
        debug!("usb{}: accept_set_address({})", self.port, addr);
        let u = usb(self.port);
        // Hardware auto-updates USBADDR when CCPL is set.
        // Disable pipe 0 BEMP and BRDY interrupts BEFORE setting CCPL (see
        // accept()).
        u.bempenb().modify(|r| *r &= !0x0001);
        u.brdyenb().modify(|r| *r &= !0x0001);
        // Same as accept(): CCPL only completes the status stage when
        // PID=BUF (TRM §28.3.30 CCPL), so set both in one RMW.
        let _ = addr;
        let ctr = pipectr(u, 0).read();
        pipectr(u, 0).write_value((ctr & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF | PIPECTR_CCPL);
    }
}

// ---------------------------------------------------------------------------
// Rusb1EndpointOut
// ---------------------------------------------------------------------------

pub struct Rusb1EndpointOut {
    pub port: u8,
    pub pipe: u8,
    pub info: EndpointInfo,
}

impl Rusb1EndpointOut {
    /// Stall (halt) this endpoint's pipe.
    ///
    /// Used by class code to signal a protocol error without access to the
    /// [`Rusb1Bus`] — e.g. a USB Mass Storage Bulk-Only Transport phase error.
    /// The host clears the halt with Clear-Feature(ENDPOINT_HALT), which the
    /// standard control path turns into `endpoint_set_stalled(false)`.
    pub fn stall(&mut self) {
        unsafe { pipe_stall(usb(self.port), self.pipe as usize) };
    }
}

impl embassy_usb_driver::Endpoint for Rusb1EndpointOut {
    fn info(&self) -> &EndpointInfo {
        &self.info
    }

    async fn wait_enabled(&mut self) {
        core::future::poll_fn(|cx| {
            PIPE_WAKERS[self.pipe as usize].register(cx.waker());
            let u = usb(self.port);
            let ctr = pipectr(u, self.pipe as usize).read();
            if (ctr & super::regs::PIPECTR_PID_MASK) == PIPECTR_PID_BUF {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

impl EndpointOut for Rusb1EndpointOut {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
        let pipe = self.pipe as usize;
        let mps = self.info.max_packet_size;

        // Store the transfer state so the ISR can fill the buffer.
        critical_section::with(|cs| {
            let state = unsafe { &mut *PIPE_XFER[pipe].borrow(cs).get() };
            state.buf = unsafe { core::ptr::NonNull::new_unchecked(buf.as_mut_ptr()) };
            state.length = buf.len() as u16;
            state.remaining = buf.len() as u16;
            state.transferred = 0;
            state.mps = mps;
            state.buf_bytes = mps; // OUT pipes drain one packet per FIFO read
            state.xfer_type = endpoint_type_to_xfer(self.info.ep_type);
        });

        let is_iso = self.info.ep_type == EndpointType::Isochronous;

        // Enable BRDY for this pipe → ISR will call pipe_xfer_out_brdy.
        //
        // For ISO OUT: BRDYENB must stay asserted continuously across packets.
        // Disabling and re-enabling it between packets wipes BRDYSTS, losing
        // any packet that arrived during the gap and permanently stalling the
        // double-buffer.  So for ISO we only arm BRDY the first time (when
        // BRDYENB is not yet set) and never disable it between reads.
        //
        // Additionally: for ISO, flush any stale packet that arrived before
        // this read() call armed the transfer state (remaining was 0 when the
        // BRDY ISR ran, so it wasn't consumed).  If we don't BCLR here the
        // double-buffer bank stays "owned" by the CPU and no further BRDYs fire.
        unsafe {
            let u = usb(self.port);
            let already_enabled = is_iso && {
                let brdyenb = u.brdyenb();
                brdyenb.read() & (1u16 << pipe) != 0
            };

            // D1FIFO mutual exclusion: the arm/drain below points the shared
            // FIFO port's CURPIPE at this pipe and then copies data out.  The
            // USB ISR does the same select-then-copy for other pipes (an IN
            // pipe's BEMP continuation fill, or another OUT pipe's BRDY drain);
            // if it fired mid-copy it would re-point CURPIPE and the rest of
            // this copy would read the wrong pipe's FIFO, corrupting both
            // transfers.  The critical section makes every select+copy sequence
            // mutually exclusive with the ISR.  (The IN first-fill path in
            // write() is covered the same way by pipe_xfer_in_start.)
            let fired = critical_section::with(|_| {
                if !already_enabled {
                    // First arm: flush any stale ISO packet before enabling BRDY.
                    if is_iso {
                        use super::fifo::{fifo_bclr, fifo_for_pipe, fifo_select_pipe};
                        let fifo = fifo_for_pipe(u, pipe);
                        fifo_select_pipe(&fifo, pipe, false);
                        fifo_bclr(&fifo);
                    }
                    pipe_brdy_enable(u, pipe);

                    // For non-ISO (bulk/interrupt) OUT: BRDY is disabled between
                    // reads and re-enabled here, and pipe_brdy_enable() just
                    // cleared BRDYSTS.  A packet that arrived while BRDY was
                    // disabled has lost its status flag and will never raise an
                    // interrupt, so drain it directly here.  `speculative = true`
                    // keeps a zero-byte drain from completing the read (see
                    // pipe_xfer_out_brdy).
                    if !is_iso {
                        return pipe_xfer_out_brdy(u, pipe, true);
                    }
                    false
                } else if is_iso {
                    // Subsequent re-arms: without an ISO OUT hook, the ISR leaves a
                    // packet that arrived while remaining==0 in the FIFO.  Drain
                    // it now so the poll_fn below completes without waiting for
                    // another BRDY.  Speculative (gated by FRDY only), but ISO is
                    // exempt from the zero-byte guard so empty microframes still
                    // complete.
                    pipe_xfer_out_brdy(u, pipe, true)
                } else {
                    false
                }
            });
            if fired {
                PIPE_DONE.fetch_or(1u16 << pipe, Ordering::Release);
                PIPE_WAKERS[pipe].wake();
            }
            pipe_enable(u, pipe);
        }

        // Wait for ISR to complete the read.
        let nbytes = core::future::poll_fn(|cx| {
            PIPE_WAKERS[pipe].register(cx.waker());
            let done = PIPE_DONE.load(Ordering::Acquire);
            if done & (1u16 << pipe) != 0 {
                PIPE_DONE.fetch_and(!(1u16 << pipe), Ordering::Release);
                let state = critical_section::with(|cs| {
                    let s = unsafe { &*PIPE_XFER[pipe].borrow(cs).get() };
                    s.transferred
                });
                Poll::Ready(Ok(state as usize))
            } else if PIPE_NRDY.load(Ordering::Acquire) & (1u16 << pipe) != 0 {
                PIPE_NRDY.fetch_and(!(1u16 << pipe), Ordering::Release);
                Poll::Ready(Err(EndpointError::Disabled))
            } else {
                Poll::Pending
            }
        })
        .await;

        // For non-ISO: disable BRDY between reads so the ISR doesn't fire when
        // no transfer state is set up.  For ISO: leave BRDYENB set so we don't
        // lose packets that arrive before the next read() call re-arms the pipe.
        if !is_iso {
            unsafe {
                let u = usb(self.port);
                pipe_brdy_disable(u, pipe);
            }
        }

        nbytes
    }
}

// ---------------------------------------------------------------------------
// Rusb1EndpointIn
// ---------------------------------------------------------------------------

pub struct Rusb1EndpointIn {
    pub port: u8,
    pub pipe: u8,
    pub info: EndpointInfo,
    /// Bytes committed to the FIFO per fill (= `mps`, or the whole multi-packet
    /// buffer for a continuous-mode bulk IN pipe).
    pub buf_bytes: u16,
}

impl Rusb1EndpointIn {
    /// Stall (halt) this endpoint's pipe.  See [`Rusb1EndpointOut::stall`].
    pub fn stall(&mut self) {
        unsafe { pipe_stall(usb(self.port), self.pipe as usize) };
    }
}

impl embassy_usb_driver::Endpoint for Rusb1EndpointIn {
    fn info(&self) -> &EndpointInfo {
        &self.info
    }

    async fn wait_enabled(&mut self) {
        core::future::poll_fn(|cx| {
            PIPE_WAKERS[self.pipe as usize].register(cx.waker());
            let u = usb(self.port);
            let ctr = pipectr(u, self.pipe as usize).read();
            if (ctr & super::regs::PIPECTR_PID_MASK) == PIPECTR_PID_BUF {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

impl EndpointIn for Rusb1EndpointIn {
    async fn write(&mut self, buf: &[u8]) -> Result<(), EndpointError> {
        let pipe = self.pipe as usize;
        let mps = self.info.max_packet_size;

        critical_section::with(|cs| {
            let state = unsafe { &mut *PIPE_XFER[pipe].borrow(cs).get() };
            // Cast away const — we only read this pointer in the ISR.
            state.buf = unsafe { core::ptr::NonNull::new_unchecked(buf.as_ptr() as *mut u8) };
            state.length = buf.len() as u16;
            state.remaining = buf.len() as u16;
            state.mps = mps;
            state.buf_bytes = self.buf_bytes;
            state.xfer_type = endpoint_type_to_xfer(self.info.ep_type);
        });

        // Clear any stale completion / error state from a previous transfer
        // before arming this one.  A leftover PIPE_DONE bit (e.g. from a
        // spurious BEMP shortly after the previous packet drained) would make
        // poll_fn below return immediately; the next write() would then send on
        // the same data toggle, and the host would discard that packet as a
        // retransmission.
        PIPE_DONE.fetch_and(!(1u16 << pipe), Ordering::Release);
        PIPE_NRDY.fetch_and(!(1u16 << pipe), Ordering::Release);

        // Kick off the first packet; subsequent packets are sent from the BEMP ISR.
        //
        // Order matters in two ways:
        //
        // 1. Write the FIFO data (pipe_xfer_in_start) BEFORE setting PID=BUF
        //    (pipe_enable).  If PID=BUF is asserted before the FIFO holds a
        //    committed buffer the SIE answers IN tokens with NAK and can stay
        //    stuck in NAK.
        //
        // 2. Use BEMP for non-control IN progress/completion.  The TRM defines
        //    BRDY on transmitting pipes as "FIFO write access available", which
        //    can fire while a packet is merely staged; BEMP is the
        //    transmit-complete source.  Order: clear stale status, fill, arm
        //    BEMP, then release the pipe to BUF — the arm must precede the enable
        //    (see the BEMP-before-BUF note below).
        unsafe {
            let u = usb(self.port);
            // Hold the pipe at NAK while we fill the FIFO and clear old status.
            // NAK -> fill -> arm BEMP -> BUF avoids stale BEMP wakeups and the
            // BRDY "CPU may write" false completion on transmitting pipes.
            pipe_disable(u, pipe);
            pipe_brdy_disable(u, pipe);
            pipe_bemp_disable(u, pipe);
            u.brdysts().write_value(!((1u16) << pipe));
            u.bempsts().write_value(!((1u16) << pipe));
            // Commit the first packet to the FIFO; the BEMP ISR sends the rest.
            pipe_xfer_in_start(u, pipe, mps);
            u.brdysts().write_value(!((1u16) << pipe));
            u.bempsts().write_value(!((1u16) << pipe));
            // Arm BEMP *before* releasing the pipe to BUF.  pipe_bemp_enable
            // clears BEMPSTS as part of enabling; if it ran after pipe_enable a
            // transmit-completion BEMP that fired in the gap — the host can read
            // a short HS IN packet in well under a microframe — would be wiped
            // before BEMPENB latched it, and with no further BEMP write() would
            // hang.  PID is still NAK here, so no transmission — hence no BEMP —
            // can occur until pipe_enable, and any stale BEMP was just cleared
            // above.
            pipe_bemp_enable(u, pipe);
            pipe_enable(u, pipe);
        }

        // Wait for all packets to drain.
        core::future::poll_fn(|cx| {
            PIPE_WAKERS[pipe].register(cx.waker());
            let done = PIPE_DONE.load(Ordering::Acquire);
            if done & (1u16 << pipe) != 0 {
                PIPE_DONE.fetch_and(!(1u16 << pipe), Ordering::Release);
                Poll::Ready(Ok(()))
            } else if PIPE_NRDY.load(Ordering::Acquire) & (1u16 << pipe) != 0 {
                PIPE_NRDY.fetch_and(!(1u16 << pipe), Ordering::Release);
                Poll::Ready(Err(EndpointError::Disabled))
            } else {
                Poll::Pending
            }
        })
        .await?;

        unsafe {
            let u = usb(self.port);
            pipe_bemp_disable(u, pipe);
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ISR
// ---------------------------------------------------------------------------

/// USB device-mode interrupt handler.
///
/// Must be registered with the GIC for IRQ 73 (port 0) or 74 (port 1) before
/// `Rusb1Bus::enable()` enables the per-pipe interrupt bits.
///
/// # Safety
/// Must only be called from an interrupt context matching the port's GIC IRQ.
/// Single-core bare-metal only.
pub unsafe fn dcd_int_handler(port: u8) {
    unsafe {
        let u = usb(port);
        let p = port as usize;

        let sts = u.intsts(0).read();

        // Clear the observed sticky flags in a single write, preserving the
        // VALID bit.  Never write 0 to bits that were not observed: hardware may
        // have set new ones since the read.
        //
        // RC-W0 semantics: writing 0 to a bit clears it; writing 1 leaves it set.
        // So: ~(observed_flags) | VALID  →  clears observed bits, keeps VALID=1.
        const STICKY_FLAGS: u16 = INTSTS0_CTRT | INTSTS0_DVST | INTSTS0_VBINT;
        u.intsts(0)
            .write_value(!(STICKY_FLAGS & sts) | INTSTS0_VALID);

        // Housekeeping: drain NRDYSTS so bits can't accumulate (matches the Linux
        // ISR's unconditional `usbhs_write(NRDYSTS, ~nrdysts)`).  NRDY is not
        // currently enabled, but leaving stale bits set would fire an interrupt
        // storm the moment NRDYE is turned on.  RC-W0: write 0 to the set bits.
        let nrdysts = u.nrdysts().read();
        if nrdysts != 0 {
            u.nrdysts().write_value(!nrdysts);
        }

        // ── VBINT (VBUS change) ──────────────────────────────────────────────
        if sts & INTSTS0_VBINT != 0 {
            if sts & INTSTS0_VBSTS != 0 {
                BUS_EVENTS[p].fetch_or(BUS_EVT_VBUS_ON, Ordering::Release);
            } else {
                BUS_EVENTS[p].fetch_or(BUS_EVT_VBUS_OFF, Ordering::Release);
            }
            BUS_WAKERS[p].wake();
        }

        // ── RESM (resume detected) ───────────────────────────────────────────
        // The host has driven the bus out of suspend. RESM is enabled only while
        // suspended (see the DVST suspend path). Clear the status, disable RESM
        // again, and emit Resume so embassy-usb's wait_resume() unblocks and
        // resumes servicing control/data.
        // RESM is not in STICKY_FLAGS, so the bulk clear at the top of the ISR
        // left it set — clear it here (write 0 to RESM, 1 elsewhere to preserve).
        if sts & INTSTS0_RESM != 0 {
            u.intsts(0).write_value(!INTSTS0_RESM);
            u.intenb(0).modify(|r| *r &= !INTENB0_RSME);
            BUS_EVENTS[p].fetch_or(BUS_EVT_RESUME, Ordering::Release);
            BUS_WAKERS[p].wake();
        }

        // ── DVST (device-state change) ───────────────────────────────────────
        if sts & INTSTS0_DVST != 0 {
            let dvsq = (sts & INTSTS0_DVSQ_MASK) >> INTSTS0_DVSQ_SHIFT;
            // DVSQ=2 (Address) and DVSQ=3 (Configured): hardware auto-handles
            // SET_ADDRESS and SET_CONFIGURATION state transitions, so no event
            // is emitted for them; emitting Resume here would disrupt
            // embassy-usb's enumeration state machine.
            let evt = match dvsq {
                d if d == DVSQ_DEFAULT => Some(BUS_EVT_RESET),
                d if d >= DVSQ_SUSP0 => Some(BUS_EVT_SUSPEND),
                _ => None,
            };
            if let Some(evt) = evt {
                if evt == BUS_EVT_SUSPEND {
                    // Arm the resume (RESM) interrupt so we can detect the host
                    // driving K-state to wake us. The module clock is gated in
                    // suspend, so DVST can't be relied on for resume — RESM is the
                    // analog line-state detector that fires regardless.  Without
                    // it the device never emits Event::Resume, and embassy-usb's
                    // wait_resume() (which only polls the bus) blocks forever.
                    u.intenb(0).modify(|r| *r |= INTENB0_RSME);
                }
                BUS_EVENTS[p].fetch_or(evt, Ordering::Release);
                BUS_WAKERS[p].wake();
            }

            // On reset, run the proper bus-reset sequence then reconfigure pipes.
            if dvsq == DVSQ_DEFAULT {
                process_bus_reset(port);
            }
        }

        // ── CTRT (control transfer stage) ────────────────────────────────────
        if sts & INTSTS0_CTRT != 0 {
            // Re-read INTSTS0 to get the live VALID bit.
            let intsts_live = u.intsts(0).read();
            if intsts_live & INTSTS0_VALID != 0 {
                // Clear CFIFO before latching the setup packet — any data left
                // over from the previous transfer must be discarded.
                u.cfifoctr().write_value(FIFOCTR_BCLR);

                // Capture setup packet from USBREQ/USBVAL/USBINDX/USBLENG.
                let req = u.usbreq().read();
                let val = u.usbval().read();
                let idx = u.usbindx().read();
                let len = u.usbleng().read();
                let pkt = [
                    (req & 0xFF) as u8,
                    (req >> 8) as u8,
                    (val & 0xFF) as u8,
                    (val >> 8) as u8,
                    (idx & 0xFF) as u8,
                    (idx >> 8) as u8,
                    (len & 0xFF) as u8,
                    (len >> 8) as u8,
                ];
                let cs = critical_section::CriticalSection::new();
                *SETUP_PKT[p].borrow(cs).get() = pkt;
                // A new control request: a disable from it may undo only its own enables.
                SETUP_GEN[p].fetch_add(1, Ordering::AcqRel);

                // Clear VALID now that the setup packet has been latched.
                u.intsts(0).write_value(!INTSTS0_VALID);

                // Signal setup() only when VALID=1 (i.e. a real SETUP packet).
                // Status-stage CTRT events (VALID=0) must NOT wake setup() or
                // it would return a stale packet.
                let ctsq = (sts & INTSTS0_CTSQ_MASK) as u8;
                CTRL_STAGE[p].store(ctsq, Ordering::Release);
                CTRL_WAKERS[p].wake();
            } else {
                // Non-VALID CTRT: a status-stage transition is completing.
                let ctsq = (sts & INTSTS0_CTSQ_MASK) as u8;
                if ctsq == 2 {
                    // Control read status stage (CS_RDSS, ctsq=2): host sends its
                    // zero-length OUT to ACK our IN data.  Set CCPL so the
                    // hardware ACKs that ZLP: disable BEMP0/BRDY0, then
                    // PID=BUF|CCPL.  embassy-usb does not call accept() for
                    // control-read transfers.
                    u.bempenb().modify(|r| *r &= !0x0001);
                    u.brdyenb().modify(|r| *r &= !0x0001);
                    let ctr = pipectr(u, 0).read();
                    pipectr(u, 0)
                        .write_value((ctr & !PIPECTR_PID_MASK) | PIPECTR_PID_BUF | PIPECTR_CCPL);
                    // The host only enters the status stage once it has all the IN data, so
                    // the data stage is complete. Its BEMP may be pending in this same
                    // interrupt, and the BEMPENB write above masks it from the BEMP handler
                    // below: complete `data_in` here or it waits forever.
                    PIPE_DONE.fetch_or(1, Ordering::Release);
                    PIPE_WAKERS[0].wake();
                }
                // ctsq=4 (WRSS) / ctsq=5 (WRND): accept() / accept_set_address()
                // are called by the embassy-usb task and set CCPL from there.
            }
        }

        // ── BRDY (OUT data received / control write data-stage data) ────────
        if sts & INTSTS0_BRDY != 0 {
            let brdysts = u.brdysts().read();
            let brdyenb = u.brdyenb().read();
            let active = brdysts & brdyenb;

            #[allow(clippy::needless_range_loop)]
            for n in 0..16usize {
                if active & (1 << n) == 0 {
                    continue;
                }
                // Clear the BRDY flag for this pipe.
                u.brdysts().write_value(!((1u16) << n));

                if n == 0 {
                    // Pipe 0 BRDY: data arrived for the control write data stage.
                    // Signal data_out() which is waiting on PIPE0_BRDY.
                    PIPE0_BRDY[p].store(1, Ordering::Release);
                    CTRL_WAKERS[p].wake();
                } else if super::iso::in_bound(port, n) {
                    // Continuous ISO IN (mic): a transmit buffer freed up — stage
                    // the next packet from the hook and keep the pipe armed.
                    pipe_xfer_in_brdy(u, n);
                } else {
                    // Real BRDY: BRDYSTS was set, so a packet (possibly a genuine
                    // ZLP) is present — speculative = false lets a zero-byte read
                    // legitimately complete the transfer.
                    let done = pipe_xfer_out_brdy(u, n, false);
                    if done {
                        PIPE_DONE.fetch_or(1u16 << n, Ordering::Release);
                        PIPE_WAKERS[n].wake();
                    }
                }
            }
        }

        // ── BEMP (buffer empty — IN transmit progress/completion) ────────────
        if sts & INTSTS0_BEMP != 0 {
            let bempsts = u.bempsts().read();
            let bempenb = u.bempenb().read();
            let active = bempsts & bempenb;

            #[allow(clippy::needless_range_loop)]
            for n in 0..16usize {
                if active & (1 << n) == 0 {
                    continue;
                }
                u.bempsts().write_value(!((1u16) << n));

                if n == 0 {
                    // Pipe 0 BEMP: all data has been sent for the control-read
                    // data stage.  Disable the BEMP0 interrupt now; CCPL is set
                    // by the CTRT ctsq=2 handler.
                    u.bempenb().modify(|r| *r &= !0x0001);
                    PIPE_DONE.fetch_or(1, Ordering::Release);
                    PIPE_WAKERS[0].wake();
                } else if PIPE_IS_IN.load(Ordering::Acquire) & (1u16 << n) != 0
                    && pipe_xfer_in_bemp(u, n)
                {
                    // All packets transmitted — complete unconditionally.
                    // `write()` clears any stale BEMP before arming, so this BEMP
                    // belongs to the current transfer.  Deliberately not gated
                    // on the SQMON data toggle: BEMP can fire a few cycles
                    // before the hardware finishes toggling SQMON on the host
                    // ACK, and with the BEMP status bit already cleared there
                    // would be no later interrupt to recover the completion.
                    //
                    // Disable this pipe's BEMP here, as Linux does when a packet
                    // completes (usbhsf_tx_irq_ctrl(pipe, 0)): the empty pipe stays
                    // at BUF and BEMP re-asserts while the host polls it, a storm
                    // that would starve the task `write()` must wake.
                    pipe_bemp_disable(u, n);
                    PIPE_DONE.fetch_or(1u16 << n, Ordering::Release);
                    PIPE_WAKERS[n].wake();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// How a device-mode endpoint is carried: the pipe type, the MXPS to programme, and whether
/// it needs pipes 1-2.
///
/// Interrupt pipes (6-8) stop at 64 bytes, so a larger interrupt endpoint (Overbridge's
/// 368-byte audio blocks) rides a bulk pipe; on the wire the host's interrupt transactions
/// look the same. Pipes 3-5 and 9-15 take only MXPS 8/16/32/64/512 (TRM §28.3.34), so up to
/// 512 bytes it is programmed at 512 (every packet short); above that only pipes 1-2 take the
/// size.
fn device_layout(ep_type: EndpointType, max_packet_size: u16) -> (XferType, u16, bool) {
    match ep_type {
        EndpointType::Interrupt if max_packet_size > 512 => (XferType::Bulk, max_packet_size, true),
        EndpointType::Interrupt if max_packet_size > 64 => (XferType::Bulk, 512, false),
        t => (endpoint_type_to_xfer(t), max_packet_size, false),
    }
}

fn endpoint_type_to_xfer(t: EndpointType) -> XferType {
    match t {
        EndpointType::Control => XferType::Control,
        EndpointType::Bulk => XferType::Bulk,
        EndpointType::Interrupt => XferType::Interrupt,
        EndpointType::Isochronous => XferType::Isochronous,
    }
}

/// Minimum 64-byte buffer blocks for a given MPS.
fn mps_to_blocks(mps: u16) -> usize {
    (mps as usize).div_ceil(PKT_BUF_BLOCK_SIZE)
}

/// Translate an `EndpointAddress` to a hardware pipe number for the given port.
fn ep_addr_to_pipe(port: u8, ep_addr: EndpointAddress) -> Option<usize> {
    let ep_num = ep_addr.index();
    let dir_idx = if ep_addr.direction() == Direction::In {
        1
    } else {
        0
    };
    critical_section::with(|cs| {
        let alloc = unsafe { &*PORT_ALLOC[port as usize].borrow(cs).get() };
        let p = alloc.ep_to_pipe[dir_idx][ep_num] as usize;
        if p == 0 && ep_num != 0 { None } else { Some(p) }
    })
}

/// Perform the hardware bus-reset sequence.
///
/// - Disables all pipe BRDY/BEMP except DCP pipe 0 BRDY.
/// - Clears and deselects all FIFO ports.
/// - Issues ACLRM + SQCLR on every active pipe.
/// - Aborts any in-progress transfers (wakes waiters with NRDY error).
/// - Then reconfigures all pipes so they are ready for the re-enumeration.
unsafe fn process_bus_reset(port: u8) {
    forget_enables(port as usize);
    unsafe {
        debug!("usb{}: bus reset — reconfiguring", port);
        let u = usb(port);
        let p = port as usize;

        // Log the negotiated bus speed from DVSTCTR0.RHST.
        let rhst = u.dvstctr0().read() & DVSTCTR0_RHST;
        let speed_str = match rhst {
            r if r == DVSTCTR0_RHST_HS => "HS (480 Mbps)",
            r if r == DVSTCTR0_RHST_FS => "FS (12 Mbps)",
            _ => "LS / unknown",
        };
        info!(
            "usb{}: bus reset — negotiated speed: {} (RHST={:#06x})",
            port,
            speed_str,
            rhst
        );

        // Only keep pipe 0 BRDY/BEMP enabled — pipes 1-15 are re-enabled by
        // `endpoint_set_enabled` and per-transfer in `read()` / `write()`.
        // BEMPENB bit 0 must be set so that data_in() receives BEMP on pipe 0.
        u.brdyenb().write_value(0x0001);
        u.bempenb().write_value(0x0001);

        // If we got a reset instead of a resume out of suspend, RSME may still be
        // armed — disable it so a later spurious RESM can't fire. RESM is re-armed
        // by the DVST suspend path as needed.
        u.intenb(0).modify(|r| *r &= !INTENB0_RSME);

        // Drop any queued-but-unpolled suspend/resume event: a reset supersedes
        // them.  Bus::poll prioritises Reset, so a stale Suspend/Resume left in
        // BUS_EVENTS would be delivered immediately after Event::Reset and
        // perturb embassy-usb's enumeration state machine.
        BUS_EVENTS[p].fetch_and(!(BUS_EVT_SUSPEND | BUS_EVT_RESUME), Ordering::AcqRel);

        // Clear the control FIFO.
        u.cfifoctr().write_value(FIFOCTR_BCLR);

        // Deselect D0FIFO and D1FIFO (CURPIPE = 0).
        u.d0fifosel().write_value(0);
        u.d1fifosel().write_value(0);

        // ACLRM + SQCLR every active pipe and abort any in-progress transfer.
        let mut abort_mask: u16 = 0;
        critical_section::with(|cs| {
            let alloc = &*PORT_ALLOC[p].borrow(cs).get();
            #[allow(clippy::needless_range_loop)]
            for n in 1..PIPE_COUNT {
                if alloc.pipe_cfg[n].is_some() {
                    let ctr = pipectr(u, n);
                    // ACLRM clears the pipe FIFO and resets the toggle.
                    ctr.write_value(PIPECTR_ACLRM | PIPECTR_SQCLR);
                    ctr.write_value(0);

                    // Signal transfer failure to any waiting task.
                    //
                    // Test `buf`, not `remaining`: an IN transfer that has
                    // staged all of its data sits at `remaining == 0` while it
                    // waits for the transmit-complete interrupt, so `remaining`
                    // alone does not mean "idle".  `buf` is the documented
                    // in-flight marker — dangling when idle, and only set
                    // dangling again on completion or abort.
                    //
                    // The reset above clears BEMPENB wholesale, so a transfer
                    // missed here loses the interrupt it is blocked on and
                    // `write()` never completes.
                    let state = &mut *PIPE_XFER[n].borrow(cs).get();
                    if state.buf != core::ptr::NonNull::dangling() {
                        state.remaining = 0;
                        state.buf = core::ptr::NonNull::dangling();
                        abort_mask |= 1u16 << n;
                    }
                }
            }
        });

        // Wake aborted waiters (they will observe PIPE_NRDY and return an error).
        if abort_mask != 0 {
            PIPE_NRDY.fetch_or(abort_mask, Ordering::Release);
            #[allow(clippy::needless_range_loop)]
            for n in 1..16usize {
                if abort_mask & (1u16 << n) != 0 {
                    PIPE_WAKERS[n].wake();
                }
            }
        }

        // Abort any in-flight control (pipe 0) data/status stage. embassy-usb
        // awaits these *outside* the bus.poll() select, so a host reset mid
        // control transfer would hang device.run() — the BRDY/BEMP it waits on
        // never arrives.  Flag + wake both control waiters so data_in/data_out
        // return an error and the device task falls back to bus.poll().
        CTRL_ABORT[p].store(true, Ordering::Release);
        CTRL_WAKERS[p].wake();
        PIPE_WAKERS[0].wake();

        // Re-apply all pipe hardware configs so the host can re-enumerate.
        configure_all_pipes(port);
    }
}

/// (Re-)configure all allocated pipes on the hardware.  Called on enable and
/// on bus reset.
unsafe fn configure_all_pipes(port: u8) {
    unsafe {
        let u = usb(port);
        critical_section::with(|cs| {
            let alloc = &*PORT_ALLOC[port as usize].borrow(cs).get();
            for (n, cfg_opt) in alloc.pipe_cfg.iter().enumerate() {
                if n == 0 {
                    continue;
                } // DCP configured via DCPMAXP/DCPCFG
                if let Some(cfg) = cfg_opt {
                    pipe_configure(u, n, cfg);
                }
            }
        });
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::mmio::test;

    /// A one-packet control read: the host's status-stage OUT follows the IN data so closely that
    /// the CTRT (control read status stage, ctsq=2) and the pipe-0 BEMP land in one interrupt.
    /// `data_in` waits on pipe 0's PIPE_DONE bit, so the data stage must still complete; if it
    /// doesn't, the control pipe stalls and the device never enumerates.
    #[test]
    fn a_status_stage_with_bemp_pending_still_completes_the_control_read() {
        let u = usb(0);
        test::reset();
        for r in [u.intsts(0).addr(), u.bempsts().addr()] {
            test::rcw0(r);
        }
        test::poke16(u.intsts(0).addr(), INTSTS0_CTRT | INTSTS0_BEMP | 2);
        test::poke16(u.bempsts().addr(), 0x0001);
        test::poke16(u.bempenb().addr(), 0x0001);
        PIPE_DONE.fetch_and(!1u16, Ordering::Relaxed);
        unsafe { dcd_int_handler(0) };
        assert_ne!(
            PIPE_DONE.load(Ordering::Acquire) & 1,
            0,
            "the data stage did not complete"
        );
        let ctr = pipectr(u, 0).read();
        assert_eq!(
            ctr & (PIPECTR_PID_MASK | PIPECTR_CCPL),
            PIPECTR_PID_BUF | PIPECTR_CCPL
        );
    }

    /// The BEMP that ends an IN transfer turns that pipe's BEMP interrupt off. The pipe stays at
    /// PID=BUF with its buffer empty, and while the host polls it with IN tokens the BEMP keeps
    /// re-asserting: left enabled until `write()` gets to run, it is an interrupt storm that keeps
    /// the task from ever running (seen on hardware as a CDC port that hangs once opened).
    #[test]
    fn the_bemp_that_ends_an_in_transfer_disables_the_pipes_bemp() {
        const PIPE: usize = 4;
        let u = usb(0);
        test::reset();
        for r in [u.intsts(0).addr(), u.bempsts().addr()] {
            test::rcw0(r);
        }
        test::poke16(u.intsts(0).addr(), INTSTS0_BEMP);
        test::poke16(u.bempsts().addr(), 1 << PIPE);
        test::poke16(u.bempenb().addr(), 1 << PIPE);
        PIPE_IS_IN.fetch_or(1 << PIPE, Ordering::Relaxed);
        critical_section::with(|cs| unsafe { (*PIPE_XFER[PIPE].borrow(cs).get()).remaining = 0 });
        PIPE_DONE.fetch_and(!(1 << PIPE), Ordering::Relaxed);
        unsafe { dcd_int_handler(0) };
        assert_ne!(
            PIPE_DONE.load(Ordering::Acquire) & (1 << PIPE),
            0,
            "the transfer completes"
        );
        assert_eq!(
            test::peek16(u.bempenb().addr()) & (1 << PIPE),
            0,
            "BEMP left enabled"
        );
    }

    /// The allocation tests share port 1's allocator; run them one at a time.
    static ALLOC_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn fresh_port1() -> Rusb1Driver<'static> {
        test::reset();
        unsafe { Rusb1Driver::new(1) }
    }

    fn pipe_cfg(pipe: u8) -> PipeConfig {
        critical_section::with(|cs| unsafe {
            (*PORT_ALLOC[1].borrow(cs).get()).pipe_cfg[pipe as usize].unwrap()
        })
    }

    fn pipes_used() -> u16 {
        critical_section::with(|cs| unsafe { (*PORT_ALLOC[1].borrow(cs).get()).pipes_used })
    }

    /// UAC2's streaming alts 1 and 2 share their endpoint addresses, so they share a pipe.
    fn shared_alt_out_endpoint(d: &mut Rusb1Driver<'static>) -> EndpointAddress {
        let alt1 = d.alloc_endpoint_out(EndpointType::Isochronous, None, 288, 1).unwrap();
        let alt2 = d
            .alloc_endpoint_out(EndpointType::Isochronous, Some(alt1.info.addr), 288, 1)
            .unwrap();
        assert_eq!(alt1.pipe, alt2.pipe);
        alt1.info.addr
    }

    /// A SETUP packet arrives (here: a SET_INTERFACE).
    fn next_setup(port: usize) {
        SETUP_GEN[port].fetch_add(1, Ordering::AcqRel);
    }

    fn pid(port: u8, addr: EndpointAddress) -> u16 {
        test::peek16(pipectr(usb(port), ep_addr_to_pipe(port, addr).unwrap()).addr()) & 0b11
    }

    /// embassy-usb walks every alt's endpoints for one SET_INTERFACE, in descriptor order: alt 1
    /// (selected) enables the address, then alt 2 (not selected) disables the same address.
    #[test]
    fn selecting_alt_1_keeps_its_shared_endpoint_enabled() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let addr = shared_alt_out_endpoint(&mut d);
        let mut bus = Rusb1Bus { port: 1, connected: true };
        next_setup(1);
        bus.endpoint_set_enabled(addr, true);
        bus.endpoint_set_enabled(addr, false);
        assert_eq!(pid(1, addr), 0b01, "PID stays BUF");
        let pipe = ep_addr_to_pipe(1, addr).unwrap();
        assert_ne!(test::peek16(usb(1).brdyenb().addr()) & (1 << pipe), 0, "BRDY stays enabled");
    }

    #[test]
    fn a_later_request_can_disable_it() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let addr = shared_alt_out_endpoint(&mut d);
        let mut bus = Rusb1Bus { port: 1, connected: true };
        next_setup(1);
        bus.endpoint_set_enabled(addr, true);
        next_setup(1); // SET_INTERFACE to alt 0
        bus.endpoint_set_enabled(addr, false);
        assert_eq!(pid(1, addr), 0b00, "PID NAK");
    }

    #[test]
    fn a_bus_reset_forgets_the_enables() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let addr = shared_alt_out_endpoint(&mut d);
        let mut bus = Rusb1Bus { port: 1, connected: true };
        next_setup(1);
        bus.endpoint_set_enabled(addr, true);
        forget_enables(1);
        bus.endpoint_set_enabled(addr, false);
        assert_eq!(pid(1, addr), 0b00, "PID NAK");
    }

    /// Enabling an iso IN endpoint with a bound producer (SET_INTERFACE, or again after a bus
    /// reset) primes both FIFO planes before releasing the pipe to BUF, or BRDY never fires for
    /// the second plane (TRM §28.4, BRDY transmitting condition 4).
    #[test]
    fn re_enabling_a_bound_in_pipe_primes_both_planes() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static FILLS: AtomicU32 = AtomicU32::new(0);
        struct Count;
        impl super::super::iso::IsoInProducer for Count {
            // Full packets: a short one's BVAL write would clear FRDY in the shadow's single
            // FIFOCTR cell (the hardware keeps it), hiding the second plane.
            fn fill_packet(&mut self, buf: &mut [u8]) -> usize {
                FILLS.fetch_add(1, Ordering::SeqCst);
                buf.len()
            }
        }
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let ep = d.alloc_endpoint_in(EndpointType::Isochronous, None, 294, 1).unwrap();
        let (addr, pipe) = (ep.info.addr, ep.pipe as usize);
        let stream = ep.into_iso_stream(Box::leak(Box::new(Count)));
        let u = usb(1);
        let fifo = crate::usb::fifo::fifo_for_pipe(u, pipe);
        test::poke16(fifo.ctr.addr(), crate::usb::regs::FIFOCTR_FRDY);
        FILLS.store(0, Ordering::SeqCst);
        let mut bus = Rusb1Bus { port: 1, connected: true };
        bus.endpoint_set_enabled(addr, true);
        assert_eq!(FILLS.load(Ordering::SeqCst), 2, "both planes primed");
        assert_eq!(test::peek16(pipectr(u, pipe).addr()) & 0b11, 0b01, "PID = BUF");
        drop(stream);
    }

    #[test]
    fn device_layout_carries_large_interrupt_endpoints_as_bulk() {
        assert_eq!(
            device_layout(EndpointType::Interrupt, 64),
            (XferType::Interrupt, 64, false)
        );
        assert_eq!(
            device_layout(EndpointType::Interrupt, 368),
            (XferType::Bulk, 512, false)
        );
        assert_eq!(
            device_layout(EndpointType::Interrupt, 1012),
            (XferType::Bulk, 1012, true)
        );
        assert_eq!(
            device_layout(EndpointType::Bulk, 512),
            (XferType::Bulk, 512, false)
        );
        assert_eq!(
            device_layout(EndpointType::Isochronous, 294),
            (XferType::Isochronous, 294, false)
        );
    }

    #[test]
    fn a_368_byte_interrupt_in_endpoint_rides_a_double_buffered_bulk_pipe() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let ep = d
            .alloc_endpoint_in(
                EndpointType::Interrupt,
                Some(EndpointAddress::from(0x83)),
                368,
                1,
            )
            .unwrap();
        assert!(matches!(ep.pipe, 3..=5 | 9..=15), "pipe {}", ep.pipe);
        assert_eq!(
            (ep.info.ep_type, ep.info.max_packet_size),
            (EndpointType::Interrupt, 368),
            "the descriptor keeps what the class asked"
        );
        let c = pipe_cfg(ep.pipe);
        assert_eq!(
            (c.xfer_type, c.mps, c.double_buf, c.continuous),
            (XferType::Bulk, 512, true, false)
        );
        assert_eq!(ep.buf_bytes, 368, "one block per write");
    }

    #[test]
    fn an_88_byte_interrupt_out_endpoint_rides_a_bulk_pipe() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let ep = d
            .alloc_endpoint_out(
                EndpointType::Interrupt,
                Some(EndpointAddress::from(0x03)),
                88,
                1,
            )
            .unwrap();
        assert!(matches!(ep.pipe, 3..=5 | 9..=15), "pipe {}", ep.pipe);
        let c = pipe_cfg(ep.pipe);
        assert_eq!((c.xfer_type, c.mps), (XferType::Bulk, 512));
    }

    #[test]
    fn a_1012_byte_interrupt_endpoint_takes_pipe_1_or_2() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let ep = d
            .alloc_endpoint_in(EndpointType::Interrupt, None, 1012, 1)
            .unwrap();
        assert!(matches!(ep.pipe, 1..=2), "pipe {}", ep.pipe);
        assert_eq!(pipe_cfg(ep.pipe).mps, 1012);
    }

    #[test]
    fn a_shared_interrupt_endpoint_that_outgrows_its_pipe_moves() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let addr = Some(EndpointAddress::from(0x83));
        let alt1 = d
            .alloc_endpoint_in(EndpointType::Interrupt, addr, 32, 1)
            .unwrap();
        assert!(
            matches!(alt1.pipe, 6..=8),
            "a 32-byte interrupt endpoint starts on an interrupt pipe"
        );
        let free_blocks = || {
            critical_section::with(|cs| unsafe {
                (*PORT_ALLOC[1].borrow(cs).get()).buf.free_blocks()
            })
        };
        let before = free_blocks();
        let alt2 = d
            .alloc_endpoint_in(EndpointType::Interrupt, addr, 88, 1)
            .unwrap();
        assert!(
            matches!(alt2.pipe, 3..=5 | 9..=15),
            "88 bytes needs a bulk pipe: pipe {}",
            alt2.pipe
        );
        assert_eq!(
            pipes_used() & (1 << alt1.pipe),
            0,
            "the interrupt pipe is released"
        );
        // The 32-byte pipe's one block comes back; the bulk pipe takes 2 banks of 8.
        assert_eq!(
            free_blocks(),
            before + 1 - 16,
            "the old packet buffer is freed"
        );
        let alt3 = d
            .alloc_endpoint_in(EndpointType::Interrupt, addr, 368, 1)
            .unwrap();
        assert_eq!(
            alt3.pipe, alt2.pipe,
            "368 bytes fits the same MXPS-512 pipe"
        );
        assert_eq!(alt3.buf_bytes, 368);
        let map = critical_section::with(|cs| unsafe {
            (*PORT_ALLOC[1].borrow(cs).get()).ep_to_pipe[1][3]
        });
        assert_eq!(map, alt3.pipe);
    }

    #[test]
    fn small_interrupt_and_bulk_endpoints_are_unchanged() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = fresh_port1();
        let i = d
            .alloc_endpoint_in(EndpointType::Interrupt, None, 64, 1)
            .unwrap();
        assert!(matches!(i.pipe, 6..=8));
        assert_eq!(pipe_cfg(i.pipe).xfer_type, XferType::Interrupt);
        let b = d
            .alloc_endpoint_out(EndpointType::Bulk, None, 512, 0)
            .unwrap();
        assert!(matches!(b.pipe, 3..=5 | 9..=15));
        assert_eq!(
            (pipe_cfg(b.pipe).xfer_type, pipe_cfg(b.pipe).mps),
            (XferType::Bulk, 512)
        );
    }

    /// A fresh device-mode bring-up (a face switch after `disconnect`) drops bus events and
    /// control state the previous session left behind.
    #[test]
    fn a_new_driver_clears_the_previous_sessions_state() {
        let _g = ALLOC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        BUS_EVENTS[1].store(0x0F, Ordering::Release);
        CTRL_STAGE[1].store(2, Ordering::Release);
        CTRL_ABORT[1].store(true, Ordering::Release);
        PIPE0_BRDY[1].store(1, Ordering::Release);
        let _d = fresh_port1();
        assert_eq!(BUS_EVENTS[1].load(Ordering::Acquire), 0);
        assert_eq!(CTRL_STAGE[1].load(Ordering::Acquire), CTRL_STAGE_NONE);
        assert!(!CTRL_ABORT[1].load(Ordering::Acquire));
        assert_eq!(PIPE0_BRDY[1].load(Ordering::Acquire), 0);
    }
}
