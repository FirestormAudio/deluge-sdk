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
        // Software-generated interrupts: an executor run from an SGI handler raises its own.
        SGI0 = 0, SGI1 = 1, SGI2 = 2, SGI3 = 3, SGI4 = 4, SGI5 = 5, SGI6 = 6, SGI7 = 7,
        SGI8 = 8, SGI9 = 9, SGI10 = 10, SGI11 = 11, SGI12 = 12, SGI13 = 13, SGI14 = 14, SGI15 = 15,
        // External pin interrupts IRQ0–IRQ7.
        IRQ0 = 32, IRQ1 = 33, IRQ2 = 34, IRQ3 = 35, IRQ4 = 36, IRQ5 = 37, IRQ6 = 38, IRQ7 = 39,
        // DMAC channel n's completion interrupt is GIC ID 41 + n.
        DMAINT0 = 41, DMAINT1 = 42, DMAINT2 = 43, DMAINT3 = 44,
        DMAINT4 = 45, DMAINT5 = 46, DMAINT6 = 47, DMAINT7 = 48,
        DMAINT8 = 49, DMAINT9 = 50, DMAINT10 = 51, DMAINT11 = 52,
        DMAINT12 = 53, DMAINT13 = 54, DMAINT14 = 55, DMAINT15 = 56,
        USB0 = 73, USB1 = 74,
        // OSTM channel n's interrupt.
        OSTMI0 = 134, OSTMI1 = 135,
        // MTU2 channel n's compare-match A.
        TGI0A = 139, TGI1A = 146, TGI2A = 150, TGI3A = 154, TGI4A = 159,
        // SCIF channel n's receive error and transmit-FIFO-empty.
        ERI0 = 222, TXI0 = 224, ERI1 = 226, TXI1 = 228, ERI2 = 230, TXI2 = 232,
        ERI3 = 234, TXI3 = 236, ERI4 = 238, TXI4 = 240,
        // SDHI channel n's status (card detect), operation and SDIO interrupts.
        SDHI0_STATUS = 302, SDHI0_OP = 303, SDHI0_SDIO = 304,
        SDHI1_STATUS = 305, SDHI1_OP = 306, SDHI1_SDIO = 307,
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
    #[test]
    fn the_4b_interrupt_ids_match_the_hardware_manual() {
        assert_eq!(typelevel::SGI0::ID, 0);
        assert_eq!(typelevel::SGI15::ID, 15);
        assert_eq!(typelevel::OSTMI0::ID, 134);
        assert_eq!(typelevel::OSTMI1::ID, 135);
        assert_eq!(
            [
                typelevel::TGI0A::ID,
                typelevel::TGI1A::ID,
                typelevel::TGI2A::ID,
                typelevel::TGI3A::ID,
                typelevel::TGI4A::ID
            ],
            crate::mtu2::TGIA_IRQ
        );
        // SCIFn: ERI = 222 + 4n, TXI = 224 + 4n (RZ/A1L hardware manual, table A.2).
        assert_eq!(
            [
                typelevel::ERI0::ID,
                typelevel::ERI1::ID,
                typelevel::ERI2::ID,
                typelevel::ERI3::ID,
                typelevel::ERI4::ID
            ],
            [222, 226, 230, 234, 238]
        );
        assert_eq!(
            [
                typelevel::TXI0::ID,
                typelevel::TXI1::ID,
                typelevel::TXI2::ID,
                typelevel::TXI3::ID,
                typelevel::TXI4::ID
            ],
            [224, 228, 232, 236, 240]
        );
    }

    #[test]
    fn disabling_an_interrupt_sets_its_clear_enable_bit() {
        test::reset();
        unsafe { crate::gic::disable(73) };
        // GICD_ICENABLER2 (0xE820_1180 + 2*4), bit 73 - 64 = 9.
        assert_eq!(test::writes(), [(0xE820_1188, 1 << 9)]);
    }

    #[test]
    fn the_4c_interrupt_ids_match_the_hardware_manual() {
        // IRQn (external pin interrupts) are GIC 32 + n.
        assert_eq!(
            [
                typelevel::IRQ0::ID,
                typelevel::IRQ1::ID,
                typelevel::IRQ2::ID,
                typelevel::IRQ3::ID,
                typelevel::IRQ4::ID,
                typelevel::IRQ5::ID,
                typelevel::IRQ6::ID,
                typelevel::IRQ7::ID
            ],
            [32, 33, 34, 35, 36, 37, 38, 39]
        );
        // SDHIn: status (card detect), operation, SDIO.
        assert_eq!(
            [
                typelevel::SDHI0_STATUS::ID,
                typelevel::SDHI0_OP::ID,
                typelevel::SDHI0_SDIO::ID
            ],
            [302, 303, 304]
        );
        assert_eq!(
            [
                typelevel::SDHI1_STATUS::ID,
                typelevel::SDHI1_OP::ID,
                typelevel::SDHI1_SDIO::ID
            ],
            [305, 306, 307]
        );
    }
}
