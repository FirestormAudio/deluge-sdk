//! The Deluge's peripherals, each in the role the board wires it to. [`Board::new`] finishes the
//! bring-up `rza1l_hal::init` starts, then hands the peripherals out by role.

use rza1l_hal::Peri;
use rza1l_hal::peripherals::*;

/// DMA channel 0: SCUX FFD0 (DVU playback path).
pub type ScuxFfd0Dma = DMA_CH0;
/// DMA channel 1: SCUX FFU0.
pub type ScuxFfu0Dma = DMA_CH1;
/// DMA channel 2: SCUX FFD2 (USB TX SRC path).
pub type ScuxFfd2Dma = DMA_CH2;
/// DMA channel 3: SCUX FFD1 (USB RX SRC path).
pub type ScuxFfd1Dma = DMA_CH3;
/// DMA channel 4: the OLED's RSPI0 transmit.
pub type OledDma = DMA_CH4;
/// DMA channel 5: SCUX FFU1.
pub type ScuxFfu1Dma = DMA_CH5;
/// DMA channel 6: SSIF0 transmit (codec out).
pub type SsiTxDma = DMA_CH6;
/// DMA channel 7: SSIF0 receive (codec in).
pub type SsiRxDma = DMA_CH7;
/// DMA channel 8: SCUX FFU2.
pub type ScuxFfu2Dma = DMA_CH8;
/// DMA channel 9: SCUX FFU3.
pub type ScuxFfu3Dma = DMA_CH9;
/// DMA channel 10: PIC UART transmit.
pub type PicTxDma = DMA_CH10;
/// DMA channel 11: SD card transmit.
pub type SdTxDma = DMA_CH11;
/// DMA channel 12: PIC UART receive.
pub type PicRxDma = DMA_CH12;
/// DMA channel 13: MIDI UART receive.
pub type MidiRxDma = DMA_CH13;
/// DMA channel 14: SD card receive.
pub type SdRxDma = DMA_CH14;
/// DMA channel 15: SCUX FFD3.
pub type ScuxFfd3Dma = DMA_CH15;

/// RSPI0: the OLED and the CV DAC share it ([`crate::bus`]).
pub type SharedSpi = RSPI0;
/// SCIF0: DIN MIDI ([`crate::uart::init_midi`]).
pub type MidiScif = SCIF0;
/// SCIF1: the PIC32 link ([`crate::uart::init_pic`]).
pub type PicScif = SCIF1;
/// MTU2 channel 2: the MIDI/CV gate one-shot ([`crate::midi_gate`]).
pub type GateTimer = MTU2_2;
/// MTU2 channel 3: the encoder pin poll ([`crate::encoder`]).
pub type EncoderPollTimer = MTU2_3;

/// Every DMA channel, by the role the Deluge gives it.
///
/// Until Phase 4b each channel is already in use by the matching BSP driver (OLED, SD, audio,
/// PIC/MIDI UARTs, SCUX paths), which still programs it by number through `dmac::raw`. Don't
/// build a `dmac::Dma` from a role here alongside that driver: the channel would have two owners.
pub struct BoardDma {
    pub scux_ffd0: Peri<'static, ScuxFfd0Dma>,
    pub scux_ffu0: Peri<'static, ScuxFfu0Dma>,
    pub scux_ffd2: Peri<'static, ScuxFfd2Dma>,
    pub scux_ffd1: Peri<'static, ScuxFfd1Dma>,
    pub oled: Peri<'static, OledDma>,
    pub scux_ffu1: Peri<'static, ScuxFfu1Dma>,
    pub ssi_tx: Peri<'static, SsiTxDma>,
    pub ssi_rx: Peri<'static, SsiRxDma>,
    pub scux_ffu2: Peri<'static, ScuxFfu2Dma>,
    pub scux_ffu3: Peri<'static, ScuxFfu3Dma>,
    pub pic_tx: Peri<'static, PicTxDma>,
    pub sd_tx: Peri<'static, SdTxDma>,
    pub pic_rx: Peri<'static, PicRxDma>,
    pub midi_rx: Peri<'static, MidiRxDma>,
    pub sd_rx: Peri<'static, SdRxDma>,
    pub scux_ffd3: Peri<'static, ScuxFfd3Dma>,
}

/// The Deluge, brought up, with its peripherals by role.
pub struct Board {
    /// USB0: the device/host port on the back panel.
    pub usb0: Peri<'static, USB0>,
    /// USB1: not wired on the Deluge.
    pub usb1: Peri<'static, USB1>,
    /// RSPI0, shared by the OLED and the CV DAC.
    pub shared_spi: Peri<'static, SharedSpi>,
    /// SCIF0, DIN MIDI.
    pub midi_uart: Peri<'static, MidiScif>,
    /// SCIF1, the PIC32 link.
    pub pic_uart: Peri<'static, PicScif>,
    /// MTU2 channel 2, the MIDI/CV gate one-shot.
    pub gate_timer: Peri<'static, GateTimer>,
    /// MTU2 channel 3, the encoder pin poll.
    pub encoder_poll_timer: Peri<'static, EncoderPollTimer>,
    pub dma: BoardDma,
}

impl Board {
    /// Finish the bring-up `rza1l_hal::init(system::BOARD_CONFIG)` started (the GIC's SDRAM code
    /// window, MMU, L1/L2 caches, SDRAM controller, OSTM clock and the Embassy time driver),
    /// then hand out the peripherals.
    ///
    /// Safe: only the one `Peripherals` from `rza1l_hal::init` can reach it.
    #[cfg(target_os = "none")]
    pub fn new(p: rza1l_hal::Peripherals) -> Self {
        // SAFETY: once, straight after `rza1l_hal::init`, before any peripheral is used.
        unsafe {
            rza1l_hal::gic::set_sdram_code_window(crate::sdram::WINDOW);
            rza1l_hal::mmu::init_and_enable();
            rza1l_hal::cache::l1_enable();
            rza1l_hal::cache::l2_init();
            crate::sdram::init();
            rza1l_hal::ostm::enable_clock();
            rza1l_hal::time_driver::init();
        }
        Self {
            usb0: p.USB0,
            usb1: p.USB1,
            shared_spi: p.RSPI0,
            midi_uart: p.SCIF0,
            pic_uart: p.SCIF1,
            gate_timer: p.MTU2_2,
            encoder_poll_timer: p.MTU2_3,
            dma: BoardDma {
                scux_ffd0: p.DMA_CH0,
                scux_ffu0: p.DMA_CH1,
                scux_ffd2: p.DMA_CH2,
                scux_ffd1: p.DMA_CH3,
                oled: p.DMA_CH4,
                scux_ffu1: p.DMA_CH5,
                ssi_tx: p.DMA_CH6,
                ssi_rx: p.DMA_CH7,
                scux_ffu2: p.DMA_CH8,
                scux_ffu3: p.DMA_CH9,
                pic_tx: p.DMA_CH10,
                sd_tx: p.DMA_CH11,
                pic_rx: p.DMA_CH12,
                midi_rx: p.DMA_CH13,
                sd_rx: p.DMA_CH14,
                scux_ffd3: p.DMA_CH15,
            },
        }
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;
    use crate::system::*;
    use rza1l_hal::dmac::Channel;

    #[test]
    fn board_dma_roles_name_the_channels_the_drivers_use() {
        // The drivers that still take channel numbers use these constants; each role's
        // singleton must be the same channel, or the channel has two owners.
        assert_eq!(ScuxFfd0Dma::NUMBER, SCUX_FFD0_DMA_CH);
        assert_eq!(ScuxFfd1Dma::NUMBER, SCUX_FFD1_DMA_CH);
        assert_eq!(ScuxFfd2Dma::NUMBER, SCUX_FFD2_DMA_CH);
        assert_eq!(ScuxFfd3Dma::NUMBER, SCUX_FFD3_DMA_CH);
        assert_eq!(ScuxFfu0Dma::NUMBER, SCUX_FFU0_DMA_CH);
        assert_eq!(ScuxFfu1Dma::NUMBER, SCUX_FFU1_DMA_CH);
        assert_eq!(ScuxFfu2Dma::NUMBER, SCUX_FFU2_DMA_CH);
        assert_eq!(ScuxFfu3Dma::NUMBER, SCUX_FFU3_DMA_CH);
        assert_eq!(SsiTxDma::NUMBER, SSI_CONFIG.tx_dma_ch);
        assert_eq!(SsiRxDma::NUMBER, SSI_CONFIG.rx_dma_ch);
        assert_eq!(OledDma::NUMBER, crate::oled::OLED_DMA_CH);
        assert_eq!(PicTxDma::NUMBER, PIC_DMA_TX_CH);
        assert_eq!(PicRxDma::NUMBER, PIC_DMA_RX_CH);
        assert_eq!(MidiRxDma::NUMBER, MIDI_DMA_RX_CH);
        assert_eq!(SdTxDma::NUMBER, SD_DMA_TX_CH);
        assert_eq!(SdRxDma::NUMBER, SD_DMA_RX_CH);
    }
}
