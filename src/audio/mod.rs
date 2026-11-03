//! SoundFont synthesis (`rustysynth`) on a real-time `cpal` stream.
//!
//! The audio callback owns the sequencer and synthesizer and is the master
//! clock: it publishes the song position of each buffer, stamped with when
//! that buffer will be heard, through a lock-free [`SharedClock`].

mod clock;
mod sequencer;
mod soundfont;

pub use clock::{ClockSnapshot, SharedClock};
pub use sequencer::{AudioCmd, RawMidi};
pub use soundfont::{LoadedSoundFont, SoundFontLoader};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, OutputCallbackInfo, SizedSample};
use rtrb::{Consumer, Producer, RingBuffer};

use sequencer::{Garbage, Sequencer, Silence, Synth as _};

/// Command queue depth; the main thread sends a handful per frame at most.
const COMMAND_CAPACITY: usize = 256;
/// Garbage queue depth; drained every frame.
const GARBAGE_CAPACITY: usize = 64;
/// Largest chunk rendered at once; device buffers larger than this are split.
const MAX_CHUNK: usize = 4096;

/// Human-readable description of the running stream.
#[derive(Debug, Clone)]
pub struct AudioInfo {
    /// Output device name.
    pub device: String,
    /// Stream sample rate in Hz.
    pub sample_rate: u32,
    /// Requested buffer size in frames, or `None` for the device default.
    pub buffer_frames: Option<u32>,
}

/// A running output stream plus the queues that talk to it.
pub struct AudioEngine {
    // Dropping the stream stops the callback; kept for its lifetime only.
    _stream: cpal::Stream,
    commands: Producer<AudioCmd>,
    garbage: Consumer<Garbage>,
    clock: Arc<SharedClock>,
    failed: Arc<AtomicBool>,
    info: AudioInfo,
}

impl AudioEngine {
    /// Opens the output device named `device_name` (the system default if
    /// `None` or not found), asking for `buffer_frames` per callback (falls
    /// back to the device default if refused).
    pub fn start(device_name: Option<&str>, buffer_frames: u32) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let named = device_name.and_then(|wanted| {
            let found = host.output_devices().ok()?.find(|d| device_label(d) == wanted);
            if found.is_none() {
                log::warn!("output device \"{wanted}\" not found; using the default");
            }
            found
        });
        let device = match named {
            Some(device) => device,
            None => host.default_output_device().context("no audio output device found")?,
        };
        let name = device_label(&device);
        let supported = device.default_output_config().context("querying the output format")?;

        let fixed = match *supported.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => buffer_frames.clamp(min, max),
            cpal::SupportedBufferSize::Unknown => buffer_frames,
        };
        match Self::open(&device, &supported, Some(fixed), &name) {
            Ok(engine) => Ok(engine),
            Err(err) => {
                log::warn!("{fixed}-frame buffer refused ({err:#}); using the device default");
                Self::open(&device, &supported, None, &name)
            }
        }
    }

    fn open(
        device: &cpal::Device,
        supported: &cpal::SupportedStreamConfig,
        buffer_frames: Option<u32>,
        name: &str,
    ) -> anyhow::Result<Self> {
        let mut config = supported.config();
        config.buffer_size = buffer_frames.map_or(cpal::BufferSize::Default, cpal::BufferSize::Fixed);

        let (commands, command_rx) = RingBuffer::new(COMMAND_CAPACITY);
        let (garbage_tx, garbage) = RingBuffer::new(GARBAGE_CAPACITY);
        let clock = Arc::new(SharedClock::new());
        let failed = Arc::new(AtomicBool::new(false));
        let state = CallbackState {
            sequencer: Sequencer::new(),
            synth: None,
            commands: command_rx,
            live: None,
            garbage: garbage_tx,
            clock: clock.clone(),
            left: vec![0.0; MAX_CHUNK],
            right: vec![0.0; MAX_CHUNK],
            sample_rate: f64::from(config.sample_rate),
            gain: 0.0,
            target_gain: 1.0,
        };

        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(device, &config, state, failed.clone()),
            cpal::SampleFormat::I16 => build::<i16>(device, &config, state, failed.clone()),
            cpal::SampleFormat::U16 => build::<u16>(device, &config, state, failed.clone()),
            cpal::SampleFormat::I32 => build::<i32>(device, &config, state, failed.clone()),
            other => anyhow::bail!("unsupported output sample format {other:?}"),
        }?;
        stream.play().context("starting the audio stream")?;

        let info = AudioInfo { device: name.to_owned(), sample_rate: config.sample_rate, buffer_frames };
        log::info!("audio: {} @ {} Hz, buffer {:?}", info.device, info.sample_rate, info.buffer_frames);
        Ok(Self { _stream: stream, commands, garbage, clock, failed, info })
    }

    /// Queues a command for the audio thread. Returns `false` if the queue is full.
    pub fn send(&mut self, cmd: AudioCmd) -> bool {
        match self.commands.push(cmd) {
            Ok(()) => true,
            Err(_) => {
                log::warn!("audio command queue full; command dropped");
                false
            }
        }
    }

    /// Frees objects the audio thread has retired. Call once per frame.
    pub fn collect_garbage(&mut self) {
        while let Ok(item) = self.garbage.pop() {
            drop(item);
        }
    }

    /// The clock published by the audio callback.
    pub fn clock(&self) -> &SharedClock {
        &self.clock
    }

    /// Whether the stream reported an error (e.g. the device was unplugged).
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Output latency reported by the device for the last buffer.
    pub fn output_latency(&self) -> std::time::Duration {
        self.clock.latency()
    }

    /// Stream description.
    pub fn info(&self) -> &AudioInfo {
        &self.info
    }
}

/// Names of the available output devices.
pub fn list_output_devices() -> Vec<String> {
    match cpal::default_host().output_devices() {
        Ok(devices) => devices.map(|d| device_label(&d)).collect(),
        Err(err) => {
            log::warn!("listing output devices failed: {err}");
            Vec::new()
        }
    }
}

fn device_label(device: &cpal::Device) -> String {
    device.description().map(|d| d.name().to_owned()).unwrap_or_else(|_| "unknown device".into())
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut state: CallbackState,
    failed: Arc<AtomicBool>,
) -> anyhow::Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let stream = device.build_output_stream::<T, _, _>(
        *config,
        move |data: &mut [T], info: &OutputCallbackInfo| state.process(data, channels, info),
        move |err| {
            log::error!("audio stream error: {err}");
            failed.store(true, Ordering::Relaxed);
        },
        None,
    )?;
    Ok(stream)
}

/// Everything the real-time callback owns. Allocated once, before the stream starts.
struct CallbackState {
    sequencer: Sequencer,
    synth: Option<Box<rustysynth::Synthesizer>>,
    commands: Consumer<AudioCmd>,
    /// Live MIDI input, played immediately at the start of each buffer.
    live: Option<Consumer<RawMidi>>,
    garbage: Producer<Garbage>,
    clock: Arc<SharedClock>,
    left: Vec<f32>,
    right: Vec<f32>,
    sample_rate: f64,
    gain: f32,
    target_gain: f32,
}

impl CallbackState {
    fn process<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T], channels: usize, info: &OutputCallbackInfo) {
        let Self { sequencer, synth, commands, live, garbage, clock, left, right, sample_rate, gain, target_gain } =
            self;
        let mut trash = |item: Garbage| {
            // The main thread drains this queue every frame, so it is never
            // full in practice; if it were, the item would be freed here.
            let _ = garbage.push(item);
        };

        while let Ok(cmd) = commands.pop() {
            match cmd {
                AudioCmd::SetSynth(new) => {
                    if let Some(old) = synth.replace(new) {
                        trash(Garbage::Synth(old));
                    }
                    if let Some(s) = synth.as_deref_mut() {
                        sequencer.resync(s);
                    }
                }
                AudioCmd::SetVolume(v) => *target_gain = v.clamp(0.0, 1.0),
                AudioCmd::SetLiveInput(queue) => {
                    if let Some(old) = live.replace(queue) {
                        trash(Garbage::LiveInput(old));
                    }
                }
                other => match synth.as_deref_mut() {
                    Some(s) => sequencer.apply(other, s, &mut trash),
                    None => sequencer.apply(other, &mut Silence, &mut trash),
                },
            }
        }

        // Live notes go straight to the synth: latency is one buffer plus the output latency.
        if let (Some(queue), Some(s)) = (live.as_mut(), synth.as_deref_mut()) {
            while let Ok(m) = queue.pop() {
                s.midi(m.status & 0x0F, m.status & 0xF0, m.data1, m.data2);
            }
        } else if let Some(queue) = live.as_mut() {
            // No SoundFont yet: drop live input so it doesn't pile up.
            while queue.pop().is_ok() {}
        }

        let ts = info.timestamp();
        let latency = ts.playback.saturating_duration_since(ts.callback);
        clock.set_latency(latency);
        clock.publish(&ClockSnapshot {
            position_s: sequencer.position(),
            heard_at: clock.epoch().elapsed() + latency,
            speed: sequencer.speed(),
            playing: sequencer.is_playing(),
            ended: sequencer.has_ended(),
            seek_serial: sequencer.seek_serial(),
        });

        let channels = channels.max(1);
        let frames = out.len() / channels;
        // ~5 ms one-pole smoothing so volume changes don't click.
        let smoothing = 1.0 - (-1.0 / (0.005 * *sample_rate)).exp() as f32;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_CHUNK);
            let (l, r) = (&mut left[..n], &mut right[..n]);
            match synth.as_deref_mut() {
                Some(s) => sequencer.render(s, l, r, *sample_rate),
                None => sequencer.render(&mut Silence, l, r, *sample_rate),
            }
            for (i, frame) in out[done * channels..(done + n) * channels].chunks_exact_mut(channels).enumerate() {
                *gain += (*target_gain - *gain) * smoothing;
                let a = (l[i] * *gain).clamp(-1.0, 1.0);
                let b = (r[i] * *gain).clamp(-1.0, 1.0);
                if channels == 1 {
                    frame[0] = T::from_sample((a + b) * 0.5);
                } else {
                    frame[0] = T::from_sample(a);
                    frame[1] = T::from_sample(b);
                    for extra in &mut frame[2..] {
                        *extra = T::EQUILIBRIUM;
                    }
                }
            }
            done += n;
        }
    }
}
