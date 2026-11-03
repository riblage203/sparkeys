//! App-side audio state: engine lifetime, SoundFont loading, reconnects.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::audio::{AudioCmd, AudioEngine, LoadedSoundFont, SoundFontLoader};
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
    buffer_frames: u32,
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
            buffer_frames: config.buffer_frames.clamp(32, 8192),
        };
        state.start_engine(Some(notices));
        if let Some(path) = &config.soundfont {
            state.load_soundfont(path.clone());
        }
        state
    }

    fn start_engine(&mut self, notices: Option<&mut Vec<String>>) {
        match AudioEngine::start(self.buffer_frames) {
            Ok(mut engine) => {
                if let Some(font) = &self.font {
                    install_synth(&mut engine, font);
                }
                self.engine = Some(engine);
                self.sync.reset();
                self.retry_at = None;
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
                format!("{font} · {} · {} Hz, {buffer}", info.device, info.sample_rate)
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
