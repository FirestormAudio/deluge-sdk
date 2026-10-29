//! The Deluge's peripherals, each in the role the board wires it to. [`Board::new`] finishes the
//! bring-up `rza1_hal::init` starts, then hands the peripherals out by role.

use rza1_hal::Peri;
use rza1_hal::gpio::AnyPin;
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

/// DIN MIDI's pins: TxD0 on P6_15, RxD0 on P6_14 ([`crate::uart::init_midi`]).
pub struct MidiPins {
    /// TxD0.
    pub tx: Peri<'static, P6_15>,
    /// RxD0.
    pub rx: Peri<'static, P6_14>,
}

/// The PIC32 link's pins: TxD1 on P3_15, RxD1 on P1_9 ([`crate::uart::init_pic`]).
pub struct PicPins {
    /// TxD1.
    pub tx: Peri<'static, P3_15>,
    /// RxD1.
    pub rx: Peri<'static, P1_9>,
}

/// The shared SPI bus's pins: RSPCK0 on P6_0, MOSI0 on P6_2 ([`crate::bus`]). The OLED and the
/// CV DAC only listen, so there is no MISO.
pub struct SpiPins {
    /// RSPCK0.
    pub sck: Peri<'static, P6_0>,
    /// MOSI0.
    pub mosi: Peri<'static, P6_2>,
}

/// The SD slot's pins: SD port 1 on P7_0–P7_7 ([`crate::sd`]).
pub struct SdPins {
    /// SD_CLK_1.
    pub clk: Peri<'static, P7_4>,
    /// SD_CMD_1.
    pub cmd: Peri<'static, P7_5>,
    /// SD_D0_1.
    pub d0: Peri<'static, P7_3>,
    /// SD_D1_1.
    pub d1: Peri<'static, P7_2>,
    /// SD_D2_1.
    pub d2: Peri<'static, P7_7>,
    /// SD_D3_1.
    pub d3: Peri<'static, P7_6>,
    /// SD_CD_1, card detect.
    pub cd: Peri<'static, P7_0>,
    /// SD_WP_1, write protect.
    pub wp: Peri<'static, P7_1>,
}

/// The codec's pins ([`crate::audio`]): its master clock, its I²S port on SSIF0, and its power.
pub struct AudioPins {
    /// AUDIO_XOUT, the codec's master clock (P7_11).
    pub xout: Peri<'static, P7_11>,
    /// SSISCK0, the bit clock (P6_8).
    pub sck: Peri<'static, P6_8>,
    /// SSIWS0, the word clock (P6_9).
    pub ws: Peri<'static, P6_9>,
    /// SSITxD0, I²S data to the codec (P6_10).
    pub tx: Peri<'static, P6_10>,
    /// SSIRxD0, I²S data from the codec (P6_11).
    pub rx: Peri<'static, P6_11>,
    /// The codec's power, high = on (P6_12).
    pub codec_power: Peri<'static, P6_12>,
}

/// The jack-detect inputs (a present jack reads high) and the speaker amplifier's enable
/// ([`crate::jacks`]).
pub struct JackPins {
    /// Headphone out (P6_5).
    pub headphone: Peri<'static, P6_5>,
    /// Line in (P6_6).
    pub line_in: Peri<'static, P6_6>,
    /// Microphone in (P7_9).
    pub mic: Peri<'static, P7_9>,
    /// Line out, left (P6_3).
    pub line_out_l: Peri<'static, P6_3>,
    /// Line out, right (P6_4).
    pub line_out_r: Peri<'static, P6_4>,
    /// The speaker amplifier's enable, high = on (P4_1).
    pub speaker_enable: Peri<'static, P4_1>,
}

/// The CV DAC's chip select and the four gate outputs ([`crate::cv_gate`]).
pub struct CvGatePins {
    /// The MAX5136's chip select, active low (P6_1).
    pub cs: Peri<'static, P6_1>,
    /// Gate 0 (P2_7). V-trig: low = gate on.
    pub gate0: Peri<'static, P2_7>,
    /// Gate 1 (P2_8).
    pub gate1: Peri<'static, P2_8>,
    /// Gate 2 (P2_9).
    pub gate2: Peri<'static, P2_9>,
    /// Gate 3 (P4_0).
    pub gate3: Peri<'static, P4_0>,
}

/// The six encoders' pins on port 1 ([`crate::encoder`]), each in its encoder's order: `irq` is
/// the edge that interrupts (IRQn), `comp` the quadrature partner read alongside it.
pub struct EncoderPins {
    /// P1_11, P1_6, P1_0, P1_5, P1_8, P1_3.
    pub irq: [Peri<'static, AnyPin>; 6],
    /// P1_12, P1_7, P1_15, P1_4, P1_10, P1_2.
    pub comp: [Peri<'static, AnyPin>; 6],
}

/// The SDRAM bus's pins, all on alternative 1 ([`crate::sdram`]). `Board::new` hands them to
/// the SDRAM controller's bring-up, which keeps them.
pub struct SdramPins {
    /// A1–A15: P3_0–P3_14.
    pub address: [Peri<'static, AnyPin>; 15],
    /// D0–D15: P5_0–P5_15.
    pub data: [Peri<'static, AnyPin>; 16],
    /// CS3, RAS, CAS, CKE, WE0, WE1, RD/WR: P2_0–P2_6.
    pub control: [Peri<'static, AnyPin>; 7],
}

/// The pins, grouped by the role the Deluge wires them to.
pub struct BoardPins {
    /// DIN MIDI.
    pub midi: MidiPins,
    /// The PIC32 link.
    pub pic: PicPins,
    /// The shared SPI bus (OLED, CV DAC).
    pub spi: SpiPins,
    /// The SD slot.
    pub sd: SdPins,
    /// The codec.
    pub audio: AudioPins,
    /// The jack detects and the speaker enable.
    pub jacks: JackPins,
    /// The CV DAC's chip select and the gates.
    pub cv_gate: CvGatePins,
    /// The encoders.
    pub encoders: EncoderPins,
    /// The trigger-clock input (P1_14, IRQ6; [`crate::trigger_clock`]).
    pub trigger: Peri<'static, P1_14>,
    /// The SYNC LED (P6_7).
    pub sync_led: Peri<'static, P6_7>,
}

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
    /// The pins, by role.
    pub pins: BoardPins,
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
        let (board, sdram) = Self::split(p);
        // SAFETY: once, straight after `rza1_hal::init`, before any peripheral is used.
        unsafe {
            rza1_hal::gic::set_sdram_code_window(crate::sdram::WINDOW);
            rza1_hal::mmu::init_and_enable();
            rza1_hal::cache::l1_enable();
            rza1_hal::cache::l2_init();
            crate::sdram::init(sdram);
        }
        apply_irq_priorities();
        board
    }

    /// The peripherals by role, and the SDRAM bus's pins for its bring-up. Touches no register.
    #[cfg_attr(not(any(target_os = "none", test)), allow(dead_code))]
    pub(crate) fn split(p: rza1_hal::Peripherals) -> (Self, SdramPins) {
        let sdram = SdramPins {
            address: [
                p.P3_0.into(), p.P3_1.into(), p.P3_2.into(), p.P3_3.into(), p.P3_4.into(),
                p.P3_5.into(), p.P3_6.into(), p.P3_7.into(), p.P3_8.into(), p.P3_9.into(),
                p.P3_10.into(), p.P3_11.into(), p.P3_12.into(), p.P3_13.into(), p.P3_14.into(),
            ],
            data: [
                p.P5_0.into(), p.P5_1.into(), p.P5_2.into(), p.P5_3.into(), p.P5_4.into(),
                p.P5_5.into(), p.P5_6.into(), p.P5_7.into(), p.P5_8.into(), p.P5_9.into(),
                p.P5_10.into(), p.P5_11.into(), p.P5_12.into(), p.P5_13.into(), p.P5_14.into(),
                p.P5_15.into(),
            ],
            control: [
                p.P2_0.into(), p.P2_1.into(), p.P2_2.into(), p.P2_3.into(), p.P2_4.into(),
                p.P2_5.into(), p.P2_6.into(),
            ],
        };
        let board = Self {
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
            pins: BoardPins {
                midi: MidiPins { tx: p.P6_15, rx: p.P6_14 },
                pic: PicPins { tx: p.P3_15, rx: p.P1_9 },
                spi: SpiPins { sck: p.P6_0, mosi: p.P6_2 },
                sd: SdPins {
                    clk: p.P7_4,
                    cmd: p.P7_5,
                    d0: p.P7_3,
                    d1: p.P7_2,
                    d2: p.P7_7,
                    d3: p.P7_6,
                    cd: p.P7_0,
                    wp: p.P7_1,
                },
                audio: AudioPins {
                    xout: p.P7_11,
                    sck: p.P6_8,
                    ws: p.P6_9,
                    tx: p.P6_10,
                    rx: p.P6_11,
                    codec_power: p.P6_12,
                },
                jacks: JackPins {
                    headphone: p.P6_5,
                    line_in: p.P6_6,
                    mic: p.P7_9,
                    line_out_l: p.P6_3,
                    line_out_r: p.P6_4,
                    speaker_enable: p.P4_1,
                },
                cv_gate: CvGatePins {
                    cs: p.P6_1,
                    gate0: p.P2_7,
                    gate1: p.P2_8,
                    gate2: p.P2_9,
                    gate3: p.P4_0,
                },
                encoders: EncoderPins {
                    irq: [
                        p.P1_11.into(), p.P1_6.into(), p.P1_0.into(),
                        p.P1_5.into(), p.P1_8.into(), p.P1_3.into(),
                    ],
                    comp: [
                        p.P1_12.into(), p.P1_7.into(), p.P1_15.into(),
                        p.P1_4.into(), p.P1_10.into(), p.P1_2.into(),
                    ],
                },
                trigger: p.P1_14,
                sync_led: p.P6_7,
            },
        };
        (board, sdram)
    }
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    #[test]
    fn audio_pins_are_named_by_their_ssi_signals() {
        // HW manual Table 41.23 (P6), alternative 3: P6_8 SSISCK0, P6_9 SSIWS0, P6_10 SSITxD0,
        // P6_11 SSIRxD0.
        use rza1_hal::gpio::Pin;
        let (board, _) = super::Board::split(unsafe { rza1_hal::Peripherals::steal() });
        let a = &board.pins.audio;
        assert_eq!((a.sck.bit(), a.ws.bit(), a.tx.bit(), a.rx.bit()), (8, 9, 10, 11));
    }

    #[test]
    fn board_pins_name_every_pin_the_bsp_drives() {
        use rza1_hal::gpio::Pin;
        // Every (port, bit) the BSP and the facade drove through rza1_hal::gpio's free functions
        // before the pins were owned, by the role that drove it.
        let mut want: std::vec::Vec<(u8, u8)> = std::vec::Vec::new();
        want.extend((0..=14).map(|b| (3, b))); // SDRAM address
        want.extend((0..=15).map(|b| (5, b))); // SDRAM data
        want.extend((0..=6).map(|b| (2, b))); // SDRAM control
        want.extend([(6, 15), (6, 14), (3, 15), (1, 9), (6, 0), (6, 2)]); // MIDI, PIC, SPI
        want.extend((0..=7).map(|b| (7, b))); // SD
        want.extend([(7, 11), (6, 8), (6, 9), (6, 10), (6, 11), (6, 12)]); // audio, codec power
        want.extend([(6, 5), (6, 6), (7, 9), (6, 3), (6, 4), (4, 1)]); // jacks, speaker
        want.extend([(6, 1), (2, 7), (2, 8), (2, 9), (4, 0)]); // CV chip select, gates
        want.extend([(1, 11), (1, 6), (1, 0), (1, 5), (1, 8), (1, 3)]); // encoder IRQ pins
        want.extend([(1, 12), (1, 7), (1, 15), (1, 4), (1, 10), (1, 2)]); // encoder direction pins
        want.extend([(1, 14), (6, 7)]); // trigger input, SYNC LED

        let (board, sdram) = super::Board::split(unsafe { rza1_hal::Peripherals::steal() });
        let pins = &board.pins;
        let mut got: std::vec::Vec<(u8, u8)> = std::vec::Vec::new();
        let mut add = |port: rza1_hal::gpio::Port, bit: u8| got.push((port as u8, bit));
        for p in sdram.address.iter().chain(&sdram.data).chain(&sdram.control) {
            add(p.port(), p.bit());
        }
        add(pins.midi.tx.port(), pins.midi.tx.bit());
        add(pins.midi.rx.port(), pins.midi.rx.bit());
        add(pins.pic.tx.port(), pins.pic.tx.bit());
        add(pins.pic.rx.port(), pins.pic.rx.bit());
        add(pins.spi.sck.port(), pins.spi.sck.bit());
        add(pins.spi.mosi.port(), pins.spi.mosi.bit());
        let sd = &pins.sd;
        for (port, bit) in [
            (sd.cd.port(), sd.cd.bit()),
            (sd.wp.port(), sd.wp.bit()),
            (sd.d1.port(), sd.d1.bit()),
            (sd.d0.port(), sd.d0.bit()),
            (sd.clk.port(), sd.clk.bit()),
            (sd.cmd.port(), sd.cmd.bit()),
            (sd.d3.port(), sd.d3.bit()),
            (sd.d2.port(), sd.d2.bit()),
        ] {
            add(port, bit);
        }
        let a = &pins.audio;
        for (port, bit) in [
            (a.xout.port(), a.xout.bit()),
            (a.sck.port(), a.sck.bit()),
            (a.ws.port(), a.ws.bit()),
            (a.tx.port(), a.tx.bit()),
            (a.rx.port(), a.rx.bit()),
            (a.codec_power.port(), a.codec_power.bit()),
        ] {
            add(port, bit);
        }
        let j = &pins.jacks;
        for (port, bit) in [
            (j.headphone.port(), j.headphone.bit()),
            (j.line_in.port(), j.line_in.bit()),
            (j.mic.port(), j.mic.bit()),
            (j.line_out_l.port(), j.line_out_l.bit()),
            (j.line_out_r.port(), j.line_out_r.bit()),
            (j.speaker_enable.port(), j.speaker_enable.bit()),
        ] {
            add(port, bit);
        }
        let c = &pins.cv_gate;
        for (port, bit) in [
            (c.cs.port(), c.cs.bit()),
            (c.gate0.port(), c.gate0.bit()),
            (c.gate1.port(), c.gate1.bit()),
            (c.gate2.port(), c.gate2.bit()),
            (c.gate3.port(), c.gate3.bit()),
        ] {
            add(port, bit);
        }
        for p in pins.encoders.irq.iter().chain(&pins.encoders.comp) {
            add(p.port(), p.bit());
        }
        add(pins.trigger.port(), pins.trigger.bit());
        add(pins.sync_led.port(), pins.sync_led.bit());

        assert_eq!(got, want);
    }

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

    /// The interrupt names in `src`'s `bind_interrupts!` blocks (`NAME => Handler;`) and
    /// `interrupt::bind::<NAME, …>` calls.
    fn bound_interrupts(src: &str) -> std::vec::Vec<std::string::String> {
        let mut names = std::vec::Vec::new();
        let mut in_block = false;
        let is_ident = |n: &str| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        for line in src.lines() {
            let line = line.trim();
            if line.starts_with("//") {
                continue;
            }
            if line.contains("bind_interrupts!(") {
                in_block = true;
                continue;
            }
            if in_block {
                if line.starts_with('}') {
                    in_block = false;
                } else if let Some((name, _)) = line.split_once("=>") {
                    if is_ident(name.trim()) {
                        names.push(name.trim().into());
                    }
                }
            }
            if let Some(rest) = line.split_once("interrupt::bind::<").map(|(_, r)| r) {
                if let Some((name, _)) = rest.split_once(',') {
                    if is_ident(name.trim()) {
                        names.push(name.trim().into());
                    }
                }
            }
        }
        names
    }

    #[test]
    fn every_interrupt_the_bsp_binds_gets_a_board_priority() {
        // A driver the BSP adds later must not sit at the HAL's default P16 by omission: each
        // interrupt bound in deluge-bsp or the deluge facade is in `apply_irq_priorities`, or is
        // a DMAC completion (its priority is `Config::dma_interrupt_priority`).
        let root = env!("CARGO_MANIFEST_DIR");
        let table = std::fs::read_to_string(std::format!("{root}/src/board.rs")).unwrap();
        let mut missing = std::vec::Vec::new();
        for dir in ["src", "../deluge-sdk/src"] {
            let mut stack = std::vec![std::path::PathBuf::from(std::format!("{root}/{dir}"))];
            while let Some(path) = stack.pop() {
                if path.is_dir() {
                    stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let src = std::fs::read_to_string(&path).unwrap();
                    for name in bound_interrupts(&src) {
                        let covered = name.starts_with("DMAINT")
                            || table.contains(&std::format!("({name}::set_priority,"));
                        if !covered {
                            missing.push(std::format!("{name} ({})", path.display()));
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "bound but given no board priority: {missing:?}");
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
