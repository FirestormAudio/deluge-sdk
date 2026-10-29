//! Deluge board audio (SSI0 / SSIF0) initialisation.
//!
//! ## Pin assignments
//!
//! | Port.Pin | Mux | Signal       | Direction |
//! |----------|-----|--------------|-----------|
//! | P7.11    | 6   | AUDIO_XOUT   | Output    |
//! | P6.8     | 3   | SSITXD0      | Output    |
//! | P6.9     | 3   | SSISCK0      | Output    |
//! | P6.10    | 3   | SSIWS0       | Output    |
//! | P6.11    | 3   | SSIRXD0      | Input     |
//! | P6.12    | —   | CODEC_POWER  | GPIO out  |
//!
//! `CODEC_POWER` (P6.12) is driven low during SSI initialisation and raised
//! high after the codec's required 5 ms clock-stable delay, following the
//! sequence used by the original C firmware.
//!
//! The SCUX DVU block sits between the CPU audio buffer and the SSIF0
//! transmitter, providing hardware volume control and click-free fades.
//! On the SCUX path ([`init_with_scux`]) SSI0 TX DMA (ch 6) is intentionally
//! **not** started; SCUX drives SSIF0 TX directly via `SSICTRL.SSI0TX`.  SSI0 RX
//! DMA (ch 7) is started as usual.

use rza1_hal::Peri;
#[cfg(feature = "audio-irq")]
use rza1_hal::dmac::BlockDma;
use rza1_hal::dmac::Dma;
use rza1_hal::gpio::{Alternate, Flex, Level, Output};
use rza1_hal::{ostm, ssi};

use crate::board::{AudioPins, AudioSsi, ScuxFfd0Dma, SsiRxDma, SsiTxDma};
use crate::scux_dvu_path;

#[cfg(feature = "audio-irq")]
rza1_hal::bind_interrupts!(struct BlockIrqs {
    DMAINT7 => rza1_hal::dmac::BlockInterruptHandler<SsiRxDma>;
});

/// Delay required between SSI clock start and codec enable, in milliseconds.
const CODEC_POWER_DELAY_MS: u32 = 5;

/// Bring up the codec via the **SCUX** path (SCUX DVU drives SSIF0 TX; SSI RX
/// DMA captures input). Use this only when SCUX is actually needed — USB audio,
/// or an internal rate that differs from the codec. For a same-rate DSP effect
/// prefer [`init`] (direct SSI, no SCUX).
///
/// Configures SSI0 pin-mux + the SCUX DVU path + SSI RX DMA, waits the codec
/// power-on delay, then asserts `CODEC_POWER`.
///
/// **Call after:** `rza1_hal::init` (which starts the time driver), before any audio task starts.
pub fn init_with_scux(
    ssi: Peri<'static, AudioSsi>,
    pins: AudioPins,
    rx: Peri<'static, SsiRxDma>,
    ffd0: Peri<'static, ScuxFfd0Dma>,
) {
    bring_up_scux(pins, || scux_dvu_path::init(ssi, rx, ffd0));
}

/// As [`init_with_scux`], with the SCUX's 2SRC converting `engine_hz` to the codec's 44.1 kHz:
/// samples written into the DVU path's ring at `engine_hz` (48 kHz from a USB audio device,
/// say) play at the codec rate, converted in hardware.
///
/// **Call after:** `rza1_hal::init` (which starts the time driver), before any audio task starts.
pub fn init_with_scux_src(
    ssi: Peri<'static, AudioSsi>,
    pins: AudioPins,
    rx: Peri<'static, SsiRxDma>,
    ffd0: Peri<'static, ScuxFfd0Dma>,
    engine_hz: u32,
) {
    bring_up_scux(pins, || scux_dvu_path::init_with_src(ssi, rx, ffd0, engine_hz));
}

/// Put SSI0's pins and AUDIO_XOUT on their functions, and hold the codec off: the codec-power
/// output, low.
///
/// Each pin keeps PIPC = 1 (the function drives its direction), as the pin-mux always set it,
/// SSITxD0 included, although HW manual Table 41.7 asks for PIPC = 0 on SSITxD0; the SSI driver
/// will apply Table 41.7 when it takes its pins. The pins are muxed in port order, as before.
fn mux_ssi(pins: AudioPins) -> Output<'static> {
    Flex::new(pins.xout).set_as_alternate(Alternate::Alt6); // AUDIO_XOUT (P7_11), codec master clock
    Flex::new(pins.sck).set_as_alternate(Alternate::Alt3); // SSISCK0 (P6_8), I²S BCLK
    Flex::new(pins.ws).set_as_alternate(Alternate::Alt3); // SSIWS0 (P6_9), I²S LRCK
    Flex::new(pins.tx).set_as_alternate(Alternate::Alt3); // SSITxD0 (P6_10), I²S TX data
    Flex::new(pins.rx).set_as_alternate(Alternate::Alt3); // SSIRxD0 (P6_11), I²S RX data
    // CODEC_POWER: low while the SSI clocks stabilise.
    Output::new(pins.codec_power, Level::Low)
}

/// Pin-mux SSI0, hold the codec off, start the SCUX path `path`, wait the codec's power-on
/// delay, then power the codec.
fn bring_up_scux(pins: AudioPins, path: impl FnOnce()) {
    log::debug!("audio: pin-mux SSI0 (P7_11, P6_8-11)");
    let mut codec_power = mux_ssi(pins);

    // ── SCUX path: SSI RX DMA + SCUX → SSIF0 TX ─────────────────────────────
    path();

    // ── 5 ms clock-stable delay before enabling the codec ────────────────────
    // OSTM0 must already be running in free-running mode.
    ostm::delay_ms(CODEC_POWER_DELAY_MS);
    log::debug!("audio: codec power-on delay done");
    // ── Assert CODEC_POWER — codec begins normal operation ───────────────────
    codec_power.set_high();
    log::debug!("audio: CODEC_POWER asserted, codec active");
}

/// Bring up the codec on the **direct SSI TX+RX DMA path** (no SCUX) — the
/// default for same-rate (44.1 kHz) DSP. SCUX is only needed when the internal
/// rate differs from the codec or for USB rate conversion; use
/// [`init_with_scux`] for those.
///
/// Configures SSI0 pin-mux, drives SSIF0 TX directly from the SSI TX DMA
/// ([`ssi::init`]), waits the codec power-on delay, then asserts `CODEC_POWER`.
///
/// **Call after:** `rza1_hal::init` (which starts the time driver), before any audio task starts.
pub fn init(
    ssi: Peri<'static, AudioSsi>,
    pins: AudioPins,
    tx: Peri<'static, SsiTxDma>,
    rx: Peri<'static, SsiRxDma>,
) {
    log::debug!("audio: pin-mux SSI0 (direct, no SCUX)");
    let mut codec_power = mux_ssi(pins);
    let tx = Dma::new_without_interrupt(tx);
    #[cfg(not(feature = "audio-irq"))]
    ssi::init(
        ssi,
        tx,
        Dma::new_without_interrupt(rx),
        crate::system::SSICR,
    );
    // `audio-irq`: RX runs as a per-block descriptor ring; its interrupt is bound before it starts.
    #[cfg(feature = "audio-irq")]
    ssi::init_block_irq(ssi, tx, BlockDma::new(rx, BlockIrqs), crate::system::SSICR);
    ostm::delay_ms(CODEC_POWER_DELAY_MS);
    codec_power.set_high();
    log::debug!("audio: CODEC_POWER asserted (direct SSI path)");
}
