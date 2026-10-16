//! RUSB1 USB modules for the RZ/A1L (HW manual §28).
//!
//! Provides the chip-level USB plumbing (GIC interrupt IDs,
//! STBCR7 clock gates) plus full `embassy-usb-driver` implementations for
//! both directions of the dual-role RUSB1 controller:
//!
//! - [`driver`] — device mode (`embassy_usb_driver::Driver`):
//!   [`Rusb1Driver`], [`Rusb1Bus`], [`Rusb1ControlPipe`], endpoints.
//! - [`host`] — host mode (`embassy_usb_driver::host::UsbHostController`):
//!   [`Rusb1HostDriver`], [`Rusb1Allocator`], [`Rusb1Pipe`].
//! - [`pipe`], [`fifo`], [`regs`] — shared pipe/FIFO/register layers.
//!
//! USB *class* implementations (UAC2 audio, MIDI, MSC) and the Bulk-Only
//! Transport engine live in the board-support crate (`deluge_bsp::usb`),
//! which re-exports this module.
//!
//! ## Quick start — device mode
//!
//! ```ignore
//! use rza1l_hal::usb::{self, init_device_mode};
//! use rza1l_hal::peripherals::USB0;
//!
//! rza1l_hal::bind_interrupts!(struct Irqs { USB0 => usb::DeviceInterruptHandler<USB0>; });
//! let (port, driver) = init_device_mode(p.USB0, Irqs);
//! // pass `driver` to `embassy_usb::UsbDevice::new(driver, config, ...)`
//! ```
//!
//! ## Quick start — host mode
//!
//! ```ignore
//! use rza1l_hal::usb::{self, init_host_mode};
//! use rza1l_hal::peripherals::USB0;
//! use embassy_usb_host::{bus, BusState, handler::BusRoute};
//!
//! rza1l_hal::bind_interrupts!(struct Irqs { USB0 => usb::HostInterruptHandler<USB0>; });
//! static BUS_STATE: BusState = BusState::new();
//! let (port, host) = init_host_mode(p.USB0, Irqs);
//! let (mut controller, handle) = bus(host, &BUS_STATE);
//! let speed = controller.wait_for_connection().await;
//! let (info, _) = handle.enumerate(BusRoute::Direct(speed), &mut [0u8; 256]).await.unwrap();
//! ```
//!
//! ## ISR wiring
//!
//! Bind the port's interrupt to the handler for the mode you run, and pass the binding to
//! `init_device_mode` / `init_host_mode`:
//!
//! ```ignore
//! rza1l_hal::bind_interrupts!(struct Irqs { USB0 => usb::DeviceInterruptHandler<USB0>; });
//! let (_port, driver) = usb::init_device_mode(p.USB0, Irqs);
//! ```
//!
//! Firmware that picks device or host mode at run time binds `DualRoleInterruptHandler`,
//! which dispatches to the mode the port was last brought up in.

pub mod driver;
pub mod fifo;
pub mod host;
pub mod pipe;
pub mod regs;

pub use driver::{
    Rusb1Bus, Rusb1ControlPipe, Rusb1Driver, Rusb1EndpointIn, Rusb1EndpointOut, dcd_int_handler,
};
pub use host::{Rusb1Allocator, Rusb1HostDriver, Rusb1Pipe, hcd_int_handler};

use core::marker::PhantomData;

use crate::pac::CPG;
use crate::pac::cpg::regs::Stbcr7;

// ---------------------------------------------------------------------------
// Registers, interrupt IDs, module clock (chip-level plumbing)
// ---------------------------------------------------------------------------

// The two channels' registers are `crate::pac::USB200` (H'E8010000) and `USB201` (H'E8207000),
// through [`regs::usb`]: identical USB 2.0 HS host/function modules, except that the
// UCKSEL/UPLLE clock bits exist only in channel 0's SYSCFG0.

/// GIC interrupt ID for USB0 (USB200).
pub const USB0_IRQ: u16 = 73;
/// GIC interrupt ID for USB1 (USB201).
pub const USB1_IRQ: u16 = 74;

/// Return the GIC interrupt ID for the given port.
#[inline]
pub const fn irq(port: u8) -> u16 {
    USB0_IRQ + port as u16
}

// CPG Standby Control Register 7 — USB clock gates.
// Bit 1: MSTP71 — channel 0 clock (0 = running, 1 = stopped)
// Bit 0: MSTP70 — channel 1 clock (0 = running, 1 = stopped)
const STBCR7_MSTP71: u8 = 1 << 1; // channel 0
const STBCR7_MSTP70: u8 = 1 << 0; // channel 1

/// Enable the clock for USB module `port` (0 or 1) in CPG STBCR7.
///
/// Channel 0 hosts the USB PLL (UPLLE/UCKSEL) shared by both channels, so
/// bringing up channel 1 (port 1) requires clocking channel 0 as well.  Per HW
/// manual §42.2.7 procedure (2): "Set MSTP71 to 0 … Note: When channel 1 is to
/// be used, also set the MSTP70 bit to 0."  Clearing only MSTP70 for port 1
/// would leave channel 0 gated, so the driver's UPLLE write to channel 0's
/// SYSCFG0 lands on an unclocked register block and the PLL never enables.
///
/// # Safety
/// Writes to memory-mapped CPG registers. Must not be called concurrently
/// with other STBCR7 writers.
pub unsafe fn module_clock_enable(port: u8) {
    let bits: u8 = if port == 0 {
        STBCR7_MSTP71 // ch0 only
    } else {
        STBCR7_MSTP71 | STBCR7_MSTP70 // ch1 also needs ch0 (shared PLL)
    };
    let stbcr7 = CPG.stbcr7();
    stbcr7.write_value(Stbcr7(stbcr7.read().0 & !bits));
    // Dummy read to flush write buffer (required by HW manual §10).
    let _ = stbcr7.read();
}

/// Stop the clock for USB module `port` in CPG STBCR7.
///
/// # Safety
/// Writes to memory-mapped CPG registers.
pub unsafe fn module_clock_disable(port: u8) {
    // Stop only this port's own channel.  Port 1 must NOT gate channel 0
    // (MSTP71): channel 0 hosts the shared PLL and may still serve port 0.
    let bit: u8 = if port == 0 {
        STBCR7_MSTP71
    } else {
        STBCR7_MSTP70
    };
    let stbcr7 = CPG.stbcr7();
    stbcr7.write_value(Stbcr7(stbcr7.read().0 | bit));
    let _ = stbcr7.read();
}

/// GIC interrupt priority for USB0/USB1.
///
/// Must be numerically less than 31 (the GICC_PMR threshold) to pass the
/// CPU-interface priority filter.  Matches the priority used by UART/SDHI.
const USB_IRQ_PRIORITY: u8 = 10;

/// Enable the GIC interrupt for USB module `port`.
///
/// Sets the interrupt priority before enabling, so the CPU-interface
/// priority filter (GICC_PMR = 31) does not block delivery.
///
/// # Safety
/// Writes to GIC distributor registers.
pub unsafe fn int_enable(port: u8) {
    unsafe {
        crate::gic::set_priority(irq(port), USB_IRQ_PRIORITY);
        crate::gic::enable(irq(port));
    }
}

/// Disable the GIC interrupt for USB module `port`.
///
/// # Safety
/// Writes to GIC distributor registers.
pub unsafe fn int_disable(port: u8) {
    unsafe {
        crate::gic::disable(irq(port));
    }
}

// ---------------------------------------------------------------------------
// Typed ports and interrupt handlers
// ---------------------------------------------------------------------------

use core::sync::atomic::{AtomicBool, Ordering};

use crate::Peri;
use crate::interrupt::{self, Binding, Handler, Interrupt};

mod sealed {
    pub trait Sealed {}
}

/// A USB port singleton (`peripherals::USB0`, `USB1`).
pub trait Instance: crate::PeripheralType + sealed::Sealed + 'static {
    /// The port number, 0 or 1.
    const PORT: u8;
    /// Its interrupt.
    type Interrupt: Interrupt;
}

impl sealed::Sealed for crate::peripherals::USB0 {}
impl Instance for crate::peripherals::USB0 {
    const PORT: u8 = 0;
    type Interrupt = crate::interrupt::typelevel::USB0;
}
impl sealed::Sealed for crate::peripherals::USB1 {}
impl Instance for crate::peripherals::USB1 {
    const PORT: u8 = 1;
    type Interrupt = crate::interrupt::typelevel::USB1;
}

/// Whether each port was last initialised in host mode: the dual-role handler's dispatch.
static HOST_MODE: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];

fn set_host_mode(port: u8, host: bool) {
    HOST_MODE[port as usize].store(host, Ordering::Release);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Device,
    Host,
}

fn role(port: u8) -> Role {
    if HOST_MODE[port as usize].load(Ordering::Acquire) {
        Role::Host
    } else {
        Role::Device
    }
}

/// Port `T`'s interrupt in device mode.
pub struct DeviceInterruptHandler<T: Instance> {
    _usb: PhantomData<T>,
}
impl<T: Instance> Handler<T::Interrupt> for DeviceInterruptHandler<T> {
    unsafe fn on_interrupt() {
        // SAFETY: the GIC dispatches this for T's interrupt.
        unsafe { dcd_int_handler(T::PORT) }
    }
}

/// Port `T`'s interrupt in host mode.
pub struct HostInterruptHandler<T: Instance> {
    _usb: PhantomData<T>,
}
impl<T: Instance> Handler<T::Interrupt> for HostInterruptHandler<T> {
    unsafe fn on_interrupt() {
        // SAFETY: as above.
        unsafe { hcd_int_handler(T::PORT) }
    }
}

/// Port `T`'s interrupt for firmware that picks device or host mode at run time: dispatches to
/// the mode the port was last initialised in.
pub struct DualRoleInterruptHandler<T: Instance> {
    _usb: PhantomData<T>,
}
impl<T: Instance> Handler<T::Interrupt> for DualRoleInterruptHandler<T> {
    unsafe fn on_interrupt() {
        // SAFETY: as above; the mode is the one `init_*_mode` recorded.
        unsafe {
            match role(T::PORT) {
                Role::Host => hcd_int_handler(T::PORT),
                Role::Device => dcd_int_handler(T::PORT),
            }
        }
    }
}

/// A handler that serves port `T` in device mode.
pub trait ServesDevice<T: Instance>: Handler<T::Interrupt> + sealed::Sealed {}
/// A handler that serves port `T` in host mode.
pub trait ServesHost<T: Instance>: Handler<T::Interrupt> + sealed::Sealed {}

impl<T: Instance> sealed::Sealed for DeviceInterruptHandler<T> {}
impl<T: Instance> sealed::Sealed for HostInterruptHandler<T> {}
impl<T: Instance> sealed::Sealed for DualRoleInterruptHandler<T> {}
impl<T: Instance> ServesDevice<T> for DeviceInterruptHandler<T> {}
impl<T: Instance> ServesDevice<T> for DualRoleInterruptHandler<T> {}
impl<T: Instance> ServesHost<T> for HostInterruptHandler<T> {}
impl<T: Instance> ServesHost<T> for DualRoleInterruptHandler<T> {}

// ---------------------------------------------------------------------------
// Mode markers
// ---------------------------------------------------------------------------

/// Mode marker for device (peripheral) operation.
pub struct Device;

/// Mode marker for host operation.
pub struct Host;

// ---------------------------------------------------------------------------
// UsbPort handle
// ---------------------------------------------------------------------------

/// A handle representing ownership of one RUSB1 port in a given `MODE`.
///
/// Created by [`init_device_mode`] or [`init_host_mode`]; not constructible
/// directly.  Dropping this type does not disable the hardware.
pub struct UsbPort<MODE> {
    port: u8,
    _mode: PhantomData<MODE>,
}

impl<M> UsbPort<M> {
    /// The hardware port index (0 or 1).
    pub fn port_index(&self) -> u8 {
        self.port
    }
}

// ---------------------------------------------------------------------------
// Convenience initialisation functions
// ---------------------------------------------------------------------------

/// Bring port `T` up in **device** mode: module clock, then the RUSB1 device driver for
/// `embassy-usb`. `irq` binds a handler that serves device mode; the driver unmasks the
/// interrupt when it starts.
///
/// The driver borrows `usb` for `'d`, so the port can't be brought up again while it lives.
/// Firmware that brings it up once per session [`disconnect`]s the previous session first.
/// The endpoints it hands out carry no lifetime: drop them with the driver.
///
/// There is no live mode switch. To bring a port up in the other mode, drop the old driver
/// and [`disconnect`] the port first: an interrupt the old mode left pending would otherwise
/// reach the new mode's handler under [`DualRoleInterruptHandler`].
pub fn init_device_mode<'d, T: Instance, H: ServesDevice<T>>(
    _usb: Peri<'d, T>,
    irq: impl Binding<T::Interrupt, H>,
) -> (UsbPort<Device>, Rusb1Driver<'d>) {
    interrupt::bind(irq);
    set_host_mode(T::PORT, false);
    // SAFETY: port T is owned for 'd; its clock and driver are this call's.
    unsafe {
        module_clock_enable(T::PORT);
        (
            UsbPort {
                port: T::PORT,
                _mode: PhantomData,
            },
            Rusb1Driver::new(T::PORT),
        )
    }
}

/// Bring port `T` up in **host** mode: module clock, then the RUSB1 host driver for
/// `embassy-usb-host`, which unmasks the interrupt. `irq` binds a handler that serves host mode.
///
/// As with [`init_device_mode`], a port that ran in device mode is [`disconnect`]ed, its driver
/// dropped, before it comes up here.
pub fn init_host_mode<'d, T: Instance, H: ServesHost<T>>(
    _usb: Peri<'d, T>,
    irq: impl Binding<T::Interrupt, H>,
) -> (UsbPort<Host>, Rusb1HostDriver<'d>) {
    interrupt::bind(irq);
    set_host_mode(T::PORT, true);
    // SAFETY: as above.
    unsafe {
        module_clock_enable(T::PORT);
        (
            UsbPort {
                port: T::PORT,
                _mode: PhantomData,
            },
            Rusb1HostDriver::new(T::PORT),
        )
    }
}

/// Disconnect a device-mode port from the host and stop its SIE: the D+ pull-up off (the host
/// sees an unplug), USB interrupts masked, `USBE` cleared. A later [`init_device_mode`]
/// re-attaches.
pub fn disconnect<T: Instance>(_usb: Peri<'_, T>) {
    // SAFETY: port T is owned; its old driver's session is over.
    unsafe {
        use regs::{SYSCFG_DPRPU, SYSCFG_USBE};
        let port = T::PORT;
        int_disable(port);
        let u = regs::usb(port);
        // Pull-up off: host sees a disconnect.
        u.syscfg0().modify(|r| *r &= !SYSCFG_DPRPU);
        u.intenb(0).write_value(0);
        u.brdyenb().write_value(0);
        u.bempenb().write_value(0);
        // Clear USBE to reset the SIE.
        let cur = u.syscfg0().read();
        u.syscfg0().write_value(cur & !SYSCFG_USBE);
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn irq_ids() {
        assert_eq!(irq(0), 73);
        assert_eq!(irq(1), 74);
    }

    #[test]
    fn dual_role_dispatch_follows_the_recorded_mode() {
        set_host_mode(1, false);
        assert_eq!(role(1), Role::Device);
        set_host_mode(1, true);
        assert_eq!(role(1), Role::Host);
        set_host_mode(1, false);
        assert_eq!(role(1), Role::Device);
    }
}
