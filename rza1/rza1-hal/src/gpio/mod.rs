//! GPIO: owned pins (HW manual ch. 41, Ports).
//!
//! Every pin is a singleton in [`Peripherals`](crate::Peripherals) (`P6_7` …). Take one into an
//! [`Input`], an [`Output`] or a [`Flex`]; drivers that use a pin for a peripheral function take
//! it by its pin-function trait (`uart::TxPin<T>` …) and set the function themselves. An output's
//! level is written through PSRn and toggled through PNOTn, which every output-capable pin has a
//! bit in. Mode, direction, function select, input buffer and PIPC are read-modify-written in a
//! critical section: PMCSRn and PMSRn would avoid that, but PMCSR1/PMSR1 have no bits for
//! P1_8-P1_15 (Table 41.14), so a pin there could never reach alternative mode through them.
//! Either way a write never undoes another pin's. Port 0 has no P, PSR, PM, PMSR or PIPC, and
//! nothing writes them.
//!
//! Register map (16-bit unless noted, port `n` at `+4n` from `0xFCFE_3000`): P `+0x000`,
//! PSR (32-bit) `+0x100`, PPR `+0x200`, PM `+0x300`, PMC `+0x400`, PFC `+0x500`, PFCE `+0x600`,
//! PNOT `+0x700`, PMSR (32-bit) `+0x800`, PMCSR (32-bit) `+0x900`, PFCAE `+0xA00`,
//! PIBC `+0x4000`, PIPC `+0x4200`.

use core::convert::Infallible;

use crate::pac::GPIO;
use crate::pac::common::{RW, Reg};
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
/// which lower bits are written (PSRn).
const fn set_reset(mask: u16, value: bool) -> u32 {
    ((mask as u32) << 16) | if value { mask as u32 } else { 0 }
}

/// Read-modify-write of a register with no set/reset twin, in a critical section so an interrupt
/// touching another pin of the port can't undo it.
fn modify_bit(r: Reg<u16, RW>, mask: u16, set: bool) {
    critical_section::with(|_| r.modify(|v| if set { *v |= mask } else { *v &= !mask }));
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

    /// A software-controlled input: port mode (PMC = 0), input direction (PM = 1; port 0 has no
    /// PM, its pins are inputs), input buffer on (PIBC = 1).
    pub fn set_as_input(&mut self) {
        let (n, m) = (self.n(), self.mask());
        modify_bit(GPIO.pmc(n), m, false);
        if n != 0 {
            modify_bit(GPIO.pm(n), m, true);
        }
        modify_bit(GPIO.pibc(n), m, true);
    }

    /// A software-controlled output at `initial`: the level is written before the direction
    /// switches, so the pin never drives a stale value.
    ///
    /// # Panics
    /// If the pin has no port-mode output (P0_0–P0_3, P1_8–P1_15).
    pub fn set_as_output(&mut self, initial: Level) {
        self.assert_can_output();
        let (n, m) = (self.n(), self.mask());
        GPIO.psr(n).write_value(set_reset(m, initial.into()));
        modify_bit(GPIO.pmc(n), m, false);
        modify_bit(GPIO.pipc(n), m, false);
        modify_bit(GPIO.pm(n), m, false);
    }

    fn assert_can_output(&self) {
        let (port, bit) = (self.pin.port() as u8, self.pin.bit());
        if !has_port_output(port, bit) {
            panic!("P{}_{} has no port-mode output", port, bit);
        }
    }

    fn select_alternate(&mut self, alt: Alternate) {
        let (n, m) = (self.n(), self.mask());
        let k = alt as u8 - 1;
        modify_bit(GPIO.pfcae(n), m, k >= 4);
        modify_bit(GPIO.pfce(n), m, (k >> 1) & 1 != 0);
        modify_bit(GPIO.pfc(n), m, k & 1 != 0);
        modify_bit(GPIO.pmc(n), m, true);
    }

    /// Alternative function `alt`, its I/O direction driven by the function (PIPC = 1). Use
    /// [`set_as_alternate_software_io`](Self::set_as_alternate_software_io) for the functions of
    /// HW manual Table 41.7 (TIOCnx, SSITxD0/1/3, WDTOVF).
    pub fn set_as_alternate(&mut self, alt: Alternate) {
        self.select_alternate(alt);
        // Port 0 has no PIPC: its alternative functions are inputs, driven by the function.
        if self.n() != 0 {
            modify_bit(GPIO.pipc(self.n()), self.mask(), true);
        }
    }

    /// Alternative function `alt` with its I/O direction set by PM, as HW manual Table 41.7
    /// requires for TIOCnx, SSITxD0/1/3 and WDTOVF (PIPC = 0; PM = 0 for `output`).
    ///
    /// # Panics
    /// If `output` and the pin has no port-mode output (P0_0–P0_3, P1_8–P1_15).
    pub fn set_as_alternate_software_io(&mut self, alt: Alternate, output: bool) {
        if output {
            self.assert_can_output();
        }
        self.select_alternate(alt);
        let (n, m) = (self.n(), self.mask());
        if n != 0 {
            modify_bit(GPIO.pipc(n), m, false);
            modify_bit(GPIO.pm(n), m, !output);
        }
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
        GPIO.psr(self.n())
            .write_value(set_reset(self.mask(), level.into()));
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
                (0xFCFE_3418, 0),                    // PMC6: bit 7 cleared (port mode)
                (0xFCFE_7218, 0),                    // PIPC6: bit 7 cleared
                (0xFCFE_3318, 0),                    // PM6: bit 7 cleared (output)
            ]
        );
    }

    #[test]
    fn set_level_and_toggle_use_psr_and_pnot() {
        let mut led = Output::new(unsafe { P6_7::steal() }, Level::High);
        test::reset();
        led.set_low();
        led.toggle();
        assert_eq!(
            test::writes(),
            [(0xFCFE_3118, 1 << 23), (0xFCFE_3718, 1 << 7)]
        );
    }

    #[test]
    fn input_new_sets_pm_and_the_input_buffer() {
        test::reset();
        let _jack = Input::new(unsafe { P6_3::steal() });
        assert_eq!(
            test::writes(),
            [
                (0xFCFE_3418, 0),      // PMC6: bit 3 cleared (port mode)
                (0xFCFE_3318, 1 << 3), // PM6: bit 3 set (input)
                (0xFCFE_7018, 1 << 3), // PIBC6: input buffer on
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
        assert_eq!(
            test::peek16(0xFCFE_3A04),
            bit,
            "PFCAE set (alternative 5 or above)"
        );
        assert_eq!(test::peek16(0xFCFE_3604), 0, "PFCE clear");
        assert_eq!(test::peek16(0xFCFE_3504), bit, "PFC set");
        assert_eq!(test::peek16(0xFCFE_3404), bit, "PMC set (alternative mode)");
        assert_eq!(
            test::peek16(0xFCFE_7204),
            bit,
            "PIPC set (the function drives direction)"
        );
    }

    #[test]
    fn set_as_alternate_software_io_clears_pipc() {
        test::reset();
        test::poke32(0xFCFE_7218, 0xFFFF); // PIPC6 all set beforehand
        test::poke32(0xFCFE_3318, 0xFFFF); // PM6 at its reset value: all inputs
        // P6_10 is SSITxD0 on alternative 3, one of Table 41.7's functions.
        let mut pin = Flex::new(unsafe { P6_10::steal() });
        pin.set_as_alternate_software_io(Alternate::Alt3, true);
        let bit = 1u16 << 10;
        assert_eq!(
            test::peek16(0xFCFE_7218) & bit,
            0,
            "PIPC clear (Table 41.7)"
        );
        assert_eq!(test::peek16(0xFCFE_3318) & bit, 0, "PM clear (output)");
        assert_eq!(test::peek16(0xFCFE_3418) & bit, bit, "PMC set");
    }

    #[test]
    fn alternate_mode_reaches_pmc_on_p1_8_to_p1_15() {
        // PMCSR1 has no bits for P1_8-P1_15 (Table 41.14: valid bits 23-16 and 7-0), so PMC1 must
        // be written itself: the resulting PMC1 holds the pin's bit.
        test::reset();
        Flex::new(unsafe { P1_9::steal() }).set_as_alternate(Alternate::Alt3);
        Flex::new(unsafe { P1_14::steal() }).set_as_alternate(Alternate::Alt2);
        let pmc1 = test::peek16(0xFCFE_3404);
        assert_eq!(
            pmc1 & ((1 << 9) | (1 << 14)),
            (1 << 9) | (1 << 14),
            "PMC1 = {pmc1:#06x}"
        );
    }

    #[test]
    fn port_0_touches_only_the_registers_it_has() {
        // Port 0 has PPR, PMC, PFC, PFCE, PMCSR, PFCAE and PIBC only (Table 41.12): no P, PSR, PM,
        // PMSR or PIPC.
        test::reset();
        Input::new(unsafe { P0_0::steal() });
        Flex::new(unsafe { P0_2::steal() }).set_as_alternate(Alternate::Alt2);
        let missing = [
            0xFCFE_3000,
            0xFCFE_3100,
            0xFCFE_3300,
            0xFCFE_3800,
            0xFCFE_7200,
        ];
        let bad: std::vec::Vec<_> = test::writes()
            .into_iter()
            .filter(|(a, _)| missing.contains(a))
            .collect();
        assert!(
            bad.is_empty(),
            "writes to port 0 registers that don't exist: {bad:x?}"
        );
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
    #[should_panic(expected = "P1_8 has no port-mode output")]
    fn an_input_only_pin_refuses_a_software_io_output() {
        let mut pin = Flex::new(unsafe { P1_8::steal() });
        pin.set_as_alternate_software_io(Alternate::Alt1, true);
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

    fn tx<T: uart::Instance>(p: &impl uart::TxPin<T>) -> Alternate {
        p.alternate()
    }
    fn rx<T: uart::Instance>(p: &impl uart::RxPin<T>) -> Alternate {
        p.alternate()
    }
    fn sck<T: rspi::Instance>(p: &impl rspi::SckPin<T>) -> Alternate {
        p.alternate()
    }
    fn mosi<T: rspi::Instance>(p: &impl rspi::MosiPin<T>) -> Alternate {
        p.alternate()
    }
    fn clk<T: sdhi::Instance>(p: &impl sdhi::ClkPin<T>) -> Alternate {
        p.alternate()
    }
    fn cmd<T: sdhi::Instance>(p: &impl sdhi::CmdPin<T>) -> Alternate {
        p.alternate()
    }
    fn d0<T: sdhi::Instance>(p: &impl sdhi::D0Pin<T>) -> Alternate {
        p.alternate()
    }
    fn d1<T: sdhi::Instance>(p: &impl sdhi::D1Pin<T>) -> Alternate {
        p.alternate()
    }
    fn d2<T: sdhi::Instance>(p: &impl sdhi::D2Pin<T>) -> Alternate {
        p.alternate()
    }
    fn d3<T: sdhi::Instance>(p: &impl sdhi::D3Pin<T>) -> Alternate {
        p.alternate()
    }
    fn cd<T: sdhi::Instance>(p: &impl sdhi::CdPin<T>) -> Alternate {
        p.alternate()
    }
    fn wp<T: sdhi::Instance>(p: &impl sdhi::WpPin<T>) -> Alternate {
        p.alternate()
    }

    #[test]
    fn pin_functions_match_a_known_working_board() {
        // Every function a known-working board muxes, with the alternative it uses.
        let p = unsafe {
            (
                P6_15::steal(),
                P6_14::steal(),
                P3_15::steal(),
                P1_9::steal(),
                P6_0::steal(),
                P6_2::steal(),
                P7_0::steal(),
                P7_1::steal(),
                P7_2::steal(),
                P7_3::steal(),
                P7_4::steal(),
                P7_5::steal(),
                P7_6::steal(),
                P7_7::steal(),
            )
        };
        assert_eq!(tx::<SCIF0>(&*p.0), Alternate::Alt5, "MIDI TxD0 on P6_15");
        assert_eq!(rx::<SCIF0>(&*p.1), Alternate::Alt5, "MIDI RxD0 on P6_14");
        assert_eq!(
            tx::<SCIF1>(&*p.2),
            Alternate::Alt5,
            "PIC TxD1 on P3_15 (the override)"
        );
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
mod register_map_tests {
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
}
