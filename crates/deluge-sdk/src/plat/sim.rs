//! Host-simulator backend ops (the `deluge_simulator::link::SharedPanel`).
use core::sync::atomic::{AtomicBool, Ordering};

use deluge_bsp::oled::FrameBuffer;
use deluge_bsp::rgb::{COLS, PadLeds, ROWS};
use embassy_executor::Spawner;
use embassy_time::Instant;

/// Host: exchange audio blocks with the simulator through [`super::sim_panel`].
///
/// The simulator's audio device plays at the device rate, so this loop is paced
/// by real time without a hardware clock: it renders while the output has room,
/// and waits half a block when it has none.
pub(crate) async fn audio_run<F: FnMut(&mut [crate::audio::StereoFrame]) + Send + 'static>(
    mut f: F,
) -> ! {
    use super::sim_panel::{BLOCK_FRAMES, SAMPLE_RATE_HZ};
    use embassy_time::{Duration, Timer};

    let mut block = [crate::audio::StereoFrame::default(); BLOCK_FRAMES];
    let period_us = (BLOCK_FRAMES as u64 * 1_000_000) / SAMPLE_RATE_HZ as u64;
    let wait = Duration::from_micros(period_us / 2);

    loop {
        let rendered = super::sim_panel::render_block(|frames| {
            for (fr, s) in block.iter_mut().zip(frames.iter()) {
                fr.l = s[0];
                fr.r = s[1];
            }
            f(&mut block);
            for (s, fr) in frames.iter_mut().zip(block.iter()) {
                *s = [fr.l, fr.r];
            }
        });
        if !rendered {
            Timer::after(wait).await;
        }
    }
}

pub(crate) async fn oled_init_panel() {}
pub(crate) async fn oled_flush(fb: &FrameBuffer) {
    crate::host::panel().set_display(fb.as_bytes());
}

pub(crate) async fn pads_flush(leds: &mut PadLeds) {
    let grid = leds.grid();
    let mut buf = [0u8; deluge_simulator::link::ALL_PADS_BYTES];
    for col in 0..COLS {
        for row in 0..ROWS {
            let o = (col * ROWS + row) * 3;
            buf[o..o + 3].copy_from_slice(&grid[col][row]);
        }
    }
    crate::host::panel().set_all_pads(&buf);
}
pub(crate) async fn pads_set_brightness_interval(interval: u8) {
    let _ = interval;
}

pub(crate) async fn leds_set(id: u8, on: bool) {
    crate::host::panel().set_led(id as usize, on);
}
pub(crate) async fn leds_clear() {
    crate::host::panel().clear_all_leds();
}
pub(crate) async fn leds_gold_knob(knob: u8, brightness: [u8; 4]) {
    crate::host::panel().set_knob_indicator(knob as usize, brightness);
}

static PIC_STARTED: AtomicBool = AtomicBool::new(false);

/// Host: nothing to wait for.
pub(crate) async fn pic_wait_ready() {}

/// Host: there is no PIC co-processor to bring up.
pub(crate) fn pic_ensure_started(_spawner: Spawner) {
    let _ = PIC_STARTED.swap(true, Ordering::Relaxed);
}

pub(crate) fn sync_led_init() -> bool {
    false
}
pub(crate) fn sync_led_set(on: bool) {
    crate::host::panel().set_synced_led(on);
}
pub(crate) fn sync_led_is_set_high(state: bool) -> bool {
    state
}
pub(crate) fn sync_led_is_set_low(state: bool) -> bool {
    !state
}

pub(crate) fn cv_gate_init() {}
pub(crate) async fn cv_set(ch: u8, code: u16) {
    crate::host::panel().set_cv(ch as usize, code);
}
pub(crate) fn gate_set(ch: u8, on: bool) {
    crate::host::panel().set_gate(ch as usize, on);
}

/// The clock out: a task times it with `Timer`s (see `clock::soft`).
pub(crate) use crate::clock::soft::{
    clock_out_init, clock_out_pulse, clock_out_retime, clock_out_running, clock_out_start,
    clock_out_stop,
};

pub(crate) fn midi_init() {}
pub(crate) async fn midi_send(data: &[u8]) {
    // Hand the bytes to the simulator panel (lights the MIDI OUT activity
    // indicator; the GUI can forward them to a host port).
    crate::host::panel().push_midi_out(data);
}
pub(crate) async fn midi_recv() -> u8 {
    // Drain bytes the simulator's MIDI bridge pushed into the panel, polling
    // at ~1 ms when the queue is empty (DIN MIDI is slow, so the latency is
    // inaudible).
    loop {
        if let Some(b) = crate::host::panel().pop_midi_in() {
            return b;
        }
        embassy_time::Timer::after_millis(1).await;
    }
}
pub(crate) fn midi_try_recv() -> Option<u8> {
    crate::host::panel().pop_midi_in()
}

pub(crate) fn clock_in_init() {}
pub(crate) async fn clock_in_wait_edge() -> u64 {
    core::future::pending::<u64>().await
}
pub(crate) fn clock_in_count() -> u32 {
    0
}
pub(crate) fn clock_in_last_edge() -> Option<Instant> {
    None
}

pub(crate) fn jacks_init() {}
pub(crate) fn jacks_headphone() -> bool {
    false
}
pub(crate) fn jacks_line_in() -> bool {
    false
}
pub(crate) fn jacks_mic() -> bool {
    false
}
pub(crate) fn jacks_line_out_left() -> bool {
    false
}
pub(crate) fn jacks_line_out_right() -> bool {
    false
}
pub(crate) fn jacks_set_speaker(on: bool) {
    let _ = on;
}

/// The simulated card: a host directory (`DELUGE_SIM_SD`, default `./sim-sd`), always present.
pub(crate) struct SdCard;

/// Host: the simulated card is always available.
pub(crate) async fn sd_mount() -> Result<SdCard, crate::sd::SdError> {
    Ok(SdCard)
}

/// The simulated SD-card root directory (`DELUGE_SIM_SD`, default `./sim-sd`).
fn sim_sd_root() -> std::path::PathBuf {
    std::env::var_os("DELUGE_SIM_SD")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("sim-sd"))
}

impl From<std::io::Error> for crate::sd::FatError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => crate::sd::FatError::NotFound,
            _ => crate::sd::FatError::Card,
        }
    }
}

impl SdCard {
    /// Read a root-directory file into `buf`; returns the number of bytes read.
    pub(crate) async fn read(
        &mut self,
        name: &str,
        buf: &mut [u8],
    ) -> Result<usize, crate::sd::FatError> {
        let mut file = self.open(name).await?;
        file.read(buf).await
    }

    /// Write `data` to a root-directory file, creating or truncating it.
    pub(crate) async fn write(
        &mut self,
        name: &str,
        data: &[u8],
    ) -> Result<(), crate::sd::FatError> {
        let root = sim_sd_root();
        std::fs::create_dir_all(&root)?;
        let path = root.join(name);
        // A directory's name isn't a file's, as on the card.
        if path.is_dir() {
            return Err(crate::sd::FatError::InvalidName);
        }
        std::fs::write(path, data)?;
        Ok(())
    }

    /// Open a root-directory file to read.
    pub(crate) async fn open(&mut self, name: &str) -> Result<SdFile<'_>, crate::sd::FatError> {
        let file = std::fs::File::open(sim_sd_root().join(name))?;
        let meta = file.metadata()?;
        // A directory isn't a file, as on the card.
        if !meta.is_file() {
            return Err(crate::sd::FatError::NotFound);
        }
        let len = u32::try_from(meta.len()).unwrap_or(u32::MAX);
        Ok(SdFile {
            file,
            len,
            _card: core::marker::PhantomData,
        })
    }
}

/// An open simulated-card file.
pub(crate) struct SdFile<'a> {
    file: std::fs::File,
    len: u32,
    _card: core::marker::PhantomData<&'a mut SdCard>,
}

impl SdFile<'_> {
    pub(crate) fn len(&self) -> u32 {
        self.len
    }

    /// Fill `buf` unless the file ends first: the bytes read.
    pub(crate) async fn read(&mut self, buf: &mut [u8]) -> Result<usize, crate::sd::FatError> {
        use std::io::Read;
        let mut n = 0;
        while n < buf.len() {
            match self.file.read(&mut buf[n..])? {
                0 => break,
                got => n += got,
            }
        }
        Ok(n)
    }

    pub(crate) async fn seek(&mut self, offset: u32) -> Result<(), crate::sd::FatError> {
        use std::io::Seek;
        if offset > self.len {
            return Err(crate::sd::FatError::SeekPastEnd);
        }
        self.file.seek(std::io::SeekFrom::Start(offset.into()))?;
        Ok(())
    }

    pub(crate) async fn close(self) -> Result<(), crate::sd::FatError> {
        Ok(())
    }
}

static INPUT_PUMP_STARTED: AtomicBool = AtomicBool::new(false);

/// Host: start the pump that forwards GUI input from the shared panel into the
/// SDK event queue. Called once by the host runtime before the app runs.
pub(crate) fn input_start_pump(spawner: Spawner) {
    if INPUT_PUMP_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    spawner.spawn(host_input_pump().unwrap());
}

/// Host: poll the shared panel for GUI input and enqueue it as
/// [`crate::input::Event`]s.
#[embassy_executor::task]
async fn host_input_pump() {
    use deluge_simulator::link::InputEvent;
    use embassy_time::{Duration, Timer};
    loop {
        while let Some(ev) = crate::host::panel().pop_event() {
            let mapped = match ev {
                InputEvent::Pad { x, y, pressed } => crate::input::Event::Pad { x, y, pressed },
                InputEvent::Button { id, pressed } => crate::input::Event::Button { id, pressed },
                InputEvent::Encoder { index, delta } => {
                    crate::input::Event::Encoder { index, delta }
                }
            };
            let _ = crate::input::EVENTS.try_send(mapped);
        }
        // Poll cadence: low enough latency to feel instant, cheap on the host.
        Timer::after(Duration::from_millis(1)).await;
    }
}
