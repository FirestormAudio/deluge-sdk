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

// ── Low-level free functions ───────────────────────────────────────────────────

/// Configure a pin as a software-controlled GPIO output.
///
/// # Safety
/// Writes to memory-mapped peripheral registers; must run with the pin not
/// already owned by another driver.
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
pub unsafe fn enable_input_buffer(port: u8, pin: u8) {
    set_bits16(pibc(port), 1u16 << pin)
}

/// Read the current logic level of a GPIO pin via the Port Pin Read (PPR) register.
///
/// Works after [`set_as_input`] has been called for the pin.
///
/// # Safety
/// Reads a memory-mapped peripheral register; `port` is 1–9.
pub unsafe fn read_pin(port: u8, pin: u8) -> bool {
    (ppr(port).read() >> pin) & 1 != 0
}

/// Read all 16 pins of a port in a single atomic PPR register read.
///
/// Returns a bitmask; bit `n` corresponds to pin `n` of `port`.
/// The PIBC bits for the pins of interest must be set before calling this.
///
/// # Safety
/// Reads a memory-mapped peripheral register; `port` is 1–9.
pub unsafe fn read_port(port: u8) -> u16 {
    ppr(port).read()
}

/// Drive a GPIO output pin high (`true`) or low (`false`).
///
/// # Safety
/// The pin must have been configured as an output via [`set_as_output`].
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
pub unsafe fn set_pin_mux(port: u8, pin: u8, mux: u8) {
    fn modify(r: Reg<u16, RW>, pin: u8, set: bool) {
        if set {
            set_bits16(r, 1u16 << pin);
        } else {
            clear_bits16(r, 1u16 << pin);
        }
    }

    log::trace!("gpio: set_pin_mux port={} pin={} mux={}", port, pin, mux);
    let n = port as usize;
    modify(GPIO.pfcae(n), pin, mux >= 5);
    modify(GPIO.pfce(n), pin, ((mux - 1) >> 1) & 1 != 0);
    modify(GPIO.pfc(n), pin, (mux - 1) & 1 != 0);
    modify(GPIO.pmc(n), pin, true);
    modify(GPIO.pipc(n), pin, true);
}

// ── Type-state GPIO pins ───────────────────────────────────────────────────────

/// Marker for a pin configured as a software-controlled output.
pub struct Output;
/// Marker for a pin configured as a software-controlled input.
pub struct Input;

/// A GPIO pin with a const-generic port and bit number, typed by direction.
///
/// `PORT` is 1–9 (the RZ/A1L's ports with outputs); `BIT` is 0–15.
///
/// Owns no resources; the hardware configuration is managed via the
/// underlying [`set_as_output`] / [`write`](fn@write) free functions.
pub struct Pin<const PORT: u8, const BIT: u8, MODE>(PhantomData<MODE>);

impl<const PORT: u8, const BIT: u8, MODE> Pin<PORT, BIT, MODE> {
    /// A typed handle to a pin without configuring it, for a pin another part of the program set
    /// up.
    ///
    /// # Safety
    /// The caller must ensure the pin is not concurrently owned by another
    /// driver and that GPIO clocks are enabled.
    #[inline]
    pub unsafe fn steal() -> Self {
        Pin(PhantomData)
    }
}

impl<const PORT: u8, const BIT: u8> Pin<PORT, BIT, Output> {
    /// Configure the underlying hardware as an output and return a typed handle.
    ///
    /// # Safety
    /// Same requirements as [`set_as_output`].
    pub unsafe fn into_output() -> Self {
        unsafe {
            set_as_output(PORT, BIT);
            Pin(PhantomData)
        }
    }
}

impl<const PORT: u8, const BIT: u8> embedded_hal::digital::ErrorType for Pin<PORT, BIT, Output> {
    type Error = Infallible;
}

impl<const PORT: u8, const BIT: u8> embedded_hal::digital::OutputPin for Pin<PORT, BIT, Output> {
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
    for Pin<PORT, BIT, Output>
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

impl<const PORT: u8, const BIT: u8> embedded_hal::digital::ErrorType for Pin<PORT, BIT, Input> {
    type Error = Infallible;
}

impl<const PORT: u8, const BIT: u8> embedded_hal::digital::InputPin for Pin<PORT, BIT, Input> {
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
        let mut pin: Pin<7, 4, Output> = Pin(core::marker::PhantomData);
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
        let mut pin: Pin<3, 9, Input> = Pin(core::marker::PhantomData);
        assert!(pin.is_high().unwrap());
        let mut other: Pin<3, 8, Input> = Pin(core::marker::PhantomData);
        assert!(other.is_low().unwrap());
    }
}
