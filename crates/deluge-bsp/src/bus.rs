//! Shared-bus arbitration for RSPI0.
//!
//! RSPI0 is physically shared between two consumers that need *mutually
//! incompatible* frame modes:
//!
//! * the **OLED** (`oled`) — 8-bit frames streamed via DMAC channel 4, and
//! * the **CV DAC** (`cv_gate`) — 32-bit blocking writes to a MAX5136.
//!
//! Driving either without coordinating corrupts the other's transfer, and
//! switching frame mode underneath an in-flight transfer is silently wrong.
//!
//! The bus is a single owned resource behind an async-aware mutex. Consumers
//! `lock_rspi0().await`, drive the bus through the returned guard, and release
//! it by dropping the guard. The guard may be held across `.await` (e.g. while
//! an OLED DMA transfer completes); other consumers wait. The guard also
//! *tracks the frame mode*, so `enter_8bit` / `enter_32bit` reconfigure only
//! when the mode actually changes.
//!
//! [`init`] brings RSPI0 up and takes the OLED's DMA channel with it, since an OLED frame is an
//! RSPI0 transmit. Until then there is no bus, and locking it panics.
//!
//! See the Advanced developer guide (`docs/advanced-guide.md`, §7 — *Dropping
//! down to the BSP & HAL*) for the RSPI0 arbitration design.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::{MappedMutexGuard, Mutex, MutexGuard};
use rza1_hal::Peri;
use rza1_hal::dmac::{self, Dma};
use rza1_hal::rspi::Rspi;

use crate::board::{OledDma, SharedSpi, SpiPins};

/// RSPI0's SCK rate, shared by the CV DAC and the OLED.
///
/// The OLED sets the limit, not the DAC (good to 20 MHz). The OEL9M0087-W-E datasheet allows 10 MHz (a 100 ns
/// cycle), but the 2026 batch of these panels, the same model, glitches at 8.5 MHz and above; 4 MHz is safe.
const RATE_HZ: u32 = 4_000_000;


rza1_hal::bind_interrupts!(struct Irqs {
    DMAINT4 => dmac::InterruptHandler<OledDma>;
});

/// RSPI0 frame mode, tracked by the bus owner so consumers don't reconfigure
/// the channel blindly on every transfer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Mode not known (a stolen bus): the next `enter_*` always reconfigures.
    Unknown,
    /// 8-bit frames (OLED).
    Bits8,
    /// 32-bit frames (CV DAC).
    Bits32,
}

/// Exclusive-access token for RSPI0, and with it the OLED's DMA channel.
///
/// The only way to obtain one is [`lock_rspi0`] / [`try_lock_rspi0`] (or, in a
/// dying context, [`steal_rspi0`]). Holding it guarantees no other consumer can
/// touch RSPI0.
pub struct Rspi0 {
    mode: Mode,
    rspi: Rspi<'static>,
    oled_dma: Dma<'static>,
}

impl Rspi0 {
    /// Ensure RSPI0 is in 8-bit frame mode (OLED). Reconfigures only if the
    /// channel is not already in 8-bit mode.
    #[inline]
    pub fn enter_8bit(&mut self) {
        if self.mode != Mode::Bits8 {
            self.rspi.configure_8bit();
            self.mode = Mode::Bits8;
        }
    }

    /// Re-apply the 8-bit transfer config (SPDCR/SPCMD0/SPBFCR including the
    /// RX-FIFO reset) **unconditionally**, regardless of the cached mode.
    ///
    /// The cached [`enter_8bit`](Self::enter_8bit) skips reconfiguration when the
    /// channel is already in 8-bit mode — fine for SPDCR/SPCMD0, which persist,
    /// but not for the RX FIFO: a full-duplex OLED DMA clocks 768 dummy bytes
    /// back into it every frame, and `SPBFCR.RXRST` must be re-asserted each time
    /// or the next transfer mis-paces (visible as a horizontal "stretch"). The C
    /// firmware (rza1) likewise rewrites SPBFCR every frame.
    /// Call once per OLED frame before arming the DMA.
    #[inline]
    pub fn reconfigure_8bit(&mut self) {
        self.rspi.configure_8bit();
        self.mode = Mode::Bits8;
    }

    /// Ensure RSPI0 is in 32-bit frame mode (CV DAC). Reconfigures only if the
    /// channel is not already in 32-bit mode.
    #[inline]
    pub fn enter_32bit(&mut self) {
        if self.mode != Mode::Bits32 {
            self.rspi.configure_32bit();
            self.mode = Mode::Bits32;
        }
    }

    /// Send one byte in 8-bit mode. Call [`enter_8bit`](Self::enter_8bit) first.
    #[inline]
    pub fn send8(&mut self, byte: u8) {
        self.rspi.send8(byte);
    }

    /// Blocking 32-bit send. Call [`enter_32bit`](Self::enter_32bit) first.
    #[inline]
    pub fn send32_blocking(&mut self, word: u32) {
        self.rspi.send32_blocking(word);
    }

    /// Poll until the shift register has drained (TEND=1). Call before
    /// de-asserting chip-select so the peripheral receives the full frame.
    #[inline]
    pub fn wait_end(&mut self) {
        self.rspi.wait_end();
    }

    /// The OLED's DMA channel, which transmits on RSPI0: held with the bus, so a frame's DMA
    /// can't run while a CV write drives RSPI0.
    pub(crate) fn oled_dma(&mut self) -> &mut Dma<'static> {
        &mut self.oled_dma
    }
}

/// The bus, once [`init`] has run, behind an async mutex.
///
/// `CriticalSectionRawMutex` (not an async-only raw mutex) is deliberate: it
/// lets [`try_lock_rspi0`] work outside the executor (boot).
static RSPI0: Mutex<CriticalSectionRawMutex, Option<Rspi0>> = Mutex::new(None);

/// Type of the guard returned by [`lock_rspi0`].
pub type Rspi0Guard = MappedMutexGuard<'static, CriticalSectionRawMutex, Rspi0>;

/// Bring RSPI0 up as the shared bus: its SCK and MOSI pins muxed, an SPI master at 4 MHz in
/// 32-bit frames, and the OLED's DMA channel aimed at its transmit register.
///
/// Firmware calls it once with the [`crate::Board`]'s `shared_spi` and `dma.oled`, before
/// [`crate::cv_gate::init`] or [`crate::oled::init`]. The SDK calls it the first time an app
/// uses either.
///
/// # Panics
/// If the bus is already up (only reachable with stolen peripherals).
pub fn init(spi: Peri<'static, SharedSpi>, pins: SpiPins, oled_dma: Peri<'static, OledDma>) {
    // Before touching RSPI0: reconfiguring a live bus would corrupt its transfer in flight.
    assert!(!is_initialised(), "RSPI0's bus is already up: bus::init runs once");
    let rspi = Rspi::new_txonly(spi, pins.sck, pins.mosi, RATE_HZ);
    let mut oled_dma = Dma::new(oled_dma, Irqs);
    // SAFETY: every OLED transfer writes SPDR's low byte, the transmit register of the RSPI0
    // this bus owns.
    unsafe {
        oled_dma.init_register_mode(
            crate::oled::OLED_CHCFG,
            crate::oled::RSPI0_SPDR_LL,
            crate::oled::OLED_DMA_DMARS,
        )
    };
    let bus = Rspi0 { mode: Mode::Bits32, rspi, oled_dma };
    match RSPI0.try_lock() {
        // Checked empty above, and nothing can fill it in between: RSPI0 is owned once.
        Ok(mut slot) => *slot = Some(bus),
        Err(_) => unreachable!("RSPI0's bus is locked before it exists"),
    }
}

/// Whether [`init`] has run.
pub fn is_initialised() -> bool {
    // Only an initialised bus can be held: locking an empty one panics, which releases it.
    RSPI0.try_lock().map_or(true, |slot| slot.is_some())
}

fn live(guard: MutexGuard<'static, CriticalSectionRawMutex, Option<Rspi0>>) -> Rspi0Guard {
    MutexGuard::map(guard, |slot| {
        slot.as_mut()
            .expect("RSPI0 isn't up: call deluge_bsp::bus::init first")
    })
}

/// Acquire exclusive RSPI0 access, awaiting if another consumer holds it.
///
/// The guard may be held across `.await` (e.g. across an OLED DMA completion).
///
/// # Panics
/// If [`init`] hasn't run.
pub async fn lock_rspi0() -> Rspi0Guard {
    live(RSPI0.lock().await)
}

/// Try to acquire RSPI0 without awaiting; returns `None` if it is already held.
///
/// Intended for single-threaded startup (`cv_gate::init`) before the executor
/// runs, where contention is impossible and `.await` is unavailable.
///
/// # Panics
/// If [`init`] hasn't run.
pub fn try_lock_rspi0() -> Option<Rspi0Guard> {
    RSPI0.try_lock().ok().map(live)
}

/// Fabricate an RSPI0 token, **bypassing the mutex**.
///
/// For use only when the normal locking discipline cannot apply — i.e. a panic
/// handler that must draw to the OLED but cannot `.await` and may run with the
/// executor dead. Safe there because panic is single-threaded with interrupts
/// masked. Never call this from normal code.
///
/// # Safety
/// [`init`] has run, and no other RSPI0 consumer is or will be active for the lifetime of the
/// returned token (e.g. inside a panic handler with IRQs masked).
pub unsafe fn steal_rspi0() -> Rspi0 {
    // SAFETY: the caller's contract.
    unsafe {
        Rspi0 {
            mode: Mode::Unknown,
            rspi: Rspi::steal::<SharedSpi>(),
            oled_dma: Dma::steal::<OledDma>(),
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use rza1_hal::mmio::test;
    use rza1_hal::peripherals::{DMA_CH4, P6_0, P6_2, RSPI0};

    fn test_pins() -> SpiPins {
        unsafe { SpiPins { sck: P6_0::steal(), mosi: P6_2::steal() } }
    }

    #[test]
    fn the_bus_comes_up_in_32_bit_mode_with_the_oled_dma_aimed_at_spdr() {
        rza1_hal::clocks::init(crate::system::CLOCKS);
        test::reset();

        assert!(!is_initialised());
        // Before init there is no bus: locking one must panic, not hand out an unconfigured RSPI0.
        assert!(std::panic::catch_unwind(|| try_lock_rspi0().is_some()).is_err());

        init(unsafe { RSPI0::steal() }, test_pins(), unsafe { DMA_CH4::steal() });
        assert!(is_initialised());
        let bus = try_lock_rspi0().expect("nobody holds the bus");
        assert_eq!(bus.mode, Mode::Bits32);
        // DMA channel 4 (registers at 0xE820_0100): N0DA is SPDR's low byte, CHCFG the OLED's.
        assert_eq!(test::peek32(0xE820_0104), crate::oled::RSPI0_SPDR_LL);
        assert_eq!(test::peek32(0xE820_012C), crate::oled::OLED_CHCFG);
        // SCK (P6_0) and MOSI (P6_2) are muxed to RSPI0 (alternative 3: PFCE6 at 0xFCFE_3618):
        // without them RSPI0 shifts the OLED's frames out to nothing.
        assert_eq!(test::peek16(0xFCFE_3618) & 0b101, 0b101);
        drop(bus);

        // A second bus over the same RSPI0 would silently drop the live one and its DMA channel.
        let again = std::panic::catch_unwind(|| init(unsafe { RSPI0::steal() }, test_pins(), unsafe { DMA_CH4::steal() }));
        assert!(again.is_err(), "bus::init must refuse to replace a live bus");
    }
}
