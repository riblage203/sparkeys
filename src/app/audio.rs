//! App-side audio state: engine lifetime, SoundFont loading, reconnects.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::audio::{AudioCmd, AudioEngine, LoadedSoundFont, RawMidi, SoundFontLoader, list_output_devices};
use crate::config::AudioConfig;
use crate::midi::Song;
use crate::timeline::{AudioSync, Transport};

/// How often to try reopening the output device after it failed or was missing.
const RETRY_INTERVAL: Duration = Duration::from_secs(3);

/// Owns the audio engine (if any) and everything needed to rebuild it.
pub struct AudioState {
    engine: Option<AudioEngine>,
    sync: AudioSync,
    font: Option<LoadedSoundFont>,
    loader: Option<SoundFontLoader>,
    retry_at: Option<Instant>,
    output_device: Option<String>,
    buffer_frames: u32,
    /// Output devices found at the last [`Self::refresh_devices`].
    devices: Vec<String>,
    /// Incremented each time an engine starts, so live inputs know to re-attach.
    generation: u64,
}

impl AudioState {
    /// Opens the output device and starts loading the configured SoundFont.
    /// Problems are reported as notices; the app keeps working silently.
    pub fn new(config: &AudioConfig, notices: &mut Vec<String>) -> Self {
        let mut state = Self {
            engine: None,
            sync: AudioSync::default(),
            font: None,
            loader: None,
            retry_at: None,
            output_device: config.output_device.clone(),
            buffer_frames: config.buffer_frames.clamp(32, 8192),
            devices: Vec::new(),
            generation: 0,
        };
        state.start_engine(Some(notices));
        if let Some(path) = &config.soundfont {
            state.load_soundfont(path.clone());
        }
        state
    }

    fn start_engine(&mut self, notices: Option<&mut Vec<String>>) {
        match AudioEngine::start(self.output_device.as_deref(), self.buffer_frames) {
            Ok(mut engine) => {
                if let Some(font) = &self.font {
                    install_synth(&mut engine, font);
                }
                self.engine = Some(engine);
                self.sync.reset();
                self.retry_at = None;
                self.generation += 1;
            }
            Err(err) => {
                log::warn!("audio unavailable: {err:#}");
                if let Some(notices) = notices {
                    notices.push(format!("No audio output ({err:#}). Playing silently; will keep retrying."));
                }
                self.retry_at = Some(Instant::now() + RETRY_INTERVAL);
            }
        }
    }

    /// Switches output device and/or buffer size, restarting the stream if anything changed.
    pub fn reconfigure(&mut self, output_device: Option<String>, buffer_frames: u32, notices: &mut Vec<String>) {
        let buffer_frames = buffer_frames.clamp(32, 8192);
        if output_device == self.output_device && buffer_frames == self.buffer_frames && self.engine.is_some() {
            return;
        }
        self.output_device = output_device;
        self.buffer_frames = buffer_frames;
        // Drop the old stream first: some drivers allow only one open stream per device.
        self.engine = None;
        self.start_engine(Some(notices));
    }

    /// Re-reads the list of output devices.
    pub fn refresh_devices(&mut self) {
        self.devices = list_output_devices();
    }

    /// Output devices found at the last refresh.
    pub fn devices(&self) -> &[String] {
        &self.devices
    }

    /// Starts loading a SoundFont in the background; it replaces the current one when ready.
    pub fn load_soundfont(&mut self, path: PathBuf) {
        self.loader = Some(SoundFontLoader::spawn(path));
    }

    /// Handles finished SoundFont loads and device failures. Call once per frame.
    pub fn poll(&mut self, config: &mut AudioConfig, notices: &mut Vec<String>) {
        if let Some(result) = self.loader.as_ref().and_then(SoundFontLoader::poll) {
            let path = self.loader.take().map(|l| l.path().to_path_buf()).unwrap_or_default();
            match result {
                Ok(font) => {
                    log::info!("SoundFont loaded: {}", font.path.display());
                    if let Some(engine) = &mut self.engine {
                        install_synth(engine, &font);
                    }
                    config.soundfont = Some(font.path.clone());
                    self.font = Some(font);
                }
                Err(err) => {
                    notices.push(format!("Could not load SoundFont {}: {err:#}", path.display()));
                    if config.soundfont.as_deref() == Some(path.as_path()) {
                        config.soundfont = None;
                    }
                }
            }
        }

        if self.engine.as_ref().is_some_and(AudioEngine::has_failed) {
            notices.push("Audio device lost; reconnecting…".into());
            self.engine = None;
            self.retry_at = Some(Instant::now() + RETRY_INTERVAL);
        }
        if self.engine.is_none() && self.retry_at.is_some_and(|t| Instant::now() >= t) {
            self.start_engine(None);
        }
    }

    /// Forwards transport changes to the audio thread and updates the
    /// transport position from the audio clock (wall clock without audio).
    pub fn update_transport(
        &mut self,
        transport: &mut Transport,
        song: Option<&Song>,
        song_id: u64,
        volume: f32,
        dt_s: f64,
    ) {
        self.sync.update(self.engine.as_mut(), transport, song, song_id, volume, dt_s);
    }

    /// Changes whenever the audio engine is (re)started.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Routes a live input queue straight into the audio callback.
    /// Returns `false` (and drops the queue) when there is no audio device.
    pub fn set_live_input(&mut self, queue: rtrb::Consumer<RawMidi>) -> bool {
        self.engine.as_mut().is_some_and(|e| e.send(AudioCmd::SetLiveInput(queue)))
    }

    /// Releases all sounding notes and the pedal.
    pub fn all_notes_off(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.send(AudioCmd::AllNotesOff);
        }
    }

    /// Visual minus audio time in ms, for the debug overlay.
    pub fn sync_error_ms(&self) -> Option<f64> {
        self.sync.sync_error_ms()
    }

    /// One-line status for the UI.
    pub fn status(&self) -> String {
        let font = match (&self.loader, &self.font) {
            (Some(loader), _) => format!(
                "loading {}…",
                loader.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            ),
            (None, Some(font)) => font.name(),
            (None, None) => "no SoundFont (silent)".into(),
        };
        match &self.engine {
            Some(engine) => {
                let info = engine.info();
                let buffer = info.buffer_frames.map_or_else(|| "default buffer".to_owned(), |n| format!("{n} frames"));
                let latency_ms = engine.output_latency().as_secs_f64() * 1000.0;
                format!("{font} · {} · {} Hz, {buffer}, out {latency_ms:.1} ms", info.device, info.sample_rate)
            }
            None => format!("{font} · no audio device"),
        }
    }
}

fn install_synth(engine: &mut AudioEngine, font: &LoadedSoundFont) {
    match font.make_synth(engine.info().sample_rate) {
        Ok(synth) => {
            engine.send(AudioCmd::SetSynth(synth));
        }
        Err(err) => log::error!("could not create synthesizer: {err:#}"),
    }
}
