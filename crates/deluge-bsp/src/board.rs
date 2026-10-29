//! The Deluge's peripherals, each in the role the board wires it to. [`Board::new`] finishes the
//! bring-up `rza1_hal::init` starts, then hands the peripherals out by role.

use rza1_hal::Peri;
use rza1_hal::peripherals::*;

/// DMA channel 0: SCUX FFD0 (DVU playback path).
pub type ScuxFfd0Dma = DMA_CH0;
/// DMA channel 1: SCUX FFU0, or USB0 receive DMA ([`Usb0RxDma`]); a firmware uses it for one.
pub type ScuxFfu0Dma = DMA_CH1;
/// DMA channel 1 as USB0's receive DMA (`Rusb1HostDriver::enable_rx_dma`): nothing in the SDK
/// uses SCUX FFU0, so the channel serves the Overbridge stream. Take it from `BoardDma::scux_ffu0`.
pub type Usb0RxDma = ScuxFfu0Dma;
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
/// SDHI1: the SD card slot ([`crate::sd`]).
pub type SdCard = SDHI1;
/// SSIF0: the codec's I²S port ([`crate::audio`]).
pub type AudioSsi = SSIF0;

/// Every DMA channel, by the role the Deluge gives it.
///
/// Each role's driver takes its channel (`bus::init`, `uart::init_*`, `sd::init_hw`, `audio::init*`);
/// a channel the board's firmware doesn't use stays here, unused.
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
    /// SDHI1, the SD card slot.
    pub sd_card: Peri<'static, SdCard>,
    /// SSIF0, the codec's I²S port.
    pub audio_ssi: Peri<'static, AudioSsi>,
    pub dma: BoardDma,
}

/// The priority of the interrupts of USB, the MIDI and PIC UARTs and the SD card: one level, as
/// the drivers set them before priorities became the board's.
const DRIVER_PRIORITY: rza1_hal::interrupt::Priority = rza1_hal::interrupt::Priority::P10;

/// The gate timer's (MTU2_2, [`crate::midi_gate`]): above USB, so gate events are not delayed
/// behind USB servicing.
const GATE_TIMER_PRIORITY: rza1_hal::interrupt::Priority = rza1_hal::interrupt::Priority::P5;

/// The encoder pins' and the encoder poll timer's (MTU2_3, [`crate::encoder`]): one level, so
/// neither preempts the other.
const ENCODER_PRIORITY: rza1_hal::interrupt::Priority = rza1_hal::interrupt::Priority::P14;

/// The trigger clock input's (IRQ6, [`crate::trigger_clock`]): the encoders' level, below the gate
/// timer and the DMAC (13).
const TRIGGER_PRIORITY: rza1_hal::interrupt::Priority = rza1_hal::interrupt::Priority::P14;

/// Give every interrupt the board's drivers enable its priority. The HAL leaves them at
/// `Priority::DEFAULT`; the DMAC's comes from `system::BOARD_CONFIG` and the time driver's from
/// the HAL's `init`.
pub(crate) fn apply_irq_priorities() {
    use rza1_hal::interrupt::{Interrupt, Priority, typelevel::*};
    let set: [(fn(Priority), Priority); 17] = [
        (USB0::set_priority, DRIVER_PRIORITY),
        (USB1::set_priority, DRIVER_PRIORITY),
        (ERI0::set_priority, DRIVER_PRIORITY),
        (TXI0::set_priority, DRIVER_PRIORITY),
        (ERI1::set_priority, DRIVER_PRIORITY),
        (TXI1::set_priority, DRIVER_PRIORITY),
        (SDHI1_STATUS::set_priority, DRIVER_PRIORITY),
        (SDHI1_OP::set_priority, DRIVER_PRIORITY),
        // The gate timer (MTU2_2).
        (TGI2A::set_priority, GATE_TIMER_PRIORITY),
        // The encoder poll timer (MTU2_3) and the encoder pins.
        (TGI3A::set_priority, ENCODER_PRIORITY),
        (IRQ0::set_priority, ENCODER_PRIORITY),
        (IRQ1::set_priority, ENCODER_PRIORITY),
        (IRQ2::set_priority, ENCODER_PRIORITY),
        (IRQ3::set_priority, ENCODER_PRIORITY),
        (IRQ4::set_priority, ENCODER_PRIORITY),
        (IRQ7::set_priority, ENCODER_PRIORITY),
        // The trigger clock input.
        (IRQ6::set_priority, TRIGGER_PRIORITY),
    ];
    for (set_priority, priority) in set {
        set_priority(priority);
    }
}

impl Board {
    /// Finish the bring-up `rza1_hal::init(system::BOARD_CONFIG)` started (the GIC's SDRAM code
    /// window, MMU, L1/L2 caches, SDRAM controller, the board's interrupt priorities), then hand
    /// out the peripherals. The HAL's `init` has already started the time driver.
    ///
    /// Safe: only the one `Peripherals` from `rza1_hal::init` can reach it.
    #[cfg(target_os = "none")]
    pub fn new(p: rza1_hal::Peripherals) -> Self {
        // SAFETY: once, straight after `rza1_hal::init`, before any peripheral is used.
        unsafe {
            rza1_hal::gic::set_sdram_code_window(crate::sdram::WINDOW);
            rza1_hal::mmu::init_and_enable();
            rza1_hal::cache::l1_enable();
            rza1_hal::cache::l2_init();
            crate::sdram::init();
        }
        apply_irq_priorities();
        Self {
            usb0: p.USB0,
            usb1: p.USB1,
            shared_spi: p.RSPI0,
            midi_uart: p.SCIF0,
            pic_uart: p.SCIF1,
            gate_timer: p.MTU2_2,
            encoder_poll_timer: p.MTU2_3,
            sd_card: p.SDHI1,
            audio_ssi: p.SSIF0,
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
    #[test]
    fn board_irq_priorities_match_the_pre_phase1_values() {
        use rza1_hal::interrupt::{Interrupt, Priority, typelevel::*};
        rza1_hal::mmio::test::reset();
        super::apply_irq_priorities();
        // What the drivers set before priorities moved to the board: USB, SCIF and SDHI at 10;
        // the encoder poll timer (MTU2_3), the encoder pins (IRQ0–4, IRQ7) and the trigger
        // input (IRQ6) at 14; the gate timer (MTU2_2) at 5.
        let got = [
            ("USB0", USB0::get_priority()),
            ("USB1", USB1::get_priority()),
            ("ERI0", ERI0::get_priority()),
            ("TXI0", TXI0::get_priority()),
            ("ERI1", ERI1::get_priority()),
            ("TXI1", TXI1::get_priority()),
            ("SDHI1_STATUS", SDHI1_STATUS::get_priority()),
            ("SDHI1_OP", SDHI1_OP::get_priority()),
            ("TGI2A", TGI2A::get_priority()),
            ("TGI3A", TGI3A::get_priority()),
            ("IRQ0", IRQ0::get_priority()),
            ("IRQ1", IRQ1::get_priority()),
            ("IRQ2", IRQ2::get_priority()),
            ("IRQ3", IRQ3::get_priority()),
            ("IRQ4", IRQ4::get_priority()),
            ("IRQ6", IRQ6::get_priority()),
            ("IRQ7", IRQ7::get_priority()),
        ];
        use Priority::*;
        let want = [
            P10, P10, P10, P10, P10, P10, P10, P10, P5, P14, P14, P14, P14, P14, P14, P14, P14,
        ];
        for ((name, got), want) in got.iter().zip(want) {
            assert_eq!(*got, want, "{name}");
        }
    }

    #[test]
    fn board_config_gives_the_time_driver_its_pre_phase1_priority() {
        // The OSTM time driver's interrupts were at 14 when the driver set them itself.
        assert_eq!(
            crate::system::BOARD_CONFIG.time_interrupt_priority,
            rza1_hal::interrupt::Priority::P14
        );
    }

    #[test]
    fn board_config_gives_the_dmac_its_pre_phase1_priority() {
        assert_eq!(
            crate::system::BOARD_CONFIG.dma_interrupt_priority,
            rza1_hal::interrupt::Priority::P13
        );
    }

    use super::*;
    use crate::system::*;
    use rza1_hal::dmac::Channel;

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
        // SSI's channels are the board's choice, made here: the codec ring's TX on 6, RX on 7.
        assert_eq!(SsiTxDma::NUMBER, 6);
        assert_eq!(SsiRxDma::NUMBER, 7);
        assert_eq!(OledDma::NUMBER, crate::oled::OLED_DMA_CH);
        assert_eq!(PicTxDma::NUMBER, PIC_DMA_TX_CH);
        assert_eq!(PicRxDma::NUMBER, PIC_DMA_RX_CH);
        assert_eq!(MidiRxDma::NUMBER, MIDI_DMA_RX_CH);
        assert_eq!(SdTxDma::NUMBER, SD_DMA_TX_CH);
        assert_eq!(SdRxDma::NUMBER, SD_DMA_RX_CH);
    }
}
