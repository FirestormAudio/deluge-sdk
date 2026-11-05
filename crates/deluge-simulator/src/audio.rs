//! Host audio for the in-process link: bridge the app's audio rings to the
//! machine's speakers/mic via `cpal`.
//!
//! The output stream drains app→GUI frames (silence on underrun); the input
//! stream fills GUI→app frames (silence if there's no input device). The output
//! callback is effectively the audio clock that paces the app's DSP loop.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::link::audio::{BLOCK_FRAMES, GuiEnds, SAMPLE_RATE_HZ};
use cpal::Sample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

/// Optional WAV file bridges, from `--audio-in` / `--audio-out` (via the
/// `DELUGE_SIM_AUDIO_IN` / `DELUGE_SIM_AUDIO_OUT` env vars).
#[derive(Default)]
pub struct AudioConfig {
    /// Feed this WAV as the codec input instead of the mic (looped).
    pub input_wav: Option<PathBuf>,
    /// Record the codec output (pre master-volume) to this WAV.
    pub output_wav: Option<PathBuf>,
}

impl AudioConfig {
    /// Build the config from the `DELUGE_SIM_AUDIO_IN` / `_OUT` env vars.
    pub fn from_env() -> Self {
        Self {
            input_wav: std::env::var_os("DELUGE_SIM_AUDIO_IN").map(PathBuf::from),
            output_wav: std::env::var_os("DELUGE_SIM_AUDIO_OUT").map(PathBuf::from),
        }
    }
}

/// The simulator's master output volume (0.0–1.0), stored as `f32` bits so the
/// audio callback can read it lock-free. Driven by the faceplate Volume knob.
pub type Volume = Arc<AtomicU32>;

/// Read a [`Volume`] as a gain in 0.0..1.0.
pub fn volume_gain(v: &Volume) -> f32 {
    f32::from_bits(v.load(Ordering::Relaxed))
}

/// A fresh master volume at full scale (1.0).
pub fn new_volume() -> Volume {
    Arc::new(AtomicU32::new(1.0f32.to_bits()))
}

/// Monitor gain in dB on top of the Volume knob, stored as `f32` bits like [`Volume`]. The Deluge leaves a single
/// voice well below full scale so a whole mix fits, and on the device its analogue output stage adds the gain a
/// listener hears; this stands in for it. Speakers only: the recording and the scopes take the output before it.
pub type Boost = Arc<AtomicU32>;

/// The most [`Boost`] can add.
pub const MAX_BOOST_DB: f32 = 24.0;

/// A fresh boost of 0 dB.
pub fn new_boost() -> Boost {
    Arc::new(AtomicU32::new(0.0f32.to_bits()))
}

/// A [`Boost`]'s dB setting.
pub fn boost_db(b: &Boost) -> f32 {
    f32::from_bits(b.load(Ordering::Relaxed))
}

/// Where [`soft_clip`] starts to bend.
const KNEE: f32 = 0.9;

/// Linear below [`KNEE`], then bending smoothly towards ±1, so a boosted mix that passes full scale rounds off rather
/// than wrapping or cracking.
fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let bent = KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh();
    bent.copysign(x)
}

/// Depth (stereo frames) of the scope-monitor ring. A few output callbacks'
/// worth of headroom so the GUI can drain it once per frame without overflow.
const MONITOR_CAP: usize = 8192;

/// Keeps the cpal streams + file-bridge threads alive for the simulator's
/// lifetime (dropping a `cpal::Stream` stops it; dropping the recorder finalises
/// the WAV).
pub struct AudioStreams {
    _output: Option<cpal::Stream>,
    _input: Option<cpal::Stream>,
    _feeder: Option<Worker>,
    _recorder: Option<Worker>,
}

/// Open the default output (and, best-effort, input) device and wire them to the
/// app's audio rings. Failures are logged and degrade to silence so the app
/// still runs.
///
/// Returns the live streams plus a **scope-monitor** consumer: a stereo copy of
/// the output going to the speakers, which the GUI drains into the rack's audio
/// oscilloscopes (one per channel).
pub fn start(
    gui: GuiEnds,
    volume: Volume,
    boost: Boost,
    cfg: AudioConfig,
) -> (AudioStreams, HeapCons<[f32; 2]>) {
    let GuiEnds { out, in_ } = gui;
    let (out, in_) = (out.0, in_.0);
    let host = cpal::default_host();

    let (mon_prod, mon_cons) = HeapRb::<[f32; 2]>::new(MONITOR_CAP).split();

    // --audio-out: tap the output (pre-volume) into a WAV recorder.
    let (rec_prod, recorder) = match &cfg.output_wav {
        Some(path) => match Recorder::start(path) {
            Ok((prod, worker)) => (Some(prod), Some(worker)),
            Err(e) => {
                log::warn!("simulator audio: --audio-out failed ({e})");
                (None, None)
            }
        },
        None => (None, None),
    };

    let output = match start_output(&host, out, mon_prod, volume, boost, rec_prod) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("simulator audio: no output ({e}); running silent");
            None
        }
    };

    // --audio-in: feed a WAV as the codec input instead of opening the mic.
    let (input, feeder) = match &cfg.input_wav {
        Some(path) => match Feeder::start(path, in_) {
            Ok(worker) => (None, Some(worker)),
            Err(e) => {
                log::warn!("simulator audio: --audio-in failed ({e})");
                (None, None)
            }
        },
        None => match start_input(&host, in_) {
            Ok(s) => (Some(s), None),
            Err(e) => {
                log::info!("simulator audio: no input ({e}); mic is silent");
                (None, None)
            }
        },
    };

    (
        AudioStreams {
            _output: output,
            _input: input,
            _feeder: feeder,
            _recorder: recorder,
        },
        mon_cons,
    )
}

/// A background file-bridge thread (WAV feeder or recorder) with cooperative
/// shutdown. Dropping it signals the thread and joins, so a recorder finalises
/// its WAV before the process exits.
struct Worker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Ring depth (stereo frames) for the recorder tap — generous so file-write
/// hiccups never stall the audio callback.
const RECORD_CAP: usize = BLOCK_FRAMES * 64;

struct Recorder;

impl Recorder {
    /// Open `path` for writing and spawn a thread draining the returned ring to
    /// it. Returns the producer (for the output callback) and the worker handle.
    fn start(path: &Path) -> Result<(HeapProd<[f32; 2]>, Worker), String> {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: SAMPLE_RATE_HZ,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer =
            hound::WavWriter::create(path, spec).map_err(|e| format!("create {path:?}: {e}"))?;
        let (prod, mut cons) = HeapRb::<[f32; 2]>::new(RECORD_CAP).split();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let path = path.to_path_buf();
        let handle = std::thread::Builder::new()
            .name("deluge-audio-rec".into())
            .spawn(move || {
                let mut drain = |cons: &mut HeapCons<[f32; 2]>| {
                    while let Some([l, r]) = cons.try_pop() {
                        let _ = writer.write_sample(l);
                        let _ = writer.write_sample(r);
                    }
                };
                while !stop2.load(Ordering::Acquire) {
                    drain(&mut cons);
                    std::thread::sleep(Duration::from_millis(5));
                }
                drain(&mut cons); // flush anything queued at shutdown
                match writer.finalize() {
                    Ok(()) => log::info!("simulator audio: recorded output to {path:?}"),
                    Err(e) => log::warn!("simulator audio: finalising {path:?} failed ({e})"),
                }
            })
            .map_err(|e| e.to_string())?;
        Ok((
            prod,
            Worker {
                stop,
                handle: Some(handle),
            },
        ))
    }
}

struct Feeder;

impl Feeder {
    /// Load `path` into memory and spawn a thread feeding it (looped) into the
    /// codec-input ring `in_`, paced by the app's consumption.
    fn start(path: &Path, mut in_: HeapProd<[f32; 2]>) -> Result<Worker, String> {
        let frames = load_wav(path)?;
        if frames.is_empty() {
            return Err("empty WAV".into());
        }
        log::info!(
            "simulator audio: feeding input from {path:?} ({} frames, looped)",
            frames.len()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::Builder::new()
            .name("deluge-audio-feed".into())
            .spawn(move || {
                'outer: loop {
                    for &f in &frames {
                        // Back off while the ring is full; the app drains it as it
                        // processes blocks, so playback runs at the codec rate.
                        while in_.is_full() {
                            if stop2.load(Ordering::Acquire) {
                                break 'outer;
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        if stop2.load(Ordering::Acquire) {
                            break 'outer;
                        }
                        let _ = in_.try_push(f);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Worker {
            stop,
            handle: Some(handle),
        })
    }
}

/// Read a WAV fully into stereo `f32` frames, converting from int/float and
/// mono/stereo. Warns (but proceeds) if the sample rate isn't the codec rate.
pub(crate) fn load_wav(path: &Path) -> Result<Vec<[f32; 2]>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("open {path:?}: {e}"))?;
    let spec = reader.spec();
    if spec.sample_rate != SAMPLE_RATE_HZ {
        log::warn!(
            "simulator audio: {path:?} is {} Hz, not {SAMPLE_RATE_HZ} Hz; playing without resampling",
            spec.sample_rate
        );
    }
    let ch = spec.channels.max(1) as usize;
    // Flatten to interleaved f32 samples regardless of source format.
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.unwrap_or(0) as f32 / max)
                .collect()
        }
    };
    Ok(samples
        .chunks(ch)
        .map(|c| match c {
            [l, r, ..] => [*l, *r],
            [m] => [*m, *m],
            _ => [0.0, 0.0],
        })
        .collect())
}

/// What [`choose_config`] needs to know about one of a device's supported configurations.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SupportedRange {
    channels: u16,
    min_hz: u32,
    max_hz: u32,
    format: cpal::SampleFormat,
}

impl From<&cpal::SupportedStreamConfigRange> for SupportedRange {
    fn from(r: &cpal::SupportedStreamConfigRange) -> Self {
        Self {
            channels: r.channels(),
            min_hz: r.min_sample_rate(),
            max_hz: r.max_sample_rate(),
            format: r.sample_format(),
        }
    }
}

/// How to open a device for the simulator's rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    /// The supported range at this index runs at the rate: open it there.
    Native(usize),
    /// No range offers the rate: open the device's default and resample.
    Resample,
}

/// Pick a configuration running at `want_hz` in a sample format the stream builders drive,
/// preferring stereo, then `f32`.
fn choose_config(supported: &[SupportedRange], want_hz: u32) -> Choice {
    let format_rank = |f| match f {
        cpal::SampleFormat::F32 => Some(0),
        cpal::SampleFormat::I16 => Some(1),
        cpal::SampleFormat::U16 => Some(2),
        _ => None,
    };
    supported
        .iter()
        .enumerate()
        .filter(|(_, r)| (r.min_hz..=r.max_hz).contains(&want_hz))
        .filter_map(|(i, r)| Some((i, (r.channels < 2, format_rank(r.format)?))))
        .min_by_key(|&(_, score)| score)
        .map_or(Choice::Resample, |(i, _)| Choice::Native(i))
}

/// A linear-interpolating sample-rate converter for one stereo stream, for a device that won't
/// run at the simulator's rate. Linear interpolation is enough to hear an app at the right pitch
/// and speed; it is not a mastering-grade converter.
struct LinearResampler {
    /// Source frames per output frame.
    step: f64,
    /// Where the next output frame falls between `prev` and `cur`, in source frames.
    pos: f64,
    prev: [f32; 2],
    cur: [f32; 2],
}

impl LinearResampler {
    fn new(from_hz: u32, to_hz: u32) -> Self {
        Self {
            step: from_hz as f64 / to_hz as f64,
            pos: 0.0,
            prev: [0.0; 2],
            cur: [0.0; 2],
        }
    }

    fn lerp(&self) -> [f32; 2] {
        let t = self.pos as f32;
        [0, 1].map(|c| self.prev[c] + (self.cur[c] - self.prev[c]) * t)
    }

    /// The next output frame, pulling source frames from `src` as it needs them (the output
    /// direction: the device callback asks for a fixed number of frames).
    fn pull(&mut self, mut src: impl FnMut() -> [f32; 2]) -> [f32; 2] {
        while self.pos >= 1.0 {
            self.prev = self.cur;
            self.cur = src();
            self.pos -= 1.0;
        }
        let out = self.lerp();
        self.pos += self.step;
        out
    }

    /// Feed one source frame, emitting the output frames that fall before it (the input
    /// direction: the device callback hands over what it captured).
    fn push(&mut self, frame: [f32; 2], mut emit: impl FnMut([f32; 2])) {
        self.prev = self.cur;
        self.cur = frame;
        while self.pos < 1.0 {
            emit(self.lerp());
            self.pos += self.step;
        }
        self.pos -= 1.0;
    }
}

/// Open `dev` at the simulator's rate if it offers it, otherwise at its default with a
/// resampler (logging the rates once).
fn open_config(
    dir: &str,
    supported: Result<Vec<cpal::SupportedStreamConfigRange>, cpal::Error>,
    default: impl FnOnce() -> Result<cpal::SupportedStreamConfig, cpal::Error>,
) -> Result<(cpal::SupportedStreamConfig, Option<u32>), String> {
    let ranges = supported.unwrap_or_default();
    let summary: Vec<SupportedRange> = ranges.iter().map(SupportedRange::from).collect();
    if let Choice::Native(i) = choose_config(&summary, SAMPLE_RATE_HZ) {
        return Ok((ranges[i].with_sample_rate(SAMPLE_RATE_HZ), None));
    }
    let cfg = default().map_err(|e| format!("{dir} config: {e}"))?;
    log::warn!(
        "simulator audio: the {dir} device runs at {} Hz, not {SAMPLE_RATE_HZ} Hz; resampling",
        cfg.sample_rate()
    );
    let rate = cfg.sample_rate();
    Ok((cfg, Some(rate)))
}

fn start_output(
    host: &cpal::Host,
    out: HeapCons<[f32; 2]>,
    mon: HeapProd<[f32; 2]>,
    volume: Volume,
    boost: Boost,
    rec: Option<HeapProd<[f32; 2]>>,
) -> Result<cpal::Stream, String> {
    let dev = host
        .default_output_device()
        .ok_or("no default output device")?;
    let (cfg, device_hz) = open_config(
        "output",
        dev.supported_output_configs().map(Iterator::collect),
        || dev.default_output_config(),
    )?;
    let rs = device_hz.map(|hz| LinearResampler::new(SAMPLE_RATE_HZ, hz));
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let stream = match fmt {
        cpal::SampleFormat::F32 => {
            build_output::<f32>(&dev, &config, out, mon, volume, boost, rec, rs)
        }
        cpal::SampleFormat::I16 => {
            build_output::<i16>(&dev, &config, out, mon, volume, boost, rec, rs)
        }
        cpal::SampleFormat::U16 => {
            build_output::<u16>(&dev, &config, out, mon, volume, boost, rec, rs)
        }
        other => return Err(format!("unsupported output sample format {other:?}")),
    }
    .map_err(|e| format!("build output stream: {e}"))?;
    stream.play().map_err(|e| format!("play output: {e}"))?;
    Ok(stream)
}

fn build_output<T>(
    dev: &cpal::Device,
    config: &cpal::StreamConfig,
    mut out: HeapCons<[f32; 2]>,
    mut mon: HeapProd<[f32; 2]>,
    volume: Volume,
    boost: Boost,
    mut rec: Option<HeapProd<[f32; 2]>>,
    mut rs: Option<LinearResampler>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    dev.build_output_stream(
        config.clone(),
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let boost_db = boost_db(&boost);
            let gain = volume_gain(&volume) * 10f32.powf(boost_db / 20.0);
            // At 0 dB the speakers get the app's output exactly, as the device's DAC does.
            let shape = |v: f32| if boost_db > 0.0 { soft_clip(v) } else { v };
            // The app's next frame. Tee it, *pre-volume* and at the app's rate, to the scope
            // monitor (and the recorder, if any), so both capture the app's true output
            // regardless of the monitor knob.
            let mut next = || {
                let s = out.try_pop().unwrap_or([0.0, 0.0]);
                let _ = mon.try_push(s);
                if let Some(rec) = rec.as_mut() {
                    let _ = rec.try_push(s);
                }
                s
            };
            for frame in data.chunks_mut(channels.max(1)) {
                let s = match rs.as_mut() {
                    Some(rs) => rs.pull(&mut next),
                    None => next(),
                };
                for (i, ch) in frame.iter_mut().enumerate() {
                    let v = match i {
                        0 => s[0],
                        1 => s[1],
                        _ => 0.0,
                    };
                    *ch = T::from_sample(shape(v * gain));
                }
            }
        },
        |e| log::warn!("simulator audio output error: {e}"),
        None,
    )
}

fn start_input(host: &cpal::Host, in_: HeapProd<[f32; 2]>) -> Result<cpal::Stream, String> {
    let dev = host
        .default_input_device()
        .ok_or("no default input device")?;
    let (cfg, device_hz) = open_config(
        "input",
        dev.supported_input_configs().map(Iterator::collect),
        || dev.default_input_config(),
    )?;
    let rs = device_hz.map(|hz| LinearResampler::new(hz, SAMPLE_RATE_HZ));
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let stream = match fmt {
        cpal::SampleFormat::F32 => build_input::<f32>(&dev, &config, in_, rs),
        cpal::SampleFormat::I16 => build_input::<i16>(&dev, &config, in_, rs),
        cpal::SampleFormat::U16 => build_input::<u16>(&dev, &config, in_, rs),
        other => return Err(format!("unsupported input sample format {other:?}")),
    }
    .map_err(|e| format!("build input stream: {e}"))?;
    stream.play().map_err(|e| format!("play input: {e}"))?;
    Ok(stream)
}

fn build_input<T>(
    dev: &cpal::Device,
    config: &cpal::StreamConfig,
    mut in_: HeapProd<[f32; 2]>,
    mut rs: Option<LinearResampler>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels.max(1) as usize;
    dev.build_input_stream(
        config.clone(),
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            for frame in data.chunks(channels) {
                let l = f32::from_sample(frame[0]);
                let r = if channels > 1 {
                    f32::from_sample(frame[1])
                } else {
                    l
                };
                match rs.as_mut() {
                    Some(rs) => rs.push([l, r], |s| {
                        let _ = in_.try_push(s);
                    }),
                    None => {
                        let _ = in_.try_push([l, r]);
                    }
                }
            }
        },
        |e| log::warn!("simulator audio input error: {e}"),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 kHz stereo tone, `secs` long at `rate`.
    fn tone(rate: u32, secs: f64) -> Vec<[f32; 2]> {
        (0..(rate as f64 * secs) as usize)
            .map(|n| {
                let v =
                    (std::f64::consts::TAU * 1000.0 * n as f64 / rate as f64 + 0.3).sin() as f32;
                [v, v]
            })
            .collect()
    }

    /// The left channel's frequency, from its rising zero crossings.
    fn frequency(frames: &[[f32; 2]], rate: u32) -> f64 {
        let rising = frames
            .windows(2)
            .filter(|w| w[0][0] < 0.0 && w[1][0] >= 0.0)
            .count();
        rising as f64 * rate as f64 / frames.len() as f64
    }

    #[test]
    fn resample_48k_to_44k1_preserves_a_1khz_tone_frequency() {
        let mut rs = LinearResampler::new(48_000, 44_100);
        let mut out = Vec::new();
        for frame in tone(48_000, 1.0) {
            rs.push(frame, |s| out.push(s));
        }
        assert!(out.len().abs_diff(44_100) <= 1, "{} frames", out.len());
        let hz = frequency(&out, 44_100);
        assert!((hz - 1000.0).abs() <= 1.0, "{hz} Hz");
    }

    #[test]
    fn resample_44k1_to_48k_for_the_output_device_preserves_a_1khz_tone_frequency() {
        let mut src = tone(44_100, 1.0).into_iter();
        let mut rs = LinearResampler::new(44_100, 48_000);
        let out: Vec<_> = (0..48_000)
            .map(|_| rs.pull(|| src.next().unwrap_or([0.0; 2])))
            .collect();
        assert!(src.len() <= 1, "{} source frames left over", src.len());
        let hz = frequency(&out, 48_000);
        assert!((hz - 1000.0).abs() <= 1.0, "{hz} Hz");
    }

    #[test]
    fn choose_config_takes_a_range_offering_the_rate_and_prefers_stereo_f32() {
        use cpal::SampleFormat::{F32, I16};
        let range = |channels, min_hz, max_hz, format| SupportedRange {
            channels,
            min_hz,
            max_hz,
            format,
        };
        let fixed_48k = range(2, 48_000, 48_000, F32);
        assert_eq!(choose_config(&[fixed_48k], 44_100), Choice::Resample);
        assert_eq!(
            choose_config(&[fixed_48k, range(2, 8_000, 96_000, I16)], 44_100),
            Choice::Native(1)
        );
        let ranges = [
            range(1, 8_000, 96_000, F32),
            range(2, 44_100, 44_100, I16),
            range(2, 8_000, 192_000, F32),
        ];
        assert_eq!(choose_config(&ranges, 44_100), Choice::Native(2));
        // A format the stream builders can't drive is never chosen.
        assert_eq!(
            choose_config(&[range(2, 8_000, 96_000, cpal::SampleFormat::F64)], 44_100),
            Choice::Resample
        );
    }

    #[test]
    fn the_soft_clip_is_linear_below_the_knee_and_stays_within_full_scale() {
        for x in [0.0, 0.25, -0.5, KNEE, -KNEE] {
            assert_eq!(soft_clip(x), x);
        }
        let mut last = KNEE;
        for i in 1..=100 {
            let y = soft_clip(KNEE + i as f32 * 0.05);
            assert!(y >= last && y <= 1.0, "{y} after {last}");
            last = y;
        }
        assert_eq!(soft_clip(-4.0), -soft_clip(4.0));
    }
}
