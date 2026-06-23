//! Host audio for the in-process link: bridge the app's audio rings to the
//! machine's speakers/mic via `cpal`.
//!
//! The output stream drains app→GUI frames (silence on underrun); the input
//! stream fills GUI→app frames (silence if there's no input device). The output
//! callback is effectively the audio clock that paces the app's DSP loop.

use cpal::Sample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use deluge_sim_link::audio::{Consumer, GuiEnds, HeapCons, HeapProd, Producer};

/// Keeps the cpal streams alive for the lifetime of the simulator window
/// (dropping a `cpal::Stream` stops it).
pub struct AudioStreams {
    _output: Option<cpal::Stream>,
    _input: Option<cpal::Stream>,
}

/// Open the default output (and, best-effort, input) device and wire them to the
/// app's audio rings. Failures are logged and degrade to silence so the app
/// still runs.
pub fn start(gui: GuiEnds) -> AudioStreams {
    let GuiEnds { out, in_ } = gui;
    let host = cpal::default_host();

    let output = match start_output(&host, out) {
        Ok(s) => Some(s),
        Err(e) => {
            log::warn!("simulator audio: no output ({e}); running silent");
            None
        }
    };
    let input = match start_input(&host, in_) {
        Ok(s) => Some(s),
        Err(e) => {
            log::info!("simulator audio: no input ({e}); mic is silent");
            None
        }
    };

    AudioStreams {
        _output: output,
        _input: input,
    }
}

fn start_output(host: &cpal::Host, out: HeapCons<[f32; 2]>) -> Result<cpal::Stream, String> {
    let dev = host
        .default_output_device()
        .ok_or("no default output device")?;
    let cfg = dev
        .default_output_config()
        .map_err(|e| format!("output config: {e}"))?;
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let stream = match fmt {
        cpal::SampleFormat::F32 => build_output::<f32>(&dev, &config, out),
        cpal::SampleFormat::I16 => build_output::<i16>(&dev, &config, out),
        cpal::SampleFormat::U16 => build_output::<u16>(&dev, &config, out),
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
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    dev.build_output_stream(
        config.clone(),
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            for frame in data.chunks_mut(channels.max(1)) {
                let s = out.try_pop().unwrap_or([0.0, 0.0]);
                for (i, ch) in frame.iter_mut().enumerate() {
                    let v = match i {
                        0 => s[0],
                        1 => s[1],
                        _ => 0.0,
                    };
                    *ch = T::from_sample(v);
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
