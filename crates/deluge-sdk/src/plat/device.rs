//! Device backend ops (deluge-bsp peripherals).
use core::convert::Infallible;
use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Poll;

use deluge_bsp::audio_block::{self, BlockState};
use deluge_bsp::encoder;
use deluge_bsp::fat::{self, FatError, Mode, VolumeIdx};
use deluge_bsp::jacks::{self, Jack};
use deluge_bsp::oled::{self, FrameBuffer};
use deluge_bsp::pic;
use deluge_bsp::rgb::PadLeds;
use deluge_bsp::trigger_clock;
use deluge_bsp::uart as bsp_uart;
use embassy_executor::Spawner;
use embassy_time::Instant;
#[cfg(not(feature = "audio-irq"))]
use embassy_time::{Duration, Ticker};
use embedded_hal::digital::{OutputPin, StatefulOutputPin};
use rza1l_hal::gpio::{Output, Pin};

/// Run `f` over every audio block, forever.
pub(crate) async fn audio_run<F: FnMut(&mut [crate::audio::StereoFrame]) + Send + 'static>(
    mut f: F,
) -> ! {
    // Prime the TX ring with dither, then anchor read/write heads.
    audio_block::prime_tx();
    let mut state = BlockState::new();
    let mut block = [crate::audio::StereoFrame::default(); audio_block::BLOCK_FRAMES];

    // Default: Ticker-paced poll loop. The codec crystal is the effective master
    // (try_read_block skips on underrun / re-anchors on overrun), so OSTM vs
    // codec drift costs at most an occasional one-block glitch.
    #[cfg(not(feature = "audio-irq"))]
    {
        let period_us =
            (audio_block::BLOCK_FRAMES as u64 * 1_000_000) / audio_block::SAMPLE_RATE_HZ as u64;
        let mut tick = Ticker::every(Duration::from_micros(period_us));
        loop {
            tick.next().await;
            if !state.try_read_block(&mut block) {
                continue; // not enough input yet — try next tick
            }
            f(&mut block);
            state.write_output_block(&block);
        }
    }

    // `audio-irq`: per-block RX DMA interrupt clock — codec-locked, drift-free.
    #[cfg(feature = "audio-irq")]
    loop {
        audio_block::wait_block().await;
        // Drain every completed block (usually one) so a missed wake can't
        // back the read head up.
        while state.try_read_block(&mut block) {
            f(&mut block);
            state.write_output_block(&block);
        }
    }
}

/// The SYNC LED is wired to port 6, pin 7.
type SyncLedPin = Pin<6, 7, Output>;

/// Bring up RSPI0, which the OLED and the CV DAC share, the first time either needs it, unless
/// the app already did.
fn ensure_shared_spi() {
    if deluge_bsp::bus::is_initialised() {
        return;
    }
    let spi = crate::parts::take("RSPI0 (the OLED and CV bus)", |p| p.shared_spi.take());
    let dma = crate::parts::take("the OLED DMA channel", |p| p.oled_dma.take());
    deluge_bsp::bus::init(spi, dma);
}

pub(crate) async fn oled_init_panel() {
    ensure_shared_spi();
    oled::init().await;
}
pub(crate) async fn oled_flush(fb: &FrameBuffer) {
    oled::send_frame(fb).await;
}

pub(crate) async fn pads_flush(leds: &mut PadLeds) {
    leds.flush().await;
}
pub(crate) async fn pads_set_brightness_interval(interval: u8) {
    deluge_bsp::pic::set_refresh_time(interval).await;
}

pub(crate) async fn leds_set(id: u8, on: bool) {
    if on {
        pic::led_on(id).await;
    } else {
        pic::led_off(id).await;
    }
}
pub(crate) async fn leds_clear() {
    for id in 0..crate::leds::Leds::NUM_INDICATOR_LEDS {
        pic::led_off(id).await;
    }
}
pub(crate) async fn leds_gold_knob(knob: u8, brightness: [u8; 4]) {
    pic::set_gold_knob_indicators(knob, brightness).await;
}

/// PIC UART link speed at power-on (`pic::init` then switches to the fast baud).
const PIC_BAUD: u32 = 31_250;

static PIC_STARTED: AtomicBool = AtomicBool::new(false);

/// Wait for the PIC to finish configuring.
pub(crate) async fn pic_wait_ready() {
    pic::wait_ready().await;
}

/// Ensure the PIC UART is up and the RX pump is running. Idempotent.
pub(crate) fn pic_ensure_started(spawner: Spawner) {
    if PIC_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    // Runs once (guarded above). `init_pic` binds its handlers before it enables their sources,
    // so it is safe with interrupts already enabled.
    let uart = crate::parts::take("SCIF1 (the PIC link)", |p| p.pic_uart.take());
    let rx = crate::parts::take("the PIC receive DMA channel", |p| p.pic_rx_dma.take());
    let tx = crate::parts::take("the PIC transmit DMA channel", |p| p.pic_tx_dma.take());
    bsp_uart::init_pic(uart, rx, tx, PIC_BAUD);
    // `#[embassy_executor::task]` returns a Result in this Embassy version; the
    // only failure is pool exhaustion, impossible for this single spawn.
    spawner.spawn(pic_pump().unwrap());
}

/// PIC RX pump: configure the PIC, then forward decoded events.
///
/// OLED chip-select echoes go to the waiters in
/// [`oled::send_frame`](deluge_bsp::oled::send_frame); pad and button events go
/// to the input queue.
#[embassy_executor::task]
async fn pic_pump() {
    // Configures debounce/refresh, switches to the fast baud, and signals
    // `pic::wait_ready()`.
    pic::init().await;

    let mut parser = pic::Parser::new();
    loop {
        let byte = pic::read_byte().await;
        let Some(event) = parser.push(byte) else {
            continue;
        };
        match event {
            pic::Event::OledSelected => pic::notify_oled_selected(),
            pic::Event::OledDeselected => pic::notify_oled_deselected(),
            // Pads, buttons, etc. go to the input event queue (dropped there if
            // no `input()` consumer is draining it).
            other => crate::input::route_pic_event(other),
        }
    }
}

pub(crate) fn sync_led_init() -> SyncLedPin {
    // SAFETY: the take-once guard in `Deluge::sync_led` ensures this runs once
    // and nothing else owns P6_7; clocks are up by the time an app runs.
    unsafe { SyncLedPin::into_output() }
}
pub(crate) fn sync_led_set_high(pin: &mut SyncLedPin) -> Result<(), Infallible> {
    pin.set_high()
}
pub(crate) fn sync_led_set_low(pin: &mut SyncLedPin) -> Result<(), Infallible> {
    pin.set_low()
}
pub(crate) fn sync_led_toggle(pin: &mut SyncLedPin) -> Result<(), Infallible> {
    StatefulOutputPin::toggle(pin)
}
pub(crate) fn sync_led_is_set_high(pin: &mut SyncLedPin) -> Result<bool, Infallible> {
    pin.is_set_high()
}
pub(crate) fn sync_led_is_set_low(pin: &mut SyncLedPin) -> Result<bool, Infallible> {
    pin.is_set_low()
}

pub(crate) fn cv_gate_init() {
    ensure_shared_spi();
    // SAFETY: runs once, after the bus is up. Configures GPIO and runs the DAC's ~10 ms
    // linearity init (poll-based delays). Acquire CV/gate before entering a
    // loop that also drives the OLED, so this one-time RSPI0 setup can't race an
    // in-flight OLED transfer (see docs/advanced-guide.md §7).
    unsafe { deluge_bsp::cv_gate::init() };
}
pub(crate) async fn cv_set(ch: u8, code: u16) {
    deluge_bsp::cv_gate::cv_set(ch, code).await;
}
pub(crate) fn gate_set(ch: u8, on: bool) {
    // SAFETY: GPIO write to a gate line we own; pins configured by init.
    unsafe { deluge_bsp::cv_gate::gate_set(ch, on) };
}

/// MIDI DIN baud rate.
const MIDI_BAUD: u32 = 31_250;

pub(crate) fn midi_init() {
    // Runs once. Binds SCIF0's handlers before it enables their sources, so it is safe at runtime.
    let uart = crate::parts::take("SCIF0 (DIN MIDI)", |p| p.midi_uart.take());
    let rx = crate::parts::take("the MIDI receive DMA channel", |p| p.midi_rx_dma.take());
    deluge_bsp::uart::init_midi(uart, rx, MIDI_BAUD);
}
pub(crate) async fn midi_send(data: &[u8]) {
    deluge_bsp::uart::write_midi(data).await;
}
pub(crate) async fn midi_recv() -> u8 {
    deluge_bsp::uart::read_midi_byte().await
}
pub(crate) fn midi_try_recv() -> Option<u8> {
    deluge_bsp::uart::try_read_midi()
}

pub(crate) fn clock_in_init() {
    // SAFETY: runs once. Registers the P1_14/IRQ6 handler and enables the
    // GIC line. Registered lazily, after interrupts are enabled, as the
    // encoder IRQs are in `input_start_pump`.
    unsafe { trigger_clock::irq_init() };
}
pub(crate) async fn clock_in_wait_edge() -> u64 {
    let start = trigger_clock::EDGE_COUNT.load(Ordering::Relaxed);
    poll_fn(|cx| {
        trigger_clock::EDGE_WAKER.register(cx.waker());
        if trigger_clock::EDGE_COUNT.load(Ordering::Relaxed) != start {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    trigger_clock::LAST_EDGE_TICKS.load(Ordering::Relaxed)
}
pub(crate) fn clock_in_count() -> u32 {
    trigger_clock::EDGE_COUNT.load(Ordering::Relaxed)
}
pub(crate) fn clock_in_last_edge() -> Option<Instant> {
    match trigger_clock::LAST_EDGE_TICKS.load(Ordering::Relaxed) {
        0 => None,
        t => Some(Instant::from_ticks(t)),
    }
}

pub(crate) fn jacks_init() {
    // SAFETY: runs once. Configures the five jack-detect inputs and the
    // speaker-enable output (left disabled).
    unsafe { jacks::init() };
}
pub(crate) fn jacks_headphone() -> bool {
    jacks::is_inserted(Jack::Headphone)
}
pub(crate) fn jacks_line_in() -> bool {
    jacks::is_inserted(Jack::LineIn)
}
pub(crate) fn jacks_mic() -> bool {
    jacks::is_inserted(Jack::Mic)
}
pub(crate) fn jacks_line_out_left() -> bool {
    jacks::is_inserted(Jack::LineOutL)
}
pub(crate) fn jacks_line_out_right() -> bool {
    jacks::is_inserted(Jack::LineOutR)
}
pub(crate) fn jacks_set_speaker(on: bool) {
    // SAFETY: GPIO write to the speaker-enable output configured by init.
    unsafe { jacks::set_speaker_enable(on) };
}

pub(crate) async fn sd_init_card() -> Result<(), deluge_bsp::sd::SdError> {
    // The slot's hardware comes up once; the card protocol runs on every call.
    static HW_UP: AtomicBool = AtomicBool::new(false);
    if !HW_UP.swap(true, Ordering::Relaxed) {
        let card = crate::parts::take("SDHI1 (the SD card slot)", |p| p.sd_card.take());
        let tx = crate::parts::take("the SD transmit DMA channel", |p| p.sd_tx_dma.take());
        let rx = crate::parts::take("the SD receive DMA channel", |p| p.sd_rx_dma.take());
        deluge_bsp::sd::init_hw(card, tx, rx);
    }
    deluge_bsp::sd::init().await
}

/// Read a root-directory file into `buf`; returns the number of bytes read
/// (capped at `buf.len()`).
pub(crate) fn sd_read(name: &str, buf: &mut [u8]) -> Result<usize, FatError> {
    let vm = fat::new_volume_manager();
    let volume = vm.open_raw_volume(VolumeIdx(0))?;
    let root = vm.open_root_dir(volume)?;
    let file = vm.open_file_in_dir(root, name, Mode::ReadOnly)?;

    let mut total = 0;
    while total < buf.len() {
        let n = vm.read(file, &mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }

    vm.close_file(file)?;
    // Dropping `vm` releases the volume/dir handles.
    Ok(total)
}

/// Write `data` to a root-directory file, creating or truncating it.
pub(crate) fn sd_write(name: &str, data: &[u8]) -> Result<(), FatError> {
    let vm = fat::new_volume_manager();
    let volume = vm.open_raw_volume(VolumeIdx(0))?;
    let root = vm.open_root_dir(volume)?;
    let file = vm.open_file_in_dir(root, name, Mode::ReadWriteCreateOrTruncate)?;

    vm.write(file, data)?;
    // close_file flushes; must happen before `vm` is dropped.
    vm.close_file(file)?;
    Ok(())
}

static INPUT_PUMP_STARTED: AtomicBool = AtomicBool::new(false);

/// Configure the encoder GPIO interrupts and spawn the encoder pump. Idempotent.
///
/// Pads/buttons additionally require the PIC service; [`Deluge::input`] starts
/// that too.
pub(crate) fn input_start_pump(spawner: Spawner) {
    if INPUT_PUMP_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    // SAFETY: runs once (guarded above). `irq_init` registers each encoder's GIC
    // handler before enabling its source, so it is safe with interrupts enabled.
    let poll_timer =
        crate::parts::take("MTU2 channel 3 (the encoder poll)", |p| p.encoder_poll_timer.take());
    unsafe { encoder::irq_init(poll_timer) };
    spawner.spawn(encoder_pump().unwrap());
}

/// Wake on encoder IRQ, drain detent deltas, and enqueue [`crate::input::Event::Encoder`].
#[embassy_executor::task]
async fn encoder_pump() {
    let mut acc = [0i8; encoder::NUM_ENCODERS];
    loop {
        // Sleep until an ISR records a non-zero delta on some encoder.
        poll_fn(|cx| {
            encoder::ENCODER_WAKER.register(cx.waker());
            if encoder::ENCODER_DELTAS
                .iter()
                .any(|d| d.load(Ordering::Relaxed) != 0)
            {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;

        for (i, acc_i) in acc.iter_mut().enumerate() {
            let delta = encoder::take_detents(i, acc_i);
            if delta != 0 {
                let _ = crate::input::EVENTS.try_send(crate::input::Event::Encoder {
                    index: i as u8,
                    delta,
                });
            }
        }
    }
}
