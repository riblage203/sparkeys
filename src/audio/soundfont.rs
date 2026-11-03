//! SoundFont loading off the main thread.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use anyhow::Context as _;
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

/// Voices available at once; rustysynth's maximum. A sustained piano needs many.
const MAX_POLYPHONY: usize = 256;

/// A parsed SoundFont, shareable between synthesizers.
pub struct LoadedSoundFont {
    /// Where it was loaded from.
    pub path: PathBuf,
    /// The parsed font.
    pub font: Arc<SoundFont>,
}

impl LoadedSoundFont {
    /// File name for display.
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Builds a synthesizer for this font at `sample_rate`.
    pub fn make_synth(&self, sample_rate: u32) -> anyhow::Result<Box<Synthesizer>> {
        let mut settings = SynthesizerSettings::new(i32::try_from(sample_rate).context("sample rate out of range")?);
        settings.maximum_polyphony = MAX_POLYPHONY;
        Ok(Box::new(Synthesizer::new(&self.font, &settings)?))
    }
}

/// A SoundFont being parsed on a background thread (large fonts take seconds).
pub struct SoundFontLoader {
    path: PathBuf,
    rx: Receiver<anyhow::Result<LoadedSoundFont>>,
}

impl SoundFontLoader {
    /// Starts loading `path`.
    pub fn spawn(path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let thread_path = path.clone();
        let spawned = std::thread::Builder::new().name("soundfont-loader".into()).spawn(move || {
            let result = load(thread_path);
            // The receiver may be gone if another font was picked meanwhile; nothing to do then.
            let _ = tx.send(result);
        });
        if let Err(err) = spawned {
            let (tx, rx_err) = mpsc::channel();
            let _ = tx.send(Err(anyhow::Error::from(err).context("could not start loader thread")));
            return Self { path, rx: rx_err };
        }
        Self { path, rx }
    }

    /// File being loaded.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The result once loading has finished.
    pub fn poll(&self) -> Option<anyhow::Result<LoadedSoundFont>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!("loader thread stopped unexpectedly"))),
        }
    }
}

fn load(path: PathBuf) -> anyhow::Result<LoadedSoundFont> {
    let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let font = SoundFont::new(&mut BufReader::new(file)).context("not a valid SoundFont (.sf2)")?;
    Ok(LoadedSoundFont { path, font: Arc::new(font) })
}
