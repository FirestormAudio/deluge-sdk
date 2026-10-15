//! Typed interrupts. Each GIC interrupt the HAL drives is a type in [`typelevel`]; a driver's
//! interrupt handler is a [`Handler`] for it; [`bind_interrupts!`](crate::bind_interrupts)
//! declares which handler serves which interrupt. A driver's constructor takes that binding and
//! installs the handler with [`bind`], so a driver can't be built without its interrupt wired.

use crate::gic;

/// A GIC interrupt.
pub trait Interrupt: 'static {
    /// Its GIC interrupt ID.
    const ID: u16;
}

/// An interrupt handler for `I`.
pub trait Handler<I: Interrupt> {
    /// Runs when `I` fires: in IRQ context (SYS mode, with IRQs re-enabled for nesting).
    ///
    /// # Safety
    /// Only the GIC dispatcher calls it, for `I`.
    unsafe fn on_interrupt();
}

/// Proof that `H` is bound to `I`.
///
/// # Safety
/// Implemented only by [`bind_interrupts!`](crate::bind_interrupts).
pub unsafe trait Binding<I: Interrupt, H: Handler<I>> {}

/// Install `H` as the handler for `I`. A driver's constructor calls this with the binding it
/// takes.
pub fn bind<I: Interrupt, H: Handler<I>>(_binding: impl Binding<I, H>) {
    gic::set_handler(I::ID, dispatch_to::<I, H>);
}

fn dispatch_to<I: Interrupt, H: Handler<I>>() {
    // SAFETY: installed only for `I::ID`, so the dispatcher calls this for `I`.
    unsafe { H::on_interrupt() }
}

/// The interrupt types, by name.
pub mod typelevel {
    macro_rules! interrupts {
        ($($name:ident = $id:expr),* $(,)?) => {$(
            #[allow(non_camel_case_types)]
            #[doc = concat!("GIC interrupt ", stringify!($id), ".")]
            pub enum $name {}
            impl super::Interrupt for $name {
                const ID: u16 = $id;
            }
        )*};
    }

    interrupts! {
        // DMAC channel n's completion interrupt is GIC ID 41 + n.
        DMAINT0 = 41, DMAINT1 = 42, DMAINT2 = 43, DMAINT3 = 44,
        DMAINT4 = 45, DMAINT5 = 46, DMAINT6 = 47, DMAINT7 = 48,
        DMAINT8 = 49, DMAINT9 = 50, DMAINT10 = 51, DMAINT11 = 52,
        DMAINT12 = 53, DMAINT13 = 54, DMAINT14 = 55, DMAINT15 = 56,
        USB0 = 73, USB1 = 74,
    }
}

/// Declare which handler serves which interrupt:
///
/// ```ignore
/// rza1l_hal::bind_interrupts!(struct Irqs {
///     USB0 => MyUsbHandler;
/// });
/// ```
///
/// One handler per interrupt: a GIC slot holds one function.
#[macro_export]
macro_rules! bind_interrupts {
    ($vis:vis struct $name:ident { $($irq:ident => $handler:ty;)* }) => {
        #[derive(Copy, Clone)]
        $vis struct $name;
        $(
            // SAFETY: this macro implements `Binding` only for the pairs it is given.
            unsafe impl $crate::interrupt::Binding<$crate::interrupt::typelevel::$irq, $handler>
                for $name {}
        )*
    };
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::mmio::test;
    use core::sync::atomic::{AtomicU32, Ordering};

    static CALLS: AtomicU32 = AtomicU32::new(0);

    struct CountingHandler;
    impl Handler<typelevel::USB1> for CountingHandler {
        unsafe fn on_interrupt() {
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
    }

    crate::bind_interrupts!(struct TestIrqs { USB1 => CountingHandler; });

    #[test]
    fn a_bound_handler_runs_for_its_interrupt() {
        bind(TestIrqs);
        let handler = crate::gic::handler(typelevel::USB1::ID).expect("USB1 has a handler");
        handler();
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn interrupt_ids_match_the_hardware_manual() {
        assert_eq!(typelevel::DMAINT0::ID, 41);
        assert_eq!(typelevel::DMAINT15::ID, 56);
        assert_eq!(typelevel::USB0::ID, crate::usb::USB0_IRQ);
        assert_eq!(typelevel::USB1::ID, crate::usb::USB1_IRQ);
    }

    #[test]
    fn enabling_an_interrupt_sets_its_distributor_bit() {
        test::reset();
        unsafe { crate::gic::enable(73) };
        // GICD_ISENABLER2 (0xE820_1100 + 2*4), bit 73 - 64 = 9.
        assert_eq!(test::writes(), [(0xE820_1108, 1 << 9)]);
    }

    #[test]
    fn setting_a_priority_writes_its_byte_lane() {
        test::reset();
        unsafe { crate::gic::set_priority(45, 13) };
        // GICD_IPRIORITYR11 (0xE820_1400 + 11*4), byte lane 45 % 4 = 1, value 13 << 3.
        assert_eq!(test::writes(), [(0xE820_142C, (13 << 3) << 8)]);
    }
}
