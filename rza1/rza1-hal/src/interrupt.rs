//! Typed interrupts. Each GIC interrupt the HAL drives is a type in [`typelevel`]; a driver's
//! interrupt handler is a [`Handler`] for it; [`bind_interrupts!`](crate::bind_interrupts)
//! declares which handler serves which interrupt. A driver's constructor takes that binding and
//! installs the handler with [`bind`], so a driver can't be built without its interrupt wired.

use crate::gic;

/// A GIC interrupt priority: `P0` is the highest. The GIC implements 32 levels, and the CPU
/// interface's priority mask sits at 31, so an interrupt at 31 would never be signalled: the
/// levels offered are `P0`–`P30`. Every interrupt starts at [`Priority::DEFAULT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
#[allow(missing_docs)]
#[rustfmt::skip]
pub enum Priority {
    P0 = 0, P1, P2, P3, P4, P5, P6, P7, P8, P9, P10, P11, P12, P13, P14, P15,
    P16, P17, P18, P19, P20, P21, P22, P23, P24, P25, P26, P27, P28, P29, P30,
}

impl Priority {
    /// What every interrupt starts at: the middle level, so an application can place one above
    /// or below the rest.
    pub const DEFAULT: Self = Priority::P16;

    /// The priority with GIC level `level`, or `None` past `P30`.
    pub const fn from_level(level: u8) -> Option<Self> {
        if level <= Priority::P30 as u8 {
            // SAFETY: `Priority` is `repr(u8)` with every value 0..=30 a variant.
            Some(unsafe { core::mem::transmute::<u8, Priority>(level) })
        } else {
            None
        }
    }
}

impl Default for Priority {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A GIC interrupt.
pub trait Interrupt: 'static {
    /// Its GIC interrupt ID.
    const ID: u16;

    /// Unmask it in the distributor.
    ///
    /// # Safety
    /// Its handler may run as soon as this returns: whatever it touches must be ready, and no
    /// critical section the handler could break may be in progress.
    #[inline]
    unsafe fn enable() {
        // SAFETY: the caller's contract.
        unsafe { gic::enable(Self::ID) }
    }

    /// Mask it in the distributor.
    #[inline]
    fn disable() {
        // SAFETY: masking an interrupt can't make a handler run.
        unsafe { gic::disable(Self::ID) }
    }

    /// Whether it is unmasked.
    #[inline]
    fn is_enabled() -> bool {
        gic::is_enabled(Self::ID)
    }

    /// Whether it is pending.
    #[inline]
    fn is_pending() -> bool {
        gic::is_pending(Self::ID)
    }

    /// Make it pending: its handler runs when it is enabled and nothing of higher priority is
    /// active. For a software-generated interrupt (`SGI0`–`SGI15`), this raises it on this CPU.
    #[inline]
    fn pend() {
        gic::pend(Self::ID)
    }

    /// Clear its pending state. A software-generated interrupt can't be cleared; for those this
    /// does nothing.
    #[inline]
    fn unpend() {
        gic::unpend(Self::ID)
    }

    /// Its priority.
    #[inline]
    fn get_priority() -> Priority {
        // The field only ever holds what `set_priority` or `gic::init` wrote, and both write P0–P30.
        Priority::from_level(gic::priority(Self::ID)).unwrap_or(Priority::P30)
    }

    /// Set its priority.
    #[inline]
    fn set_priority(priority: Priority) {
        // A read-modify-write of the word it shares with three other interrupts.
        critical_section::with(|_| {
            // SAFETY: priority changes can't make a handler run that couldn't before.
            unsafe { gic::set_priority(Self::ID, priority as u8) }
        })
    }
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
            impl $crate::interrupt::Interrupt for $name {
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
        // MTU2 channel n's compare-match A.
        TGI0A = 139, TGI1A = 146, TGI2A = 150, TGI3A = 154, TGI4A = 159,
        // SCIF channel n's receive error, receive-FIFO-full and transmit-FIFO-empty.
        ERI0 = 222, RXI0 = 223, TXI0 = 224, ERI1 = 226, RXI1 = 227, TXI1 = 228,
        ERI2 = 230, RXI2 = 231, TXI2 = 232, ERI3 = 234, RXI3 = 235, TXI3 = 236,
        ERI4 = 238, RXI4 = 239, TXI4 = 240,
        // RSPI channel n's error, receive-buffer-full and transmit-buffer-empty.
        SPEI0 = 270, SPRI0 = 271, SPTI0 = 272, SPEI1 = 273, SPRI1 = 274, SPTI1 = 275,
        SPEI2 = 276, SPRI2 = 277, SPTI2 = 278,
    }

    // The RZ/A1H's RSPI3 and RSPI4.
    #[cfg(feature = "rza1h")]
    mod rspi34 {
        interrupts! {
            SPEI3 = 279, SPRI3 = 280, SPTI3 = 281, SPEI4 = 282, SPRI4 = 283, SPTI4 = 284,
        }
    }
    #[cfg(feature = "rza1h")]
    pub use rspi34::*;

    // SDHI channel n's status (card detect), operation and SDIO interrupts. The RZ/A1H's PAC has
    // no SDHI block, so neither are these.
    #[cfg(feature = "rza1l")]
    mod sdhi {
        interrupts! {
            SDHI0_STATUS = 302, SDHI0_OP = 303, SDHI0_SDIO = 304,
            SDHI1_STATUS = 305, SDHI1_OP = 306, SDHI1_SDIO = 307,
        }
    }
    #[cfg(feature = "rza1l")]
    pub use sdhi::*;

    // OSTM channel n's interrupt. With `time-driver-ostm` the time driver owns both, so they are
    // the HAL's own.
    mod ostm {
        interrupts! { OSTMI0 = 134, OSTMI1 = 135 }
    }
    // Only the device time driver (and the ID tests) use them; a host build has neither.
    #[cfg(all(feature = "time-driver-ostm", any(target_os = "none", test)))]
    pub(crate) use ostm::*;
    #[cfg(not(feature = "time-driver-ostm"))]
    pub use ostm::*;
}

/// Declare which handler serves which interrupt:
///
/// ```ignore
/// rza1_hal::bind_interrupts!(struct Irqs {
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
    fn pend_on_an_spi_sets_its_ispr_bit() {
        test::reset();
        typelevel::USB0::pend();
        // GICD_ISPENDR2 (0xE820_1200 + 2*4), bit 73 - 64 = 9.
        assert_eq!(test::writes(), [(0xE820_1208, 1 << 9)]);
    }

    #[test]
    fn pend_on_an_sgi_writes_sgir_to_self() {
        test::reset();
        typelevel::SGI3::pend();
        // GICD_SGIR: TargetListFilter = 0b10 (this CPU only), SGIINTID = 3.
        assert_eq!(test::writes(), [(0xE820_1F00, (0b10 << 24) | 3)]);
    }

    #[test]
    fn unpend_clears_its_icpr_bit() {
        test::reset();
        typelevel::USB0::unpend();
        // GICD_ICPENDR2 (0xE820_1280 + 2*4), bit 9.
        assert_eq!(test::writes(), [(0xE820_1288, 1 << 9)]);
    }

    #[test]
    fn is_pending_reads_its_ispr_bit() {
        test::reset();
        test::poke32(0xE820_1208, 1 << 9);
        assert!(typelevel::USB0::is_pending());
        assert!(!typelevel::USB1::is_pending());
    }

    #[test]
    fn is_enabled_reads_its_iser_bit() {
        test::reset();
        test::poke32(0xE820_1108, 1 << 9);
        assert!(typelevel::USB0::is_enabled());
        assert!(!typelevel::USB1::is_enabled());
    }

    #[test]
    fn set_priority_writes_its_byte_lane() {
        test::reset();
        typelevel::DMAINT4::set_priority(Priority::P13);
        // DMAINT4 is GIC 45: GICD_IPRIORITYR11 (0xE820_1400 + 11*4), byte lane 1, value 13 << 3.
        assert_eq!(test::writes(), [(0xE820_142C, (13 << 3) << 8)]);
    }

    #[test]
    fn get_priority_reads_back_what_was_set() {
        test::reset();
        typelevel::DMAINT4::set_priority(Priority::P13);
        typelevel::DMAINT5::set_priority(Priority::P2);
        assert_eq!(typelevel::DMAINT4::get_priority(), Priority::P13);
        assert_eq!(typelevel::DMAINT5::get_priority(), Priority::P2);
    }

    #[test]
    fn the_default_priority_is_p16() {
        assert_eq!(Priority::default(), Priority::P16);
    }

    #[test]
    fn gic_init_defaults_every_priority_to_p16() {
        test::reset();
        unsafe { crate::gic::init() };
        let ipr = 0xE820_1400..0xE820_1400 + (crate::gic::INT_ID_TOTAL / 4 + 1) * 4;
        let writes: std::vec::Vec<_> = test::writes()
            .into_iter()
            .filter(|(a, _)| ipr.contains(a))
            .collect();
        assert_eq!(writes.len(), crate::gic::INT_ID_TOTAL / 4 + 1);
        assert!(writes.iter().all(|&(_, v)| v == 0x8080_8080), "{writes:x?}");
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
        unsafe { typelevel::USB0::enable() };
        // GICD_ISENABLER2 (0xE820_1100 + 2*4), bit 73 - 64 = 9.
        assert_eq!(test::writes(), [(0xE820_1108, 1 << 9)]);
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
        typelevel::USB0::disable();
        // GICD_ICENABLER2 (0xE820_1180 + 2*4), bit 73 - 64 = 9.
        assert_eq!(test::writes(), [(0xE820_1188, 1 << 9)]);
    }

    #[test]
    fn rspi_interrupt_ids_match_the_hardware_manual() {
        // SPEIn/SPRIn/SPTIn = 270/271/272 + 3n (Table 7.3).
        assert_eq!(
            [typelevel::SPEI0::ID, typelevel::SPRI0::ID, typelevel::SPTI0::ID],
            [270, 271, 272]
        );
        assert_eq!(typelevel::SPEI1::ID, 273);
        assert_eq!(typelevel::SPTI2::ID, 278);
    }

    #[test]
    fn scif_receive_interrupt_ids_match_the_hardware_manual() {
        // RXIn = 223 + 4n (Table 7.3).
        assert_eq!(
            [
                typelevel::RXI0::ID,
                typelevel::RXI1::ID,
                typelevel::RXI2::ID,
                typelevel::RXI3::ID,
                typelevel::RXI4::ID
            ],
            [223, 227, 231, 235, 239]
        );
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
