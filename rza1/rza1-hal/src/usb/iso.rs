//! Isochronous streams: an iso endpoint bound to a consumer that runs in the USB interrupt.
//!
//! [`Rusb1EndpointOut::into_iso_stream`] binds an [`IsoOutConsumer`]: on every BRDY the ISR
//! drains the received packet from the FIFO and hands it over, with no task hop. A packet left in
//! the double-buffered FIFO for a couple of microframes makes the SIE drop the next one (TRM
//! §28.4.9), so audio can't wait for a task.
//!
//! [`Rusb1EndpointIn::into_iso_stream`] binds an [`IsoInProducer`]: on every BRDY the ISR asks it
//! for the next packet and commits it. Enabling the endpoint primes both FIFO planes first, or
//! BRDY never fires for the second one (TRM §28.4, BRDY transmitting condition 4).
//!
//! This sits next to the `embassy_usb_driver` endpoint traits, not in place of them: a class
//! builds its endpoints as usual, and the board turns the iso ones into streams.
//!
//! The consumer is lent `&'static mut` because the interrupt reaches it for as long as the
//! stream lives, and a stream can be leaked. One stream per pipe. Dropping a stream masks the
//! pipe's BRDY and unbinds it, so the consumer is never called after the drop returns, provided
//! the drop runs in thread mode or at a priority below the USB interrupt's (a higher-priority
//! context could land between the interrupt fetching the consumer and calling it).
//!
//! Bind an IN stream before the host enables the endpoint, or at any time after: binding to an
//! endpoint already enabled starts it then.
//!
//! ```ignore
//! struct Speaker { /* write offset, dither state */ }
//! impl IsoOutConsumer for Speaker {
//!     fn on_packet(&mut self, pcm: &[u8]) { /* convert into the audio ring */ }
//! }
//! static mut SPEAKER: Speaker = Speaker { .. };
//! let stream = ep_out.into_iso_stream(unsafe { &mut *core::ptr::addr_of_mut!(SPEAKER) });
//! ```

use core::cell::Cell;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_usb_driver::EndpointType;

use super::driver::{Rusb1EndpointIn, Rusb1EndpointOut};
use super::fifo::{
    fifo_bclr, fifo_bval, fifo_dtln, fifo_for_pipe, fifo_is_ready, fifo_select_pipe,
    hw_to_sw_fifo, sw_to_hw_fifo,
};
use super::pipe::{PIPE_COUNT, pipe_brdy_disable, pipe_iso_in_activate};
use super::regs::{PIPECTR_PID_BUF, PIPECTR_PID_MASK, pipectr, usb};
use crate::pac::usb::Usb;

/// Receives an iso OUT endpoint's packets, in the USB interrupt (which only a higher-priority
/// interrupt can preempt): keep it short.
pub trait IsoOutConsumer: Send {
    /// One received packet (never empty).
    fn on_packet(&mut self, packet: &[u8]);
}

/// Supplies an iso IN endpoint's packets, in the USB interrupt (which only a higher-priority
/// interrupt can preempt), and twice in a critical section when the endpoint is enabled, to prime
/// both FIFO planes. Keep it short.
pub trait IsoInProducer: Send {
    /// Fill the next packet into `buf` (its length is the endpoint's max packet size) and return
    /// its length; 0 sends a zero-length packet. A longer return is clamped to `buf.len()`.
    fn fill_packet(&mut self, buf: &mut [u8]) -> usize;
}

/// The largest high-speed isochronous packet.
const SCRATCH_LEN: usize = 1024;

#[derive(Clone, Copy)]
enum Binding {
    None,
    Out(*mut dyn IsoOutConsumer),
    In(*mut dyn IsoInProducer, u16),
}

// SAFETY: the pointers come from `&'static mut` consumers, which are `Send`, and are only
// dereferenced by the port's USB interrupt or with interrupts masked.
unsafe impl Send for Binding {}

type Slot = critical_section::Mutex<Cell<Binding>>;

static BINDINGS: [[Slot; PIPE_COUNT]; 2] =
    [const { [const { critical_section::Mutex::new(Cell::new(Binding::None)) }; PIPE_COUNT] }; 2];

/// OUT packets dropped for not fitting the scratch buffer, per port and pipe.
static BAD_PACKETS: [[AtomicU32; PIPE_COUNT]; 2] =
    [const { [const { AtomicU32::new(0) }; PIPE_COUNT] }; 2];

/// Per port: where a packet is drained to, or filled in, before the FIFO copy. Touched only by
/// that port's interrupt (or with it masked).
static mut OUT_SCRATCH: [[u8; SCRATCH_LEN]; 2] = [[0; SCRATCH_LEN]; 2];
static mut IN_SCRATCH: [[u8; SCRATCH_LEN]; 2] = [[0; SCRATCH_LEN]; 2];

/// Which port `u` is.
pub(crate) fn port_of(u: Usb) -> usize {
    if u.as_ptr() == usb(1).as_ptr() { 1 } else { 0 }
}

fn binding(port: usize, pipe: usize) -> Binding {
    critical_section::with(|cs| BINDINGS[port][pipe].borrow(cs).get())
}

fn bind(port: u8, pipe: u8, b: Binding) {
    critical_section::with(|cs| {
        let slot = BINDINGS[port as usize][pipe as usize].borrow(cs);
        assert!(matches!(slot.get(), Binding::None), "usb: pipe {} already has an iso stream", pipe);
        slot.set(b);
    });
}

/// Mask the pipe's BRDY and unbind it, at once.
fn unbind(port: u8, pipe: u8) {
    critical_section::with(|cs| {
        // SAFETY: masking one pipe's interrupt; the binding goes in the same critical section.
        unsafe { pipe_brdy_disable(usb(port), pipe as usize) };
        BINDINGS[port as usize][pipe as usize].borrow(cs).set(Binding::None);
    });
}

/// Whether an IN stream is bound to `pipe` on `port`.
pub(crate) fn in_bound(port: u8, pipe: usize) -> bool {
    matches!(binding(port as usize, pipe), Binding::In(..))
}

/// Drain pipe `n`'s received packet to its bound consumer, if it has one: BCLR always, so the
/// SIE can take the next packet. Returns whether a consumer is bound (false leaves the packet
/// for a task's `read()`).
///
/// # Safety
/// The port's USB interrupt, or with it masked.
pub(crate) unsafe fn out_brdy(u: Usb, n: usize) -> bool {
    let port = port_of(u);
    let Binding::Out(consumer) = binding(port, n) else {
        return false;
    };
    // SAFETY: the caller's contract; the FIFO helpers touch only this pipe's port.
    unsafe {
        let fifo = fifo_for_pipe(u, n);
        fifo_select_pipe(&fifo, n, false);
        if fifo_is_ready(&fifo, n) {
            let len = fifo_dtln(&fifo) as usize;
            if len > SCRATCH_LEN {
                BAD_PACKETS[port][n].fetch_add(1, Ordering::Relaxed);
                fifo_bclr(&fifo);
            } else {
                let scratch: &mut [u8; SCRATCH_LEN] = &mut (&mut *core::ptr::addr_of_mut!(OUT_SCRATCH))[port];
                if len > 0 {
                    hw_to_sw_fifo(&fifo, scratch.as_mut_ptr(), len);
                }
                fifo_bclr(&fifo);
                if len > 0 {
                    (*consumer).on_packet(&scratch[..len]);
                }
            }
        }
    }
    true
}

/// Hand a packet a task's `read()` received on pipe `n` to its bound consumer too.
///
/// # Safety
/// The port's USB interrupt, or with it masked.
pub(crate) unsafe fn out_after_read(u: Usb, n: usize, packet: &[u8]) {
    if let Binding::Out(consumer) = binding(port_of(u), n)
        && !packet.is_empty()
    {
        // SAFETY: the caller's contract.
        unsafe { (*consumer).on_packet(packet) };
    }
}

/// Stage pipe `n`'s next IN packet from its bound producer and commit it (BVAL when short, a
/// zero-length packet included), leaving the pipe armed. Nothing if no producer is bound or the
/// FIFO isn't ready.
///
/// # Safety
/// The port's USB interrupt, or with it masked.
pub(crate) unsafe fn in_brdy(u: Usb, n: usize) {
    let port = port_of(u);
    let Binding::In(producer, mps) = binding(port, n) else {
        return;
    };
    let mps = (mps as usize).min(SCRATCH_LEN);
    // SAFETY: the caller's contract.
    unsafe {
        let fifo = fifo_for_pipe(u, n);
        fifo_select_pipe(&fifo, n, true);
        if !fifo_is_ready(&fifo, n) {
            return;
        }
        let scratch: &mut [u8] = &mut (&mut *core::ptr::addr_of_mut!(IN_SCRATCH))[port][..mps];
        let len = (*producer).fill_packet(scratch).min(mps);
        if len > 0 {
            sw_to_hw_fifo(&fifo, scratch.as_ptr(), len);
        }
        if len < mps {
            fifo_bval(&fifo);
        }
    }
}

/// An iso OUT endpoint bound to its [`IsoOutConsumer`]. Dropping it unbinds the consumer.
pub struct IsoOutStream {
    port: u8,
    pipe: u8,
}

impl IsoOutStream {
    /// The pipe it runs on.
    pub fn pipe(&self) -> u8 {
        self.pipe
    }

    /// Packets dropped for being longer than the 1024-byte limit.
    pub fn bad_packets(&self) -> u32 {
        BAD_PACKETS[self.port as usize][self.pipe as usize].load(Ordering::Relaxed)
    }
}

impl Drop for IsoOutStream {
    fn drop(&mut self) {
        unbind(self.port, self.pipe);
    }
}

/// An iso IN endpoint bound to its [`IsoInProducer`]. Dropping it unbinds the producer.
pub struct IsoInStream {
    port: u8,
    pipe: u8,
}

impl IsoInStream {
    /// The pipe it runs on.
    pub fn pipe(&self) -> u8 {
        self.pipe
    }
}

impl Drop for IsoInStream {
    fn drop(&mut self) {
        unbind(self.port, self.pipe);
    }
}

/// Bind `consumer` to `pipe` on `port` (what [`Rusb1EndpointOut::into_iso_stream`] does).
pub(crate) fn bind_out(port: u8, pipe: u8, consumer: &'static mut dyn IsoOutConsumer) {
    bind(port, pipe, Binding::Out(consumer));
}

/// Bind `producer` to `pipe` on `port` with `mps`-byte packets.
pub(crate) fn bind_in(port: u8, pipe: u8, mps: u16, producer: &'static mut dyn IsoInProducer) {
    bind(port, pipe, Binding::In(producer, mps));
}

impl Rusb1EndpointOut {
    /// Hand every packet this iso endpoint receives to `consumer`, in the USB interrupt.
    ///
    /// # Panics
    /// If the endpoint isn't isochronous, or its pipe already has a stream.
    pub fn into_iso_stream(self, consumer: &'static mut dyn IsoOutConsumer) -> IsoOutStream {
        assert!(self.info.ep_type == EndpointType::Isochronous, "usb: into_iso_stream needs an isochronous endpoint");
        bind_out(self.port, self.pipe, consumer);
        IsoOutStream { port: self.port, pipe: self.pipe }
    }
}

impl Rusb1EndpointIn {
    /// Send the packets `producer` fills on this iso endpoint, from the USB interrupt. Both FIFO
    /// planes are primed when the host enables the endpoint.
    ///
    /// # Panics
    /// If the endpoint isn't isochronous, or its pipe already has a stream.
    pub fn into_iso_stream(self, producer: &'static mut dyn IsoInProducer) -> IsoInStream {
        assert!(self.info.ep_type == EndpointType::Isochronous, "usb: into_iso_stream needs an isochronous endpoint");
        bind_in(self.port, self.pipe, self.info.max_packet_size, producer);
        // Bound after the host enabled the endpoint: start it now, as the enable would have.
        let u = usb(self.port);
        if pipectr(u, self.pipe as usize).read() & PIPECTR_PID_MASK == PIPECTR_PID_BUF {
            // SAFETY: this endpoint's pipe; the activation runs in a critical section.
            unsafe { pipe_iso_in_activate(u, self.pipe as usize) };
        }
        IsoInStream { port: self.port, pipe: self.pipe }
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use std::boxed::Box;
    use super::*;
    use crate::mmio::test;
    use crate::usb::driver::{Rusb1EndpointIn, Rusb1EndpointOut};
    use crate::usb::regs::{FIFOCTR_BCLR, FIFOCTR_BVAL, FIFOCTR_FRDY, usb};
    use embassy_usb_driver::{Direction, EndpointAddress, EndpointInfo, EndpointType};

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn info(dir: Direction, ep_type: EndpointType, mps: u16) -> EndpointInfo {
        EndpointInfo { addr: EndpointAddress::from_parts(1, dir), ep_type, max_packet_size: mps, interval_ms: 1 }
    }

    fn out_ep(pipe: u8) -> Rusb1EndpointOut {
        Rusb1EndpointOut { port: 0, pipe, info: info(Direction::Out, EndpointType::Isochronous, 288) }
    }

    fn in_ep(pipe: u8, mps: u16) -> Rusb1EndpointIn {
        Rusb1EndpointIn { port: 0, pipe, info: info(Direction::In, EndpointType::Isochronous, mps), buf_bytes: mps }
    }

    /// A packet of `len` bytes, each 32-bit FIFO read returning `word`, waits in D0FIFO.
    fn packet(word: u32, len: u16) {
        let u = usb(0);
        test::poke32(u.d0fifo().addr(), word);
        test::poke16(u.d0fifoctr().addr(), FIFOCTR_FRDY | len);
    }

    fn fifo_reads() -> usize {
        let a = usb(0).d0fifo().addr();
        test::log().iter().filter(|x| !x.write && x.addr == a).count()
    }

    fn fifo_writes() -> usize {
        let a = usb(0).d0fifo().addr();
        test::log().iter().filter(|x| x.write && x.addr == a).count()
    }

    fn ctr_written(bit: u16) -> bool {
        let a = usb(0).d0fifoctr().addr();
        test::log().iter().any(|x| x.write && x.addr == a && x.val as u16 & bit != 0)
    }

    #[derive(Default)]
    struct Recorder {
        calls: u32,
        last: (usize, u8),
    }
    impl IsoOutConsumer for Recorder {
        fn on_packet(&mut self, packet: &[u8]) {
            self.calls += 1;
            self.last = (packet.len(), packet[0]);
        }
    }

    struct Filler {
        offered: usize,
        give: usize,
    }
    impl IsoInProducer for Filler {
        fn fill_packet(&mut self, buf: &mut [u8]) -> usize {
            self.offered = buf.len();
            buf.fill(0xAB);
            self.give
        }
    }

    fn recorder() -> &'static mut Recorder {
        Box::leak(Box::new(Recorder::default()))
    }

    fn filler(give: usize) -> &'static mut Filler {
        Box::leak(Box::new(Filler { offered: 0, give }))
    }

    #[test]
    fn an_out_stream_hands_each_packet_to_its_consumer() {
        let _serial = serial();
        test::reset();
        let rec = recorder();
        let rec_ptr: *const Recorder = rec;
        let _stream = out_ep(1).into_iso_stream(rec);
        packet(0x0403_0201, 288);
        assert!(unsafe { out_brdy(usb(0), 1) });
        let rec = unsafe { &*rec_ptr };
        assert_eq!((rec.calls, rec.last), (1, (288, 0x01)));
        assert!(ctr_written(FIFOCTR_BCLR), "the bank is released");
    }

    #[test]
    fn an_oversized_out_packet_is_dropped_not_overflowed() {
        let _serial = serial();
        test::reset();
        let rec = recorder();
        let rec_ptr: *const Recorder = rec;
        let stream = out_ep(1).into_iso_stream(rec);
        packet(0x0403_0201, 1100);
        unsafe { out_brdy(usb(0), 1) };
        assert_eq!(unsafe { &*rec_ptr }.calls, 0);
        assert_eq!(stream.bad_packets(), 1);
        assert!(ctr_written(FIFOCTR_BCLR));
        assert_eq!(fifo_reads(), 0, "nothing read into the 1024-byte scratch");
    }

    #[test]
    fn an_in_stream_fills_from_its_producer_and_commits_short_packets() {
        let _serial = serial();
        test::reset();
        let _stream = in_ep(1, 294).into_iso_stream(filler(288));
        test::poke16(usb(0).d0fifoctr().addr(), FIFOCTR_FRDY);
        unsafe { in_brdy(usb(0), 1) };
        assert_eq!(fifo_writes(), 288 / 4);
        assert!(ctr_written(FIFOCTR_BVAL), "a short packet is committed");
    }

    #[test]
    fn a_full_size_capture_packet_is_not_clamped() {
        let _serial = serial();
        test::reset();
        let f = filler(882);
        let f_ptr: *const Filler = f;
        let _stream = in_ep(1, 882).into_iso_stream(f);
        test::poke16(usb(0).d0fifoctr().addr(), FIFOCTR_FRDY);
        unsafe { in_brdy(usb(0), 1) };
        assert_eq!(unsafe { &*f_ptr }.offered, 882);
    }

    #[test]
    fn a_producer_overlong_return_is_clamped() {
        let _serial = serial();
        test::reset();
        let _stream = in_ep(1, 294).into_iso_stream(filler(5000));
        test::poke16(usb(0).d0fifoctr().addr(), FIFOCTR_FRDY);
        unsafe { in_brdy(usb(0), 1) };
        // 294 bytes: 73 words, then a 2-byte tail through the byte lanes (D0FIFO + 3, + 2).
        assert_eq!(fifo_writes(), 73);
        let lanes = [usb(0).d0fifo().addr() + 3, usb(0).d0fifo().addr() + 2];
        let tail = test::log().iter().filter(|x| x.write && lanes.contains(&x.addr)).count();
        assert_eq!(tail, 2);
        assert!(!ctr_written(FIFOCTR_BVAL), "a full packet needs no BVAL");
    }

    #[test]
    fn a_dropped_out_stream_is_never_called_again() {
        let _serial = serial();
        test::reset();
        let rec = recorder();
        let rec_ptr: *const Recorder = rec;
        let stream = out_ep(1).into_iso_stream(rec);
        test::poke16(usb(0).brdyenb().addr(), 1 << 1);
        drop(stream);
        assert_eq!(test::peek16(usb(0).brdyenb().addr()) & (1 << 1), 0, "BRDY masked");
        packet(0x0403_0201, 288);
        assert!(!unsafe { out_brdy(usb(0), 1) });
        assert_eq!(unsafe { &*rec_ptr }.calls, 0);
    }

    #[test]
    #[should_panic(expected = "isochronous")]
    fn binding_a_bulk_endpoint_panics() {
        let _serial = serial();
        let ep = Rusb1EndpointOut { port: 0, pipe: 3, info: info(Direction::Out, EndpointType::Bulk, 512) };
        let _ = ep.into_iso_stream(recorder());
    }

    #[test]
    fn streams_are_per_port() {
        let _serial = serial();
        test::reset();
        let _stream = in_ep(1, 294).into_iso_stream(filler(0));
        assert!(in_bound(0, 1));
        assert!(!in_bound(1, 1));
    }
}
