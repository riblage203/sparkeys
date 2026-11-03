//! User settings persisted as `config.toml` next to the executable.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// File name of the settings file.
const FILE_NAME: &str = "config.toml";

/// All persisted settings. Missing fields fall back to their defaults so old
/// files keep loading as new settings are added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// Window geometry.
    pub window: WindowConfig,
    /// Overlay visibility.
    pub ui: UiConfig,
    /// Playback preferences.
    pub playback: PlaybackConfig,
    /// Audio output.
    pub audio: AudioConfig,
    /// Look of the scene.
    pub visual: VisualConfig,
}

/// Visual settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualConfig {
    /// Bloom intensity, 0 = off.
    pub bloom: f32,
    /// Particle amount multiplier, 0 = off.
    pub particles: f32,
    /// Note corner radius in pixels.
    pub corner_radius: f32,
    /// Whether the glowing hit line is drawn.
    pub show_hit_line: bool,
}

impl Default for VisualConfig {
    fn default() -> Self {
        Self { bloom: 1.0, particles: 1.0, corner_radius: 5.0, show_hit_line: true }
    }
}

/// Audio output settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Last SoundFont, reloaded on startup.
    pub soundfont: Option<PathBuf>,
    /// Output volume, 0..=1.
    pub volume: f32,
    /// Requested frames per audio callback (lower = less latency, more CPU).
    pub buffer_frames: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { soundfont: None, volume: 0.8, buffer_frames: 256 }
    }
}

/// Window geometry restored on startup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    /// Logical width of the window.
    pub width: u32,
    /// Logical height of the window.
    pub height: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self { width: 1600, height: 900 }
    }
}

/// Overlay visibility toggles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Whether the egui panels are shown (toggled with H).
    pub visible: bool,
    /// Whether the FPS counter is shown (toggled with F3).
    pub show_fps: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { visible: true, show_fps: false }
    }
}

/// Playback preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackConfig {
    /// Playback speed multiplier.
    pub speed: f64,
    /// Seconds of music visible above the hit line.
    pub fall_seconds: f64,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self { speed: 1.0, fall_seconds: 3.0 }
    }
}

impl Config {
    /// Default location of the settings file: next to the executable.
    pub fn default_path() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        Some(exe.parent()?.join(FILE_NAME))
    }

    /// Loads settings from the default location.
    ///
    /// Never fails: on a missing file the defaults are used silently, on an
    /// unreadable or invalid file the defaults are used and a message for the
    /// user is returned alongside.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = Self::default_path() else {
            return (
                Self::default(),
                Some("Could not locate the executable directory; settings will not be saved.".into()),
            );
        };
        if !path.exists() {
            return (Self::default(), None);
        }
        match Self::load_from(&path) {
            Ok(cfg) => (cfg, None),
            Err(err) => (Self::default(), Some(format!("Ignoring invalid settings file: {err:#}"))),
        }
    }

    /// Reads and parses a settings file.
    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Saves settings to the default location.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::default_path().context("executable directory unknown")?;
        self.save_to(&path)
    }

    /// Serializes settings to `path`.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let path = std::env::temp_dir().join(format!("sparkeys-cfg-{}.toml", std::process::id()));
        let cfg = Config {
            window: WindowConfig { width: 1234, height: 567 },
            ui: UiConfig { visible: false, show_fps: true },
            playback: PlaybackConfig { speed: 0.5, fall_seconds: 4.5 },
            audio: AudioConfig {
                soundfont: Some(PathBuf::from("C:/fonts/piano.sf2")),
                volume: 0.5,
                buffer_frames: 128,
            },
            visual: VisualConfig { bloom: 1.5, particles: 0.5, corner_radius: 2.0, show_hit_line: false },
        };
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(cfg, loaded);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let cfg: Config = toml::from_str("[ui]\nshow_fps = true\n").unwrap();
        assert!(cfg.ui.show_fps);
        assert!(cfg.ui.visible);
        assert_eq!(cfg.window, WindowConfig::default());
    }
}
