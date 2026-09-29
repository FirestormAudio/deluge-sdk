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

use cpal::Sample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use deluge_sim_link::audio::{
    BLOCK_FRAMES, Consumer, GuiEnds, HeapCons, HeapProd, HeapRb, Observer, Producer,
    SAMPLE_RATE_HZ, Split,
};

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
    let cfg = dev
        .default_output_config()
        .map_err(|e| format!("output config: {e}"))?;
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let stream = match fmt {
        cpal::SampleFormat::F32 => build_output::<f32>(&dev, &config, out, mon, volume, boost, rec),
        cpal::SampleFormat::I16 => build_output::<i16>(&dev, &config, out, mon, volume, boost, rec),
        cpal::SampleFormat::U16 => build_output::<u16>(&dev, &config, out, mon, volume, boost, rec),
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
            for frame in data.chunks_mut(channels.max(1)) {
                let s = out.try_pop().unwrap_or([0.0, 0.0]);
                // Tee the *pre-volume* stereo frame to the scope monitor (and the
                // recorder, if any), so both capture the app's true output
                // regardless of the monitor knob.
                let _ = mon.try_push(s);
                if let Some(rec) = rec.as_mut() {
                    let _ = rec.try_push(s);
                }
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
    let cfg = dev
        .default_input_config()
        .map_err(|e| format!("input config: {e}"))?;
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let stream = match fmt {
        cpal::SampleFormat::F32 => build_input::<f32>(&dev, &config, in_),
        cpal::SampleFormat::I16 => build_input::<i16>(&dev, &config, in_),
        cpal::SampleFormat::U16 => build_input::<u16>(&dev, &config, in_),
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
                let _ = in_.try_push([l, r]);
            }
        },
        |e| log::warn!("simulator audio input error: {e}"),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
