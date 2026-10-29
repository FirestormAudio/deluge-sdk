//! GPIO port registers for the RZ/A1L (see HW Manual §54), from rza1-pac.
//!
//! Port numbering mirrors the hardware: the RZ/A1L has ports 0–9, and pin functions use
//! ports 1–9 (port 0 is input-only). PMC and PIBC include port 0, so `pmc(0)` is valid.
//!
//! ## Register map summary (all 16-bit, stride 4, indexed by port)
//! - Pn    (output data)           : `0xFCFE_3000 + n*4`
//! - PMn   (mode: 0=out, 1=in)     : `0xFCFE_3300 + n*4`
//! - PMCn  (mux: 0=GPIO, 1=periph) : `0xFCFE_3400 + n*4`
//! - PIPCn (ctrl: 0=sw, 1=periph)  : `0xFCFE_7200 + n*4`

use core::convert::Infallible;
use core::marker::PhantomData;

use crate::pac::GPIO;
use crate::pac::common::{R, RW, Reg};
use crate::{Peri, PeripheralType};

mod pins;

/// One row of the generated pin-function table: `$pin` carries `$instance`'s `$trait` signal on
/// alternative `$alt`.
#[cfg(feature = "rza1l")]
macro_rules! pin_function {
    ($($trait:ident)::+, $instance:ident, $pin:ident, $alt:ident) => {
        impl $($trait)::+<crate::peripherals::$instance> for crate::peripherals::$pin {
            fn alternate(&self) -> Alternate {
                Alternate::$alt
            }
        }
    };
}

// The RZ/A1L's pin functions, generated from its manual by rza1/tools/gen-pins.py.
#[cfg(feature = "rza1l")]
include!("rza1l_functions.rs");

// ── Registers ──────────────────────────────────────────────────────────────────

/// Port Mode Register n (PMn): bit=0 → output, bit=1 → input. `port` is 1–9.
fn pm(port: u8) -> Reg<u16, RW> {
    GPIO.pm(port as usize)
}

/// Port Mode Control Register n (PMCn): bit=0 → GPIO, bit=1 → peripheral. `port` is 0–9.
fn pmc(port: u8) -> Reg<u16, RW> {
    GPIO.pmc(port as usize)
}

/// Port Data Register n (Pn) — output data. `port` is 1–9.
fn p(port: u8) -> Reg<u16, RW> {
    GPIO.p(port as usize)
}

/// Port Input Peripheral Control Register n (PIPCn): bit=0 → software ctrl. Must be 0 for GPIO
/// output. `port` is 1–9.
fn pipc(port: u8) -> Reg<u16, RW> {
    GPIO.pipc(port as usize)
}

/// Port Pin Read register n (PPRn) — reflects actual pin voltage regardless of direction.
/// Read-only. `port` is 1–9.
fn ppr(port: u8) -> Reg<u16, R> {
    GPIO.ppr(port as usize)
}

/// Port Input Buffer Control register n (PIBCn): bit=1 enables the input buffer. Must be 1 to
/// read external pin state via PPR. `port` is 0–9 (PIBC0 exists).
fn pibc(port: u8) -> Reg<u16, RW> {
    GPIO.pibc(port as usize)
}

// ── Owned pins ─────────────────────────────────────────────────────────────────

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// A pin's logic level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Level {
    /// Low.
    Low,
    /// High.
    High,
}

impl From<bool> for Level {
    fn from(high: bool) -> Self {
        if high { Level::High } else { Level::Low }
    }
}

impl From<Level> for bool {
    fn from(level: Level) -> bool {
        level == Level::High
    }
}

/// An alternative (peripheral) function of a pin: the function tables' 1st to 8th alternative
/// (HW manual Table 41.6, set through PFCn, PFCEn and PFCAEn).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[allow(missing_docs)]
pub enum Alternate {
    Alt1 = 1,
    Alt2,
    Alt3,
    Alt4,
    Alt5,
    Alt6,
    Alt7,
    Alt8,
}

/// A GPIO port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[allow(missing_docs)]
pub enum Port {
    P0 = 0,
    P1,
    P2,
    P3,
    P4,
    P5,
    P6,
    P7,
    P8,
    P9,
}

impl Port {
    const ALL: [Port; 10] = [
        Port::P0,
        Port::P1,
        Port::P2,
        Port::P3,
        Port::P4,
        Port::P5,
        Port::P6,
        Port::P7,
        Port::P8,
        Port::P9,
    ];

    /// Port `n` (0–9).
    pub(crate) const fn from_index(n: u8) -> Self {
        Self::ALL[n as usize]
    }
}

/// A pin singleton (`peripherals::P6_7` …) or a type-erased [`AnyPin`].
pub trait Pin: PeripheralType + sealed::Sealed + Into<AnyPin> + 'static {
    /// Its port.
    fn port(&self) -> Port;
    /// Its bit in the port, 0–15.
    fn bit(&self) -> u8;
    /// The pin as an [`AnyPin`].
    fn degrade(self) -> AnyPin {
        self.into()
    }
}

/// A pin with a port-mode output: every pin but P0_0–P0_3 and P1_8–P1_15. P1_0–P1_7 are
/// open-drain outputs.
pub trait OutputCapable: Pin {}

/// Any pin, its identity carried at run time.
#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct AnyPin {
    port: u8,
    bit: u8,
}

embassy_hal_internal::impl_peripheral!(AnyPin);

impl sealed::Sealed for AnyPin {}

impl Pin for AnyPin {
    #[inline]
    fn port(&self) -> Port {
        Port::from_index(self.port)
    }
    #[inline]
    fn bit(&self) -> u8 {
        self.bit
    }
}

/// Whether port `port` bit `bit` has a port-mode output (see [`OutputCapable`]).
const fn has_port_output(port: u8, bit: u8) -> bool {
    !(port == 0 || (port == 1 && bit >= 8))
}

/// A set/reset register's value that sets the bits of `mask` to `value`'s: the upper half selects
/// which lower bits are written (PSRn, PMSRn, PMCSRn).
const fn set_reset(mask: u16, value: bool) -> u32 {
    ((mask as u32) << 16) | if value { mask as u32 } else { 0 }
}

/// Read-modify-write of a register with no set/reset twin, in a critical section so an interrupt
/// touching another pin of the port can't undo it.
fn modify_bit(r: Reg<u16, RW>, mask: u16, set: bool) {
    critical_section::with(|_| {
        r.modify(|v| {
            if set {
                *v |= mask
            } else {
                *v &= !mask
            }
        })
    });
}

/// A pin that can be an input, an output or an alternative function, and change between them.
///
/// Dropping it changes nothing: the pin keeps its configuration (a board that hands a pin to a
/// long-lived owner, such as the SDRAM bus, must not have it released by a drop).
pub struct Flex<'d> {
    pin: Peri<'d, AnyPin>,
}

impl<'d> Flex<'d> {
    /// Take `pin`, leaving its configuration as it is.
    #[inline]
    pub fn new(pin: Peri<'d, impl Pin>) -> Self {
        Self { pin: pin.into() }
    }

    fn n(&self) -> usize {
        self.pin.port() as usize
    }

    fn mask(&self) -> u16 {
        1 << self.pin.bit()
    }

    /// A software-controlled input: port mode (PMC = 0), input direction (PM = 1), input buffer
    /// on (PIBC = 1).
    pub fn set_as_input(&mut self) {
        let (n, m) = (self.n(), self.mask());
        GPIO.pmcsr(n).write_value(set_reset(m, false));
        GPIO.pmsr(n).write_value(set_reset(m, true));
        modify_bit(GPIO.pibc(n), m, true);
    }

    /// A software-controlled output at `initial`: the level is written before the direction
    /// switches, so the pin never drives a stale value.
    ///
    /// # Panics
    /// If the pin has no port-mode output (P0_0–P0_3, P1_8–P1_15).
    pub fn set_as_output(&mut self, initial: Level) {
        let (port, bit) = (self.pin.port() as u8, self.pin.bit());
        if !has_port_output(port, bit) {
            panic!("P{}_{} has no port-mode output", port, bit);
        }
        let (n, m) = (self.n(), self.mask());
        GPIO.psr(n).write_value(set_reset(m, initial.into()));
        GPIO.pmcsr(n).write_value(set_reset(m, false));
        modify_bit(GPIO.pipc(n), m, false);
        GPIO.pmsr(n).write_value(set_reset(m, false));
    }

    fn select_alternate(&mut self, alt: Alternate) {
        let (n, m) = (self.n(), self.mask());
        let k = alt as u8 - 1;
        modify_bit(GPIO.pfcae(n), m, k >= 4);
        modify_bit(GPIO.pfce(n), m, (k >> 1) & 1 != 0);
        modify_bit(GPIO.pfc(n), m, k & 1 != 0);
        GPIO.pmcsr(n).write_value(set_reset(m, true));
    }

    /// Alternative function `alt`, its I/O direction driven by the function (PIPC = 1). Use
    /// [`set_as_alternate_software_io`](Self::set_as_alternate_software_io) for the functions of
    /// HW manual Table 41.7 (TIOCnx, SSITxD0/1/3, WDTOVF).
    pub fn set_as_alternate(&mut self, alt: Alternate) {
        self.select_alternate(alt);
        modify_bit(GPIO.pipc(self.n()), self.mask(), true);
    }

    /// Alternative function `alt` with its I/O direction set by PM, as HW manual Table 41.7
    /// requires for TIOCnx, SSITxD0/1/3 and WDTOVF (PIPC = 0; PM = 0 for `output`).
    pub fn set_as_alternate_software_io(&mut self, alt: Alternate, output: bool) {
        self.select_alternate(alt);
        let (n, m) = (self.n(), self.mask());
        modify_bit(GPIO.pipc(n), m, false);
        GPIO.pmsr(n).write_value(set_reset(m, !output));
    }

    /// Turn the input buffer on (PIBC = 1), so the level of a pin in an alternative function can
    /// be read.
    pub fn enable_input_buffer(&mut self) {
        modify_bit(GPIO.pibc(self.n()), self.mask(), true);
    }

    /// Whether the pin reads high (PPRn).
    #[inline]
    pub fn is_high(&self) -> bool {
        GPIO.ppr(self.n()).read() & self.mask() != 0
    }

    /// Whether the pin reads low (PPRn).
    #[inline]
    pub fn is_low(&self) -> bool {
        !self.is_high()
    }

    /// The pin's level.
    #[inline]
    pub fn get_level(&self) -> Level {
        self.is_high().into()
    }

    /// Drive `level` (PSRn).
    #[inline]
    pub fn set_level(&mut self, level: Level) {
        GPIO.psr(self.n()).write_value(set_reset(self.mask(), level.into()));
    }

    /// Drive high.
    #[inline]
    pub fn set_high(&mut self) {
        self.set_level(Level::High)
    }

    /// Drive low.
    #[inline]
    pub fn set_low(&mut self) {
        self.set_level(Level::Low)
    }

    /// Invert the driven level (PNOTn).
    #[inline]
    pub fn toggle(&mut self) {
        GPIO.pnot(self.n()).write_value(self.mask());
    }

    /// Whether the output is set high (Pn).
    #[inline]
    pub fn is_set_high(&self) -> bool {
        GPIO.p(self.n()).read() & self.mask() != 0
    }

    /// Whether the output is set low (Pn).
    #[inline]
    pub fn is_set_low(&self) -> bool {
        !self.is_set_high()
    }
}

/// A software-controlled input.
pub struct Input<'d>(Flex<'d>);

impl<'d> Input<'d> {
    /// Take `pin` and make it an input.
    pub fn new(pin: Peri<'d, impl Pin>) -> Self {
        let mut flex = Flex::new(pin);
        flex.set_as_input();
        Self(flex)
    }

    /// Whether the pin reads high.
    #[inline]
    pub fn is_high(&self) -> bool {
        self.0.is_high()
    }

    /// Whether the pin reads low.
    #[inline]
    pub fn is_low(&self) -> bool {
        self.0.is_low()
    }

    /// The pin's level.
    #[inline]
    pub fn get_level(&self) -> Level {
        self.0.get_level()
    }
}

/// A software-controlled output.
pub struct Output<'d>(Flex<'d>);

impl<'d> Output<'d> {
    /// Take `pin` and make it an output at `initial`.
    pub fn new(pin: Peri<'d, impl OutputCapable>, initial: Level) -> Self {
        let mut flex = Flex::new(pin);
        flex.set_as_output(initial);
        Self(flex)
    }

    /// Drive `level`.
    #[inline]
    pub fn set_level(&mut self, level: Level) {
        self.0.set_level(level)
    }

    /// Drive high.
    #[inline]
    pub fn set_high(&mut self) {
        self.0.set_high()
    }

    /// Drive low.
    #[inline]
    pub fn set_low(&mut self) {
        self.0.set_low()
    }

    /// Invert the driven level.
    #[inline]
    pub fn toggle(&mut self) {
        self.0.toggle()
    }

    /// Whether the output is set high.
    #[inline]
    pub fn is_set_high(&self) -> bool {
        self.0.is_set_high()
    }

    /// Whether the output is set low.
    #[inline]
    pub fn is_set_low(&self) -> bool {
        self.0.is_set_low()
    }
}

/// Put `pin` on alternative function `alt`, its direction driven by the function: what a driver
/// does with each pin it takes.
pub(crate) fn mux(pin: Peri<'_, impl Pin>, alt: Alternate) {
    Flex::new(pin).set_as_alternate(alt);
}

/// All 16 pin levels of `port` in one PPRn read; bit `m` is pin `m`. Reading changes nothing, so
/// it needs no pin ownership (an interrupt handler can read several pins at once this way). A
/// pin's level is only visible with its input buffer on.
#[inline]
pub fn read_port(port: Port) -> u16 {
    GPIO.ppr(port as usize).read()
}

macro_rules! digital_impls {
    ($ty:ident: input) => {
        impl embedded_hal::digital::ErrorType for $ty<'_> {
            type Error = Infallible;
        }
        impl embedded_hal::digital::InputPin for $ty<'_> {
            fn is_high(&mut self) -> Result<bool, Infallible> {
                Ok($ty::is_high(self))
            }
            fn is_low(&mut self) -> Result<bool, Infallible> {
                Ok($ty::is_low(self))
            }
        }
    };
    ($ty:ident: output) => {
        impl embedded_hal::digital::OutputPin for $ty<'_> {
            fn set_high(&mut self) -> Result<(), Infallible> {
                $ty::set_high(self);
                Ok(())
            }
            fn set_low(&mut self) -> Result<(), Infallible> {
                $ty::set_low(self);
                Ok(())
            }
        }
        impl embedded_hal::digital::StatefulOutputPin for $ty<'_> {
            fn is_set_high(&mut self) -> Result<bool, Infallible> {
                Ok($ty::is_set_high(self))
            }
            fn is_set_low(&mut self) -> Result<bool, Infallible> {
                Ok($ty::is_set_low(self))
            }
            fn toggle(&mut self) -> Result<(), Infallible> {
                $ty::toggle(self);
                Ok(())
            }
        }
    };
}

digital_impls!(Flex: input);
digital_impls!(Flex: output);
digital_impls!(Input: input);
impl embedded_hal::digital::ErrorType for Output<'_> {
    type Error = Infallible;
}
digital_impls!(Output: output);

// ── Low-level free functions (deprecated: removed in this phase) ────────────────

/// Configure a pin as a software-controlled GPIO output.
///
/// # Safety
/// Writes to memory-mapped peripheral registers; must run with the pin not
/// already owned by another driver.
#[deprecated(note = "use gpio::{Input, Output, Flex} on an owned pin")]
pub unsafe fn set_as_output(port: u8, pin: u8) {
    let bit = 1u16 << pin;
    // PMC = 0: GPIO (not peripheral multiplexed)
    clear_bits16(pmc(port), bit);
    // PM = 0: output direction
    clear_bits16(pm(port), bit);
    // PIPC = 0: software (not hardware-peripheral) control
    clear_bits16(pipc(port), bit);
}

/// Read-modify-write: clear the given bits in a 16-bit register.
#[inline]
fn clear_bits16(r: Reg<u16, RW>, bits: u16) {
    r.modify(|v| *v &= !bits);
}

/// Read-modify-write: set the given bits in a 16-bit register.
#[inline]
fn set_bits16(r: Reg<u16, RW>, bits: u16) {
    r.modify(|v| *v |= bits);
}

/// Configure a pin as a software-controlled GPIO input.
///
/// Sets PMC=0 (GPIO mode), PM=1 (input direction), PIBC=1 (enable input buffer).
/// After this call, the pin voltage can be read via [`read_pin`].
///
/// # Safety
/// Writes to memory-mapped peripheral registers; must run with the pin not
/// already owned by another driver.
#[deprecated(note = "use gpio::{Input, Output, Flex} on an owned pin")]
pub unsafe fn set_as_input(port: u8, pin: u8) {
    let bit = 1u16 << pin;
    // PMC = 0: GPIO (not peripheral multiplexed)
    clear_bits16(pmc(port), bit);
    // PM = 1: input direction
    set_bits16(pm(port), bit);
    // PIBC = 1: enable input buffer so PPR reflects the live pin state
    set_bits16(pibc(port), bit);
}

/// Enable the input buffer for a pin already muxed to a peripheral function.
///
/// [`set_pin_mux`] puts the pin in peripheral mode (PMC=1, PIPC=1) but does
/// **not** set PIBC, so the live logic level is not reflected in PPR.  Call
/// this after [`set_pin_mux`] when the ISR or any other code needs to read the
/// pin state via [`read_pin`] (e.g. for quadrature encoder direction decode).
///
/// # Safety
/// Writes to a memory-mapped peripheral register; `port` is 1–9.
#[deprecated(note = "use gpio::{Input, Output, Flex} on an owned pin")]
pub unsafe fn enable_input_buffer(port: u8, pin: u8) {
    set_bits16(pibc(port), 1u16 << pin)
}

/// Read the current logic level of a GPIO pin via the Port Pin Read (PPR) register.
///
/// Works after [`set_as_input`] has been called for the pin.
///
/// # Safety
/// Reads a memory-mapped peripheral register; `port` is 1–9.
#[deprecated(note = "use gpio::{Input, Output, Flex} on an owned pin")]
pub unsafe fn read_pin(port: u8, pin: u8) -> bool {
    (ppr(port).read() >> pin) & 1 != 0
}

/// Drive a GPIO output pin high (`true`) or low (`false`).
///
/// # Safety
/// The pin must have been configured as an output via [`set_as_output`].
#[deprecated(note = "use gpio::{Input, Output, Flex} on an owned pin")]
pub unsafe fn write(port: u8, pin: u8, high: bool) {
    let bit = 1u16 << pin;
    if high {
        set_bits16(p(port), bit);
    } else {
        clear_bits16(p(port), bit);
    }
}

/// Configure a pin as a peripheral function (GPIO pin-mux).
///
/// Mirrors `setPinMux(port, pin, mux)` from `gpio.c`.
///
/// `port`: 1–9.  `pin`: bit position (0–15).  `mux`: function (1–7).
///
/// # Safety
/// Writes to memory-mapped GPIO registers.
#[deprecated(note = "use gpio::{Input, Output, Flex} on an owned pin")]
pub unsafe fn set_pin_mux(port: u8, pin: u8, mux: u8) {
    fn modify(r: Reg<u16, RW>, pin: u8, set: bool) {
        if set {
            set_bits16(r, 1u16 << pin);
        } else {
            clear_bits16(r, 1u16 << pin);
        }
    }

    trace!("gpio: set_pin_mux port={} pin={} mux={}", port, pin, mux);
    let n = port as usize;
    modify(GPIO.pfcae(n), pin, mux >= 5);
    modify(GPIO.pfce(n), pin, ((mux - 1) >> 1) & 1 != 0);
    modify(GPIO.pfc(n), pin, (mux - 1) & 1 != 0);
    modify(GPIO.pmc(n), pin, true);
    modify(GPIO.pipc(n), pin, true);
}

// ── Type-state GPIO pins ───────────────────────────────────────────────────────

/// Marker for a pin configured as a software-controlled output.
#[deprecated(note = "use gpio::Output")]
pub struct OutputMode;
/// Marker for a pin configured as a software-controlled input.
#[deprecated(note = "use gpio::Input")]
pub struct InputMode;

/// A GPIO pin with a const-generic port and bit number, typed by direction.
///
/// `PORT` is 1–9 (the RZ/A1L's ports with outputs); `BIT` is 0–15.
///
/// Owns no resources; the hardware configuration is managed via the
/// underlying [`set_as_output`] / [`write`](fn@write) free functions.
#[deprecated(note = "use gpio::{Input, Output} on an owned pin")]
pub struct TypedPin<const PORT: u8, const BIT: u8, MODE>(PhantomData<MODE>);

impl<const PORT: u8, const BIT: u8, MODE> TypedPin<PORT, BIT, MODE> {
    /// A typed handle to a pin without configuring it, for a pin another part of the program set
    /// up.
    ///
    /// # Safety
    /// The caller must ensure the pin is not concurrently owned by another
    /// driver and that GPIO clocks are enabled.
    #[inline]
    pub unsafe fn steal() -> Self {
        TypedPin(PhantomData)
    }
}

impl<const PORT: u8, const BIT: u8> TypedPin<PORT, BIT, OutputMode> {
    /// Configure the underlying hardware as an output and return a typed handle.
    ///
    /// # Safety
    /// Same requirements as [`set_as_output`].
    pub unsafe fn into_output() -> Self {
        unsafe {
            set_as_output(PORT, BIT);
            TypedPin(PhantomData)
        }
    }
}

impl<const PORT: u8, const BIT: u8> embedded_hal::digital::ErrorType for TypedPin<PORT, BIT, OutputMode> {
    type Error = Infallible;
}

impl<const PORT: u8, const BIT: u8> embedded_hal::digital::OutputPin for TypedPin<PORT, BIT, OutputMode> {
    #[inline]
    fn set_high(&mut self) -> Result<(), Infallible> {
        unsafe { write(PORT, BIT, true) };
        Ok(())
    }
    #[inline]
    fn set_low(&mut self) -> Result<(), Infallible> {
        unsafe { write(PORT, BIT, false) };
        Ok(())
    }
}

impl<const PORT: u8, const BIT: u8> embedded_hal::digital::StatefulOutputPin
    for TypedPin<PORT, BIT, OutputMode>
{
    #[inline]
    fn is_set_high(&mut self) -> Result<bool, Infallible> {
        let val = p(PORT).read();
        Ok(val & (1u16 << BIT) != 0)
    }
    #[inline]
    fn is_set_low(&mut self) -> Result<bool, Infallible> {
        let val = p(PORT).read();
        Ok(val & (1u16 << BIT) == 0)
    }
}

impl<const PORT: u8, const BIT: u8> embedded_hal::digital::ErrorType for TypedPin<PORT, BIT, InputMode> {
    type Error = Infallible;
}

impl<const PORT: u8, const BIT: u8> embedded_hal::digital::InputPin for TypedPin<PORT, BIT, InputMode> {
    #[inline]
    fn is_high(&mut self) -> Result<bool, Infallible> {
        // Read PPR (Port Pin Read) — reflects actual pin voltage.
        // p() is the output-latch register and reads back 0 for input-configured pins.
        let val = ppr(PORT).read();
        Ok(val & (1u16 << BIT) != 0)
    }
    #[inline]
    fn is_low(&mut self) -> Result<bool, Infallible> {
        let val = ppr(PORT).read();
        Ok(val & (1u16 << BIT) == 0)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(all(test, not(target_os = "none")))]
mod owned_pin_tests {
    use super::*;
    use crate::mmio::test;
    use crate::peripherals::*;

    #[test]
    fn output_new_writes_the_level_before_the_direction() {
        test::reset();
        let _led = Output::new(unsafe { P6_7::steal() }, Level::High);
        assert_eq!(
            test::writes(),
            [
                (0xFCFE_3118, (1 << 23) | (1 << 7)), // PSR6: P6_7 high
                (0xFCFE_3918, 1 << 23),              // PMCSR6: PMC6_7 = 0 (port mode)
                (0xFCFE_7218, 0),                    // PIPC6: bit 7 cleared
                (0xFCFE_3818, 1 << 23),              // PMSR6: PM6_7 = 0 (output)
            ]
        );
    }

    #[test]
    fn set_level_and_toggle_use_psr_and_pnot() {
        let mut led = Output::new(unsafe { P6_7::steal() }, Level::High);
        test::reset();
        led.set_low();
        led.toggle();
        assert_eq!(test::writes(), [(0xFCFE_3118, 1 << 23), (0xFCFE_3718, 1 << 7)]);
    }

    #[test]
    fn input_new_sets_pm_and_the_input_buffer() {
        test::reset();
        let _jack = Input::new(unsafe { P6_3::steal() });
        assert_eq!(
            test::writes(),
            [
                (0xFCFE_3918, 1 << 19),              // PMCSR6: PMC6_3 = 0
                (0xFCFE_3818, (1 << 19) | (1 << 3)), // PMSR6: PM6_3 = 1 (input)
                (0xFCFE_7018, 1 << 3),               // PIBC6: input buffer on
            ]
        );
    }

    #[test]
    fn is_high_reads_ppr() {
        let jack = Input::new(unsafe { P6_4::steal() });
        test::poke32(0xFCFE_3218, 1 << 4);
        assert!(jack.is_high());
        test::poke32(0xFCFE_3218, 0);
        assert!(jack.is_low());
    }

    #[test]
    fn set_as_alternate_matches_set_pin_mux() {
        test::reset();
        let mut pin = Flex::new(unsafe { P1_7::steal() });
        pin.set_as_alternate(Alternate::Alt6);
        let bit = 1u16 << 7;
        assert_eq!(test::peek16(0xFCFE_3A04), bit, "PFCAE set (alternative 5 or above)");
        assert_eq!(test::peek16(0xFCFE_3604), 0, "PFCE clear");
        assert_eq!(test::peek16(0xFCFE_3504), bit, "PFC set");
        assert!(
            test::writes().contains(&(0xFCFE_3904, ((bit as u32) << 16) | bit as u32)),
            "PMCSR1: PMC1_7 = 1 (alternative mode)"
        );
        assert_eq!(test::peek16(0xFCFE_7204), bit, "PIPC set (the function drives direction)");
    }

    #[test]
    fn set_as_alternate_software_io_clears_pipc() {
        test::reset();
        test::poke32(0xFCFE_7218, 0xFFFF); // PIPC6 all set beforehand
        let mut pin = Flex::new(unsafe { P6_8::steal() });
        pin.set_as_alternate_software_io(Alternate::Alt3, true);
        let bit = 1u32 << 8;
        assert_eq!(test::peek16(0xFCFE_7218) as u32 & bit, 0, "PIPC clear (Table 41.7)");
        let writes = test::writes();
        assert!(writes.contains(&(0xFCFE_3818, bit << 16)), "PMSR6: PM6_8 = 0 (output)");
        assert!(writes.contains(&(0xFCFE_3918, (bit << 16) | bit)), "PMCSR6: PMC6_8 = 1");
    }

    #[test]
    fn dropping_an_output_leaves_the_pin_as_it_was() {
        let led = Output::new(unsafe { P6_7::steal() }, Level::High);
        test::reset();
        drop(led);
        assert!(test::writes().is_empty());
    }

    #[test]
    fn output_capable_pins_are_the_tables_outputs() {
        fn assert_output<T: OutputCapable>() {}
        assert_output::<P1_0>();
        assert_output::<P1_7>();
        assert_output::<P2_0>();
        assert_output::<P9_5>();
    }

    #[test]
    #[should_panic(expected = "P1_8 has no port-mode output")]
    fn an_input_only_pin_refuses_to_become_an_output() {
        let mut pin = Flex::new(unsafe { P1_8::steal() });
        pin.set_as_output(Level::Low);
    }

    #[test]
    fn read_port_reads_one_ppr() {
        test::poke32(0xFCFE_3204, 0xA5A5);
        assert_eq!(read_port(Port::P1), 0xA5A5);
    }
}

#[cfg(all(test, not(target_os = "none"), feature = "rza1l"))]
mod pin_function_tests {
    use super::Alternate;
    use crate::peripherals::*;
    use crate::{rspi, sdhi, uart};

    fn tx<T: uart::Instance>(p: &impl uart::TxPin<T>) -> Alternate { p.alternate() }
    fn rx<T: uart::Instance>(p: &impl uart::RxPin<T>) -> Alternate { p.alternate() }
    fn sck<T: rspi::Instance>(p: &impl rspi::SckPin<T>) -> Alternate { p.alternate() }
    fn mosi<T: rspi::Instance>(p: &impl rspi::MosiPin<T>) -> Alternate { p.alternate() }
    fn clk<T: sdhi::Instance>(p: &impl sdhi::ClkPin<T>) -> Alternate { p.alternate() }
    fn cmd<T: sdhi::Instance>(p: &impl sdhi::CmdPin<T>) -> Alternate { p.alternate() }
    fn d0<T: sdhi::Instance>(p: &impl sdhi::D0Pin<T>) -> Alternate { p.alternate() }
    fn d1<T: sdhi::Instance>(p: &impl sdhi::D1Pin<T>) -> Alternate { p.alternate() }
    fn d2<T: sdhi::Instance>(p: &impl sdhi::D2Pin<T>) -> Alternate { p.alternate() }
    fn d3<T: sdhi::Instance>(p: &impl sdhi::D3Pin<T>) -> Alternate { p.alternate() }
    fn cd<T: sdhi::Instance>(p: &impl sdhi::CdPin<T>) -> Alternate { p.alternate() }
    fn wp<T: sdhi::Instance>(p: &impl sdhi::WpPin<T>) -> Alternate { p.alternate() }

    #[test]
    fn pin_functions_match_a_known_working_board() {
        // Every function a known-working board muxes, with the alternative it uses.
        let p = unsafe {
            (
                P6_15::steal(), P6_14::steal(), P3_15::steal(), P1_9::steal(), P6_0::steal(),
                P6_2::steal(), P7_0::steal(), P7_1::steal(), P7_2::steal(), P7_3::steal(),
                P7_4::steal(), P7_5::steal(), P7_6::steal(), P7_7::steal(),
            )
        };
        assert_eq!(tx::<SCIF0>(&*p.0), Alternate::Alt5, "MIDI TxD0 on P6_15");
        assert_eq!(rx::<SCIF0>(&*p.1), Alternate::Alt5, "MIDI RxD0 on P6_14");
        assert_eq!(tx::<SCIF1>(&*p.2), Alternate::Alt5, "PIC TxD1 on P3_15 (the override)");
        assert_eq!(rx::<SCIF1>(&*p.3), Alternate::Alt3, "PIC RxD1 on P1_9");
        assert_eq!(sck::<RSPI0>(&*p.4), Alternate::Alt3, "RSPCK0 on P6_0");
        assert_eq!(mosi::<RSPI0>(&*p.5), Alternate::Alt3, "MOSI0 on P6_2");
        assert_eq!(cd::<SDHI1>(&*p.6), Alternate::Alt3, "SD_CD_1 on P7_0");
        assert_eq!(wp::<SDHI1>(&*p.7), Alternate::Alt3, "SD_WP_1 on P7_1");
        assert_eq!(d1::<SDHI1>(&*p.8), Alternate::Alt3, "SD_D1_1 on P7_2");
        assert_eq!(d0::<SDHI1>(&*p.9), Alternate::Alt3, "SD_D0_1 on P7_3");
        assert_eq!(clk::<SDHI1>(&*p.10), Alternate::Alt3, "SD_CLK_1 on P7_4");
        assert_eq!(cmd::<SDHI1>(&*p.11), Alternate::Alt3, "SD_CMD_1 on P7_5");
        assert_eq!(d3::<SDHI1>(&*p.12), Alternate::Alt3, "SD_D3_1 on P7_6");
        assert_eq!(d2::<SDHI1>(&*p.13), Alternate::Alt3, "SD_D2_1 on P7_7");
    }
}

#[cfg(all(test, not(target_os = "none")))]
#[allow(deprecated)]
mod tests {
    use super::*;

    #[test]
    fn the_pac_indexes_gpio_registers_by_port() {
        use crate::pac::GPIO;
        for port in 1..=9usize {
            let n = port * 4;
            assert_eq!(
                (GPIO.p(port).addr(), GPIO.p(port).width()),
                (0xFCFE_3000 + n, 2)
            );
            assert_eq!(GPIO.ppr(port).addr(), 0xFCFE_3200 + n);
            assert_eq!(GPIO.pm(port).addr(), 0xFCFE_3300 + n);
            assert_eq!(GPIO.pfc(port).addr(), 0xFCFE_3500 + n);
            assert_eq!(GPIO.pfce(port).addr(), 0xFCFE_3600 + n);
            assert_eq!(GPIO.pfcae(port).addr(), 0xFCFE_3A00 + n);
            assert_eq!(GPIO.pipc(port).addr(), 0xFCFE_7200 + n);
        }
        for port in 0..=9usize {
            assert_eq!(
                (GPIO.pmc(port).addr(), GPIO.pmc(port).width()),
                (0xFCFE_3400 + port * 4, 2)
            );
            assert_eq!(
                (GPIO.pibc(port).addr(), GPIO.pibc(port).width()),
                (0xFCFE_7000 + port * 4, 2)
            );
        }
    }

    /// The {PFCAE, PFCE, PFC} 3-bit field encodes `mux - 1` for every valid
    /// mux function (1–7).  This matches the RZ/A1L HW Manual Table 21-x.
    #[test]
    fn gpio_pin_mux_encoding() {
        for mux in 1u8..=7 {
            let pfcae = mux >= 5;
            let pfce = ((mux - 1) >> 1) & 1 != 0;
            let pfc = (mux - 1) & 1 != 0;
            let bits = ((pfcae as u8) << 2) | ((pfce as u8) << 1) | (pfc as u8);
            assert_eq!(bits, mux - 1, "mux={mux}");
        }
    }

    // ── register-effect tests driven through the MMIO seam ──────────────────
    use crate::mmio;

    /// `set_as_output` clears the pin bit in PMC, PM and PIPC (read-modify-write
    /// that leaves the other bits untouched).
    #[test]
    fn set_as_output_clears_pmc_pm_pipc_bits() {
        mmio::test::reset();
        // Pre-fill the three registers so we can see exactly which bit clears.
        mmio::test::poke16(pmc(6).addr(), 0xFFFF);
        mmio::test::poke16(pm(6).addr(), 0xFFFF);
        mmio::test::poke16(pipc(6).addr(), 0xFFFF);

        unsafe { set_as_output(6, 12) };

        assert_eq!(
            mmio::test::peek16(pmc(6).addr()),
            0xEFFF,
            "PMC bit 12 cleared"
        );
        assert_eq!(
            mmio::test::peek16(pm(6).addr()),
            0xEFFF,
            "PM bit 12 cleared (output)"
        );
        assert_eq!(
            mmio::test::peek16(pipc(6).addr()),
            0xEFFF,
            "PIPC bit 12 cleared (sw)"
        );
    }

    /// `set_as_input` clears PMC, sets PM (input) and sets PIBC (input buffer).
    #[test]
    fn set_as_input_sets_direction_and_buffer() {
        mmio::test::reset();
        unsafe { set_as_input(3, 5) };
        let bit = 1u16 << 5;
        assert_eq!(
            mmio::test::peek16(pmc(3).addr()),
            0,
            "PMC bit cleared (GPIO)"
        );
        assert_eq!(mmio::test::peek16(pm(3).addr()), bit, "PM bit set (input)");
        assert_eq!(mmio::test::peek16(pibc(3).addr()), bit, "PIBC bit set");
    }

    /// `write` is a read-modify-write that only touches the target bit.
    #[test]
    fn write_sets_and_clears_single_bit() {
        mmio::test::reset();
        mmio::test::poke16(p(2).addr(), 0b1010);
        unsafe { write(2, 0, true) }; // set bit 0
        assert_eq!(mmio::test::peek16(p(2).addr()), 0b1011);
        unsafe { write(2, 1, false) }; // clear bit 1
        assert_eq!(mmio::test::peek16(p(2).addr()), 0b1001);
    }

    /// `set_pin_mux` writes the 3-bit function code across PFCAE/PFCE/PFC and
    /// enables peripheral mode (PMC=1, PIPC=1). Check a representative mux=6
    /// (encodes (mux-1)=5 = 0b101 → PFCAE=1, PFCE=0, PFC=1).
    #[test]
    fn set_pin_mux_encodes_function_and_enables_peripheral() {
        mmio::test::reset();
        const PFC: usize = 0xFCFE_3504;
        const PFCE: usize = 0xFCFE_3604;
        const PFCAE: usize = 0xFCFE_3A04;
        const PMC1: usize = 0xFCFE_3404;
        const PIPC1: usize = 0xFCFE_7204;
        let port = 1u8;
        let pin = 7u8;
        let off = (port as usize - 1) * 4;
        let bit = 1u16 << pin;

        unsafe { set_pin_mux(port, pin, 6) };

        assert_eq!(mmio::test::peek16(PFCAE + off), bit, "PFCAE set (mux>=5)");
        assert_eq!(mmio::test::peek16(PFCE + off), 0, "PFCE clear");
        assert_eq!(mmio::test::peek16(PFC + off), bit, "PFC set");
        assert_eq!(mmio::test::peek16(PMC1 + off), bit, "PMC set (peripheral)");
        assert_eq!(
            mmio::test::peek16(PIPC1 + off),
            bit,
            "PIPC set (hw control)"
        );
    }

    /// The type-state `Pin` output drives the output-latch bit and reads it
    /// back, all through the seam.
    #[test]
    fn output_pin_set_and_read_back() {
        use embedded_hal::digital::{OutputPin, StatefulOutputPin};
        mmio::test::reset();
        let mut pin: TypedPin<7, 4, OutputMode> = TypedPin(core::marker::PhantomData);
        pin.set_high().unwrap();
        assert_eq!(mmio::test::peek16(p(7).addr()), 1 << 4);
        assert!(pin.is_set_high().unwrap());
        pin.set_low().unwrap();
        assert!(pin.is_set_low().unwrap());
        assert_eq!(mmio::test::peek16(p(7).addr()), 0);
    }

    /// The input `Pin` reads the live pin level from PPR (preloaded here).
    #[test]
    fn input_pin_reads_ppr() {
        use embedded_hal::digital::InputPin;
        mmio::test::reset();
        mmio::test::poke16(ppr(3).addr(), 1 << 9);
        let mut pin: TypedPin<3, 9, InputMode> = TypedPin(core::marker::PhantomData);
        assert!(pin.is_high().unwrap());
        let mut other: TypedPin<3, 8, InputMode> = TypedPin(core::marker::PhantomData);
        assert!(other.is_low().unwrap());
    }
}
