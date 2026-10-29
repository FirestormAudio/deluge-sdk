//! External pin interrupts IRQ0–IRQ7: [`ExtiInput`], a pin that waits for an edge or a level, as
//! embedded-hal's [`Wait`](embedded_hal_async::digital::Wait).
//!
//! Each IRQn is a singleton (`peripherals::EXTI0`–`EXTI7`) that pairs with a pin that can carry
//! it ([`IrqPin`]), and is bound to its handler:
//!
//! ```ignore
//! rza1_hal::bind_interrupts!(struct Irqs {
//!     IRQ3 => exti::InterruptHandler<EXTI3>;
//! });
//! let mut button = exti::ExtiInput::new(p.P1_7, p.EXTI3, Irqs);
//! button.wait_for_falling_edge().await;
//! ```

use core::convert::Infallible;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Poll;

use embassy_hal_internal::drop::OnDrop;
use embassy_sync::waitqueue::AtomicWaker;

use crate::Peri;
use crate::gpio::Flex;
use crate::interrupt::{self, Binding, Handler, Interrupt};

mod sealed {
    pub trait Sealed {}
}

/// An external interrupt line, IRQ0–IRQ7 (`peripherals::EXTI0`–`EXTI7`).
pub trait Channel: crate::PeripheralType + sealed::Sealed + 'static {
    /// The line number, 0–7.
    const NUMBER: u8;
    /// Its GIC interrupt, `IRQ<NUMBER>`.
    type Interrupt: Interrupt;
}

macro_rules! channels {
    ($($exti:ident = $n:literal, $irq:ident;)*) => {$(
        impl sealed::Sealed for crate::peripherals::$exti {}
        impl Channel for crate::peripherals::$exti {
            const NUMBER: u8 = $n;
            type Interrupt = crate::interrupt::typelevel::$irq;
        }
    )*};
}

channels! {
    EXTI0 = 0, IRQ0;
    EXTI1 = 1, IRQ1;
    EXTI2 = 2, IRQ2;
    EXTI3 = 3, IRQ3;
    EXTI4 = 4, IRQ4;
    EXTI5 = 5, IRQ5;
    EXTI6 = 6, IRQ6;
    EXTI7 = 7, IRQ7;
}

pin_trait!(
    /// A pin that can carry external interrupt `T` (IRQn).
    IrqPin, Channel
);

/// IRQn's waiting task.
static WAKERS: [AtomicWaker; 8] = [const { AtomicWaker::new() }; 8];
/// Whether IRQn fired since its wait was armed.
static FIRED: [AtomicBool; 8] = [const { AtomicBool::new(false) }; 8];

/// ICR1 sense values.
const LEVEL_LOW: u16 = 0b00;
const FALLING: u16 = 0b01;
const RISING: u16 = 0b10;
const BOTH: u16 = 0b11;

/// IRQn's handler: clears its IRQRR flag, masks it (a level-low line would fire again at once)
/// and wakes the waiting task.
pub struct InterruptHandler<C: Channel> {
    _ch: PhantomData<C>,
}

impl<C: Channel> Handler<C::Interrupt> for InterruptHandler<C> {
    unsafe fn on_interrupt() {
        // SAFETY: IRQn's own flag.
        unsafe { crate::gic::clear_irq_pending(C::NUMBER) };
        C::Interrupt::disable();
        FIRED[C::NUMBER as usize].store(true, Ordering::Release);
        WAKERS[C::NUMBER as usize].wake();
    }
}

/// A pin on its IRQn function, waiting on the line's interrupt. Dropping it masks the interrupt.
pub struct ExtiInput<'d> {
    pin: Flex<'d>,
    number: u8,
    irq: u16,
}

impl<'d> ExtiInput<'d> {
    /// Put `pin` on IRQ line `ch`'s function, with its input buffer on so its level reads too.
    /// The line stays masked until a wait arms it.
    pub fn new<C: Channel>(
        pin: Peri<'d, impl IrqPin<C>>,
        _ch: Peri<'d, C>,
        irq: impl Binding<C::Interrupt, InterruptHandler<C>> + 'd,
    ) -> Self {
        let alt = pin.alternate();
        let mut pin = Flex::new(pin);
        pin.set_as_alternate(alt);
        pin.enable_input_buffer();
        C::Interrupt::disable();
        interrupt::bind::<C::Interrupt, InterruptHandler<C>>(irq);
        Self { pin, number: C::NUMBER, irq: C::Interrupt::ID }
    }

    /// Whether the pin reads high.
    pub fn is_high(&self) -> bool {
        self.pin.is_high()
    }

    /// Whether the pin reads low.
    pub fn is_low(&self) -> bool {
        self.pin.is_low()
    }

    /// Wait for a rising edge.
    pub async fn wait_for_rising_edge(&mut self) {
        self.wait(RISING, None).await
    }

    /// Wait for a falling edge.
    pub async fn wait_for_falling_edge(&mut self) {
        self.wait(FALLING, None).await
    }

    /// Wait for either edge.
    pub async fn wait_for_any_edge(&mut self) {
        self.wait(BOTH, None).await
    }

    /// Wait until the pin reads high: at once if it does.
    pub async fn wait_for_high(&mut self) {
        self.wait(RISING, Some(true)).await
    }

    /// Wait until the pin reads low: at once if it does.
    pub async fn wait_for_low(&mut self) {
        self.wait(LEVEL_LOW, Some(false)).await
    }

    /// Arm the line with `sense`, then wait for it to fire, or return once the pin reads
    /// `level_done` (checked after arming, so a change in between still fires).
    async fn wait(&mut self, sense: u16, level_done: Option<bool>) {
        let (n, irq) = (self.number, self.irq);
        FIRED[n as usize].store(false, Ordering::Release);
        // SAFETY: the line is masked (by `new`, its handler, or the guard below).
        unsafe {
            crate::gic::set_irq_sense(n, sense);
            crate::gic::clear_irq_pending(n);
        }
        let _guard = OnDrop::new(move || {
            // SAFETY: masking an interrupt can't make a handler run.
            unsafe { crate::gic::disable(irq) }
        });
        // SAFETY: this line's handler only touches its own flag, waker and mask.
        unsafe { crate::gic::enable(irq) };
        if let Some(high) = level_done {
            if self.pin.is_high() == high {
                return;
            }
        }
        core::future::poll_fn(|cx| {
            WAKERS[n as usize].register(cx.waker());
            if FIRED[n as usize].load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

impl Drop for ExtiInput<'_> {
    fn drop(&mut self) {
        // SAFETY: masking an interrupt can't make a handler run.
        unsafe { crate::gic::disable(self.irq) }
    }
}

impl embedded_hal::digital::ErrorType for ExtiInput<'_> {
    type Error = Infallible;
}

impl embedded_hal::digital::InputPin for ExtiInput<'_> {
    fn is_high(&mut self) -> Result<bool, Infallible> {
        Ok(self.pin.is_high())
    }

    fn is_low(&mut self) -> Result<bool, Infallible> {
        Ok(self.pin.is_low())
    }
}

impl embedded_hal_async::digital::Wait for ExtiInput<'_> {
    async fn wait_for_high(&mut self) -> Result<(), Infallible> {
        ExtiInput::wait_for_high(self).await;
        Ok(())
    }

    async fn wait_for_low(&mut self) -> Result<(), Infallible> {
        ExtiInput::wait_for_low(self).await;
        Ok(())
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), Infallible> {
        ExtiInput::wait_for_rising_edge(self).await;
        Ok(())
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), Infallible> {
        ExtiInput::wait_for_falling_edge(self).await;
        Ok(())
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), Infallible> {
        ExtiInput::wait_for_any_edge(self).await;
        Ok(())
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::mmio::test;
    use crate::peripherals::{EXTI3, P1_7};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    crate::bind_interrupts!(struct Irqs {
        IRQ3 => InterruptHandler<EXTI3>;
    });

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    struct Count(AtomicUsize);
    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn assert_irq<P: IrqPin<EXTI3>>() {}

    fn exti3() -> ExtiInput<'static> {
        test::reset();
        unsafe { ExtiInput::new(P1_7::steal(), EXTI3::steal(), Irqs) }
    }

    fn icr1_bits(ch: u8) -> u16 {
        (test::peek16(crate::pac::INTC.icr1().addr()) >> (2 * ch)) & 0b11
    }

    #[test]
    fn irq_pin_functions_are_generated() {
        assert_irq::<P1_7>();
    }

    #[test]
    fn wait_for_rising_edge_programs_icr1_and_wakes_on_irq() {
        let _serial = serial();
        let mut input = exti3();
        let count = Arc::new(Count(AtomicUsize::new(0)));
        let waker = Waker::from(count.clone());
        let mut cx = Context::from_waker(&waker);
        let irqrr = crate::pac::INTC.irqrr().addr();
        {
            let mut fut = core::pin::pin!(input.wait_for_rising_edge());
            assert!(fut.as_mut().poll(&mut cx).is_pending());
            assert_eq!(icr1_bits(3), 0b10, "rising edge");
            // GICD_ISENABLER1 (0xE820_1104): IRQ3 = 35 is bit 3.
            assert!(test::writes().contains(&(0xE820_1104, 1 << 3)), "IRQ3 enabled");
            test::poke16(irqrr, 1 << 3);
            unsafe {
                <InterruptHandler<EXTI3> as crate::interrupt::Handler<
                    crate::interrupt::typelevel::IRQ3,
                >>::on_interrupt()
            };
            assert_eq!(test::peek16(irqrr) & (1 << 3), 0, "IRQRR cleared");
            assert_eq!(count.0.load(Ordering::SeqCst), 1, "woken");
            assert_eq!(fut.as_mut().poll(&mut cx), Poll::Ready(()));
        }
    }

    #[test]
    fn wait_for_any_edge_sets_both_edges() {
        let _serial = serial();
        let mut input = exti3();
        let mut fut = core::pin::pin!(input.wait_for_any_edge());
        let _ = fut.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        assert_eq!(icr1_bits(3), 0b11);
    }

    #[test]
    fn wait_for_high_returns_at_once_when_the_pin_is_high() {
        let _serial = serial();
        let mut input = exti3();
        // PPR1 bit 7.
        test::poke16(crate::pac::GPIO.ppr(1).addr(), 1 << 7);
        assert!(input.is_high());
        let mut fut = core::pin::pin!(input.wait_for_high());
        assert_eq!(fut.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(()));
    }

    #[test]
    fn a_dropped_wait_masks_the_irq() {
        let _serial = serial();
        let mut input = exti3();
        {
            let mut fut = core::pin::pin!(input.wait_for_falling_edge());
            let _ = fut.as_mut().poll(&mut Context::from_waker(Waker::noop()));
            assert_eq!(icr1_bits(3), 0b01, "falling edge");
        }
        // GICD_ICENABLER1 (0xE820_1184): IRQ3 = 35 is bit 3.
        assert_eq!(test::writes().last(), Some(&(0xE820_1184, 1 << 3)));
    }
}
