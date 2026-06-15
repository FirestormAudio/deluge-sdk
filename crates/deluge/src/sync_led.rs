//! The Deluge SYNC LED.

use embedded_hal::digital::{OutputPin, StatefulOutputPin};
use rza1l_hal::gpio::{Output, Pin};

/// The SYNC LED is wired to port 6, pin 7.
type SyncLedPin = Pin<6, 7, Output>;

/// The Deluge SYNC LED (P6_7).
///
/// Obtained once from [`Deluge::sync_led`](crate::Deluge::sync_led). Owning the
/// `SyncLed` means the compiler keeps a single place in charge of it — the GPIO
/// analogue of the bus ownership the BSP enforces for shared peripherals.
///
/// Beyond the convenience methods, `SyncLed` is an `embedded-hal`
/// [`OutputPin`]/[`StatefulOutputPin`], and [`SyncLed::pin`] hands the underlying
/// pin to driver crates that expect one.
pub struct SyncLed {
    pin: SyncLedPin,
}

impl SyncLed {
    /// Configure P6_7 as an output and return the handle.
    ///
    /// Internal — apps obtain the LED via
    /// [`Deluge::sync_led`](crate::Deluge::sync_led), whose take-once guard
    /// guarantees single ownership.
    pub(crate) fn new() -> Self {
        // SAFETY: the take-once guard in `Deluge::sync_led` ensures this runs once
        // and nothing else owns P6_7; clocks are up by the time an app runs.
        let pin = unsafe { SyncLedPin::into_output() };
        SyncLed { pin }
    }

    /// Turn the LED on.
    #[inline]
    pub fn on(&mut self) {
        let _ = self.pin.set_high();
    }

    /// Turn the LED off.
    #[inline]
    pub fn off(&mut self) {
        let _ = self.pin.set_low();
    }

    /// Set the LED on (`true`) or off (`false`).
    #[inline]
    pub fn set(&mut self, on: bool) {
        if on {
            self.on();
        } else {
            self.off();
        }
    }

    /// Toggle the LED.
    #[inline]
    pub fn toggle(&mut self) {
        let _ = StatefulOutputPin::toggle(&mut self.pin);
    }

    /// Borrow the underlying `embedded-hal` output pin, for driver crates that
    /// take an [`OutputPin`].
    #[inline]
    pub fn pin(&mut self) -> &mut impl OutputPin {
        &mut self.pin
    }
}

// Let `SyncLed` itself stand in wherever an `embedded-hal` output pin is expected.
impl embedded_hal::digital::ErrorType for SyncLed {
    type Error = core::convert::Infallible;
}

impl OutputPin for SyncLed {
    #[inline]
    fn set_high(&mut self) -> Result<(), Self::Error> {
        self.pin.set_high()
    }
    #[inline]
    fn set_low(&mut self) -> Result<(), Self::Error> {
        self.pin.set_low()
    }
}

impl StatefulOutputPin for SyncLed {
    #[inline]
    fn is_set_high(&mut self) -> Result<bool, Self::Error> {
        self.pin.is_set_high()
    }
    #[inline]
    fn is_set_low(&mut self) -> Result<bool, Self::Error> {
        self.pin.is_set_low()
    }
}
