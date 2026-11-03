//! Live MIDI input via `midir`: device selection, hot-plug and the
//! low-latency event path.
//!
//! The input callback pushes every message into two lock-free queues: one
//! read by the audio callback (played on the next buffer, no main-thread hop)
//! and one read by the renderer each frame.

mod keyboard;
mod state;

pub use state::{LiveHit, LiveNote, LiveState};

use std::time::{Duration, Instant};

use anyhow::Context as _;
use midir::{Ignore, MidiInput, MidiInputConnection};
use rtrb::{Consumer, RingBuffer};
use winit::keyboard::KeyCode;

use crate::app::AudioState;
use crate::audio::RawMidi;
use keyboard::KeyboardInput;
use state::LiveEvent;

/// Device-list entry for the computer-keyboard fallback.
pub const COMPUTER_KEYBOARD: &str = "Computer keyboard";
/// How often the device list is refreshed (hot-plug detection).
const RESCAN_INTERVAL: Duration = Duration::from_secs(1);
/// Queue depth; a whole chord flurry fits many times over.
const QUEUE_CAPACITY: usize = 1024;

enum Source {
    /// A hardware/virtual MIDI port; dropping the connection closes it.
    Midi { name: String, _connection: MidiInputConnection<()> },
    /// Notes typed on the computer keyboard.
    Keyboard(KeyboardInput),
}

struct Connection {
    source: Source,
    events: Consumer<LiveEvent>,
    /// Audio engine generation the live queue was handed to.
    audio_generation: u64,
}

impl Connection {
    fn name(&self) -> &str {
        match &self.source {
            Source::Midi { name, .. } => name,
            Source::Keyboard(_) => COMPUTER_KEYBOARD,
        }
    }
}

/// Live input device management.
pub struct LiveInput {
    enabled: bool,
    /// Device the user picked (kept while it is unplugged).
    wanted: Option<String>,
    /// Selectable devices: the computer keyboard plus the MIDI ports found at the last scan.
    choices: Vec<String>,
    connection: Option<Connection>,
    next_scan: Instant,
    status: String,
    last_error: Option<String>,
}

impl LiveInput {
    /// Inactive until [`Self::set_enabled`]; `wanted` is the remembered device.
    pub fn new(wanted: Option<String>) -> Self {
        Self {
            enabled: false,
            wanted,
            choices: vec![COMPUTER_KEYBOARD.to_owned()],
            connection: None,
            next_scan: Instant::now(),
            status: String::new(),
            last_error: None,
        }
    }

    /// Turns live mode on or off; off closes the device.
    pub fn set_enabled(&mut self, enabled: bool, state: &mut LiveState, audio: &mut AudioState) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            self.disconnect(state, audio);
        }
        self.next_scan = Instant::now();
    }

    /// Chooses a device (`None` = first MIDI port found); takes effect immediately.
    pub fn select(&mut self, name: Option<String>, state: &mut LiveState, audio: &mut AudioState) {
        if self.wanted == name && self.connection.is_some() {
            return;
        }
        self.wanted = name;
        self.last_error = None;
        self.disconnect(state, audio);
        self.next_scan = Instant::now();
    }

    /// The remembered device choice (persisted in the config).
    pub fn wanted(&self) -> Option<&str> {
        self.wanted.as_deref()
    }

    /// Selectable device names.
    pub fn choices(&self) -> &[String] {
        &self.choices
    }

    /// One-line status for the UI.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Whether a device is open.
    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }

    /// Feeds a key to the computer-keyboard input. Returns `true` if it was
    /// used as a note (or octave change) and should not trigger shortcuts.
    pub fn key_event(&mut self, code: KeyCode, pressed: bool, repeat: bool, loud: bool) -> bool {
        match self.connection.as_mut().map(|c| &mut c.source) {
            Some(Source::Keyboard(kb)) if self.enabled => kb.key(code, pressed, repeat, loud),
            _ => false,
        }
    }

    /// Releases computer-keyboard notes (window lost focus, keys may never come up).
    pub fn release_keys(&mut self) {
        if let Some(Source::Keyboard(kb)) = self.connection.as_mut().map(|c| &mut c.source) {
            kb.release_all();
        }
    }

    /// Per frame: hot-plug handling and draining received events into `state`.
    pub fn update(&mut self, state: &mut LiveState, audio: &mut AudioState, notices: &mut Vec<String>) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if now >= self.next_scan {
            self.next_scan = now + RESCAN_INTERVAL;
            self.rescan(state, audio, notices);
        }
        if let Some(conn) = &mut self.connection {
            while let Ok(event) = conn.events.pop() {
                state.apply(&event);
            }
        }
    }

    fn rescan(&mut self, state: &mut LiveState, audio: &mut AudioState, notices: &mut Vec<String>) {
        let ports = match list_ports() {
            Ok(ports) => ports,
            Err(err) => {
                log::warn!("listing MIDI inputs failed: {err:#}");
                Vec::new()
            }
        };
        self.choices.clear();
        self.choices.push(COMPUTER_KEYBOARD.to_owned());
        self.choices.extend(ports.iter().cloned());

        // Unplugged: close and wait for it to come back.
        if let Some(conn) = &self.connection
            && let Source::Midi { name, .. } = &conn.source
            && !ports.contains(name)
        {
            notices.push(format!("MIDI device \"{name}\" disconnected; it will reconnect when plugged back in."));
            self.disconnect(state, audio);
        }
        // The audio engine restarted, taking our live queue with it.
        if self.connection.as_ref().is_some_and(|c| c.audio_generation != audio.generation()) {
            self.disconnect(state, audio);
        }

        if self.connection.is_none() {
            let target = match &self.wanted {
                Some(name) if name == COMPUTER_KEYBOARD || ports.contains(name) => Some(name.clone()),
                Some(_) => None,
                None => ports.first().cloned(),
            };
            if let Some(name) = target {
                match self.connect(&name, audio) {
                    Ok(()) => {
                        log::info!("live input: {name}");
                        self.wanted = Some(name);
                        self.last_error = None;
                    }
                    Err(err) => {
                        let msg = format!("Could not open MIDI device \"{name}\": {err:#}");
                        if self.last_error.as_deref() != Some(msg.as_str()) {
                            notices.push(msg.clone());
                            self.last_error = Some(msg);
                        }
                    }
                }
            }
        }

        self.status = match (&self.connection, &self.wanted) {
            (Some(conn), _) => format!("connected: {}", conn.name()),
            (None, Some(name)) => format!("waiting for \"{name}\"…"),
            (None, None) => "no MIDI input found: plug in a keyboard or pick Computer keyboard".to_owned(),
        };
    }

    fn connect(&mut self, name: &str, audio: &mut AudioState) -> anyhow::Result<()> {
        let (audio_tx, audio_rx) = RingBuffer::<RawMidi>::new(QUEUE_CAPACITY);
        let (render_tx, render_rx) = RingBuffer::<LiveEvent>::new(QUEUE_CAPACITY);
        let source = if name == COMPUTER_KEYBOARD {
            Source::Keyboard(KeyboardInput::new(audio_tx, render_tx))
        } else {
            let mut input = MidiInput::new("sparkeys")?;
            input.ignore(Ignore::All);
            let port = input
                .ports()
                .into_iter()
                .find(|p| input.port_name(p).is_ok_and(|n| n == name))
                .context("device not found")?;
            let (mut audio_tx, mut render_tx) = (audio_tx, render_tx);
            let connection = input
                .connect(
                    &port,
                    "sparkeys-live",
                    move |_stamp, bytes, _| {
                        if let Some(raw) = parse_message(bytes) {
                            // Full queues drop the message rather than block the driver thread.
                            let _ = audio_tx.push(raw);
                            let _ = render_tx.push(LiveEvent { raw, at: Instant::now() });
                        }
                    },
                    (),
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Source::Midi { name: name.to_owned(), _connection: connection }
        };
        // Without an audio engine the notes still show; the queue is re-sent on reconnect.
        audio.set_live_input(audio_rx);
        self.connection = Some(Connection { source, events: render_rx, audio_generation: audio.generation() });
        Ok(())
    }

    fn disconnect(&mut self, state: &mut LiveState, audio: &mut AudioState) {
        if self.connection.take().is_some() {
            audio.all_notes_off();
            state.release_all();
        }
    }
}

/// Names of the MIDI input ports currently present.
fn list_ports() -> anyhow::Result<Vec<String>> {
    let input = MidiInput::new("sparkeys-scan")?;
    Ok(input.ports().iter().filter_map(|p| input.port_name(p).ok()).collect())
}

/// Keeps channel voice messages (note, controller, program, bend, ...); drops
/// system messages and malformed data.
fn parse_message(bytes: &[u8]) -> Option<RawMidi> {
    let status = *bytes.first()?;
    if !(0x80..0xF0).contains(&status) {
        return None;
    }
    let data = |i: usize| bytes.get(i).copied().unwrap_or(0) & 0x7F;
    Some(RawMidi { status, data1: data(1), data2: data(2) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_channel_messages_only() {
        assert_eq!(parse_message(&[0x93, 60, 100]), Some(RawMidi { status: 0x93, data1: 60, data2: 100 }));
        assert_eq!(parse_message(&[0xC0, 5]), Some(RawMidi { status: 0xC0, data1: 5, data2: 0 }));
        assert_eq!(parse_message(&[0xF8]), None);
        assert_eq!(parse_message(&[0x40, 1, 2]), None);
        assert_eq!(parse_message(&[]), None);
    }
}
