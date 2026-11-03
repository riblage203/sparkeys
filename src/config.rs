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
    /// Live MIDI input.
    pub live: LiveConfig,
    /// Frame pacing.
    pub display: DisplayConfig,
}

/// Live mode settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveConfig {
    /// Start in live mode.
    pub enabled: bool,
    /// Preferred input device name (`None` = first one found).
    pub device: Option<String>,
    /// Keys below this MIDI note are colored as the left hand (60 = middle C).
    pub split_key: u8,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self { enabled: false, device: None, split_key: 60 }
    }
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
    /// Note color scheme.
    pub palette: PaletteKind,
    /// What picks a note's color.
    pub color_mode: ColorMode,
    /// Slowly moving smoke and haze in the background.
    pub animated_background: bool,
}

impl Default for VisualConfig {
    fn default() -> Self {
        Self {
            bloom: 1.0,
            particles: 1.0,
            corner_radius: 5.0,
            show_hit_line: true,
            palette: PaletteKind::default(),
            color_mode: ColorMode::default(),
            animated_background: true,
        }
    }
}

/// Built-in note color schemes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PaletteKind {
    /// Warm orange / cool cyan plus accents.
    #[default]
    Ember,
    /// Teal, violet and blue.
    Aurora,
    /// Saturated club colors.
    Neon,
    /// Pale blues and silver.
    Ice,
}

impl PaletteKind {
    /// All schemes, for pickers.
    pub const ALL: [Self; 4] = [Self::Ember, Self::Aurora, Self::Neon, Self::Ice];

    /// Display name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ember => "Ember",
            Self::Aurora => "Aurora",
            Self::Neon => "Neon",
            Self::Ice => "Ice",
        }
    }
}

/// How notes are colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    /// One color per track/part.
    #[default]
    Track,
    /// Left hand / right hand, split at [`LiveConfig::split_key`].
    Hand,
}

/// Window and frame pacing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// Cap the frame rate (useful when the driver ignores vsync).
    pub limit_fps: bool,
    /// Frame-rate cap; 0 = the monitor's refresh rate.
    pub max_fps: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self { limit_fps: true, max_fps: 0 }
    }
}

/// Audio output settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Last SoundFont, reloaded on startup.
    pub soundfont: Option<PathBuf>,
    /// Output device name (`None` = system default).
    pub output_device: Option<String>,
    /// Output volume, 0..=1.
    pub volume: f32,
    /// Requested frames per audio callback (lower = less latency, more CPU).
    pub buffer_frames: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { soundfont: None, output_device: None, volume: 0.8, buffer_frames: 256 }
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
    /// Preferred location: `config.toml` next to the executable (portable installs).
    pub fn exe_dir_path() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        Some(exe.parent()?.join(FILE_NAME))
    }

    /// Fallback location in the OS config directory, used when the executable's
    /// folder is not writable (e.g. under Program Files).
    pub fn os_config_path() -> Option<PathBuf> {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let base = if cfg!(target_os = "windows") {
            var("APPDATA")?
        } else if cfg!(target_os = "macos") {
            var("HOME")?.join("Library").join("Application Support")
        } else {
            var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))?
        };
        Some(base.join("sparkeys").join(FILE_NAME))
    }

    fn candidates() -> impl Iterator<Item = PathBuf> {
        Self::exe_dir_path().into_iter().chain(Self::os_config_path())
    }

    /// Loads settings from the first location that has a file.
    ///
    /// Never fails: with no file the defaults are used silently; with an
    /// unreadable or invalid file the defaults are used and a message for the
    /// user is returned alongside.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = Self::candidates().find(|p| p.exists()) else {
            return (Self::default(), None);
        };
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

    /// Saves next to the executable, or in the OS config directory if that fails.
    pub fn save(&self) -> anyhow::Result<()> {
        let mut last_err = anyhow::anyhow!("no location to save settings to");
        for path in Self::candidates() {
            if let Some(dir) = path.parent()
                && let Err(err) = std::fs::create_dir_all(dir)
            {
                last_err = anyhow::Error::from(err).context(format!("creating {}", dir.display()));
                continue;
            }
            match self.save_to(&path) {
                Ok(()) => return Ok(()),
                Err(err) => last_err = err,
            }
        }
        Err(last_err)
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
                output_device: Some("Speakers".into()),
                volume: 0.5,
                buffer_frames: 128,
            },
            visual: VisualConfig {
                bloom: 1.5,
                particles: 0.5,
                corner_radius: 2.0,
                show_hit_line: false,
                palette: PaletteKind::Neon,
                color_mode: ColorMode::Hand,
                animated_background: false,
            },
            display: DisplayConfig { limit_fps: false, max_fps: 144 },
            live: LiveConfig { enabled: true, device: Some("Digital Piano".into()), split_key: 55 },
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
