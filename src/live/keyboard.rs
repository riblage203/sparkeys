//! Computer-keyboard fallback input: two octaves on the letter rows, by
//! physical key position (works the same on QWERTY and AZERTY).

use std::time::Instant;

use rtrb::Producer;
use winit::keyboard::KeyCode;

use super::state::LiveEvent;
use crate::audio::RawMidi;

/// Lowest-row C at octave shift 0 (C3).
const BASE_NOTE: i16 = 48;
/// Velocity of a normal key press; Shift plays at full velocity.
const NORMAL_VELOCITY: u8 = 96;
const LOUD_VELOCITY: u8 = 127;

/// Semitones above the base C, piano-style: white keys on the letter rows,
/// black keys on the row above.
fn semitone(code: KeyCode) -> Option<i16> {
    use KeyCode::*;
    Some(match code {
        KeyZ => 0,
        KeyS => 1,
        KeyX => 2,
        KeyD => 3,
        KeyC => 4,
        KeyV => 5,
        KeyG => 6,
        KeyB => 7,
        KeyH => 8,
        KeyN => 9,
        KeyJ => 10,
        KeyM => 11,
        Comma => 12,
        KeyL => 13,
        Period => 14,
        Semicolon => 15,
        Slash => 16,
        KeyQ => 12,
        Digit2 => 13,
        KeyW => 14,
        Digit3 => 15,
        KeyE => 16,
        KeyR => 17,
        Digit5 => 18,
        KeyT => 19,
        Digit6 => 20,
        KeyY => 21,
        Digit7 => 22,
        KeyU => 23,
        KeyI => 24,
        Digit9 => 25,
        KeyO => 26,
        Digit0 => 27,
        KeyP => 28,
        _ => return None,
    })
}

/// Sends typed notes down the same queues a MIDI device uses.
pub struct KeyboardInput {
    audio: Producer<RawMidi>,
    render: Producer<LiveEvent>,
    octave: i16,
    /// Keys currently down and the pitch they started (octave may change meanwhile).
    held: Vec<(KeyCode, u8)>,
}

impl KeyboardInput {
    /// Wraps the two live queues.
    pub fn new(audio: Producer<RawMidi>, render: Producer<LiveEvent>) -> Self {
        Self { audio, render, octave: 0, held: Vec::new() }
    }

    /// Handles a key; returns whether it was a note or octave key.
    pub fn key(&mut self, code: KeyCode, pressed: bool, repeat: bool, loud: bool) -> bool {
        match code {
            KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                if pressed && !repeat {
                    let step = if code == KeyCode::ArrowLeft { -1 } else { 1 };
                    self.octave = (self.octave + step).clamp(-3, 3);
                }
                return true;
            }
            _ => {}
        }
        let Some(offset) = semitone(code) else { return false };
        if repeat {
            return true;
        }
        if pressed {
            if self.held.iter().any(|(c, _)| *c == code) {
                return true;
            }
            // Clamped to the 88-key range, so the cast cannot truncate.
            let pitch = (BASE_NOTE + self.octave * 12 + offset).clamp(21, 108) as u8;
            self.send(0x90, pitch, if loud { LOUD_VELOCITY } else { NORMAL_VELOCITY });
            self.held.push((code, pitch));
        } else if let Some(i) = self.held.iter().position(|(c, _)| *c == code) {
            let (_, pitch) = self.held.swap_remove(i);
            self.send(0x80, pitch, 0);
        }
        true
    }

    /// Note-off for every held key.
    pub fn release_all(&mut self) {
        for (_, pitch) in std::mem::take(&mut self.held) {
            self.send(0x80, pitch, 0);
        }
    }

    fn send(&mut self, status: u8, data1: u8, data2: u8) {
        let raw = RawMidi { status, data1, data2 };
        // Queues are drained every buffer/frame; a full queue only drops this note.
        let _ = self.audio.push(raw);
        let _ = self.render.push(LiveEvent { raw, at: Instant::now() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;

    #[test]
    fn notes_octaves_and_release() {
        let (atx, mut arx) = RingBuffer::new(64);
        let (rtx, mut rrx) = RingBuffer::new(64);
        let mut kb = KeyboardInput::new(atx, rtx);

        assert!(kb.key(KeyCode::KeyZ, true, false, false));
        assert!(kb.key(KeyCode::KeyZ, true, true, false)); // OS auto-repeat ignored
        assert!(kb.key(KeyCode::ArrowRight, true, false, false));
        assert!(kb.key(KeyCode::KeyQ, true, false, true));
        assert!(kb.key(KeyCode::KeyZ, false, false, false)); // released at its original pitch
        assert!(!kb.key(KeyCode::F3, true, false, false));
        kb.release_all();

        let mut sent = Vec::new();
        while let Ok(m) = arx.pop() {
            sent.push((m.status, m.data1, m.data2));
        }
        assert_eq!(sent, vec![(0x90, 48, 96), (0x90, 72, 127), (0x80, 48, 0), (0x80, 72, 0)]);
        let mut rendered = 0;
        while rrx.pop().is_ok() {
            rendered += 1;
        }
        assert_eq!(rendered, 4);
    }
}
