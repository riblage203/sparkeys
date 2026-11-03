//! What the renderer needs to know about live playing: held and recently
//! released notes, the sustain pedal and this frame's new note-ons.

use std::collections::VecDeque;
use std::time::Instant;

use crate::audio::RawMidi;

/// Upper bound on remembered notes so memory stays bounded however long one plays.
const MAX_NOTES: usize = 4096;

/// A message from the input device with its arrival time.
#[derive(Debug, Clone, Copy)]
pub struct LiveEvent {
    /// The channel message.
    pub raw: RawMidi,
    /// When it was received.
    pub at: Instant,
}

/// A played note in live time (seconds since [`LiveState`] creation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveNote {
    /// MIDI key.
    pub pitch: u8,
    /// Note-on velocity, 1..=127.
    pub velocity: u8,
    /// Key down.
    pub start_s: f64,
    /// Key up, `None` while held.
    pub end_s: Option<f64>,
}

/// A note-on that arrived since the last frame (drives particle bursts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveHit {
    /// MIDI key.
    pub pitch: u8,
    /// Velocity, 1..=127.
    pub velocity: u8,
}

/// Live notes, oldest first.
pub struct LiveState {
    origin: Instant,
    notes: VecDeque<LiveNote>,
    hits: Vec<LiveHit>,
    pedal: [bool; 16],
}

impl LiveState {
    /// Empty state; live time starts now.
    pub fn new() -> Self {
        Self { origin: Instant::now(), notes: VecDeque::new(), hits: Vec::new(), pedal: [false; 16] }
    }

    /// Current live time in seconds.
    pub fn now_s(&self) -> f64 {
        self.time_of(Instant::now())
    }

    fn time_of(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.origin).as_secs_f64()
    }

    /// Applies one input message.
    pub fn apply(&mut self, event: &LiveEvent) {
        let t = self.time_of(event.at);
        let RawMidi { status, data1, data2 } = event.raw;
        match status & 0xF0 {
            0x90 if data2 > 0 => {
                // A re-strike without a note-off ends the previous bar.
                self.close(data1, t);
                self.notes.push_back(LiveNote { pitch: data1, velocity: data2, start_s: t, end_s: None });
                self.hits.push(LiveHit { pitch: data1, velocity: data2 });
                while self.notes.len() > MAX_NOTES {
                    self.notes.pop_front();
                }
            }
            0x80 | 0x90 => self.close(data1, t),
            0xB0 if data1 == 64 => self.pedal[usize::from(status & 0x0F)] = data2 >= 64,
            _ => {}
        }
    }

    fn close(&mut self, pitch: u8, t: f64) {
        if let Some(note) = self.notes.iter_mut().rev().find(|n| n.pitch == pitch && n.end_s.is_none()) {
            note.end_s = Some(t.max(note.start_s));
        }
    }

    /// Releases everything (device lost, mode switched).
    pub fn release_all(&mut self) {
        let t = self.now_s();
        for note in self.notes.iter_mut().filter(|n| n.end_s.is_none()) {
            note.end_s = Some(t.max(note.start_s));
        }
        self.pedal = [false; 16];
    }

    /// Forgets notes released more than `keep_s` seconds ago (scrolled off screen).
    pub fn prune(&mut self, keep_s: f64) {
        let now = self.now_s();
        while self.notes.front().is_some_and(|n| n.end_s.is_some_and(|end| now - end > keep_s)) {
            self.notes.pop_front();
        }
    }

    /// Notes, oldest first.
    pub fn notes(&self) -> &VecDeque<LiveNote> {
        &self.notes
    }

    /// Note-ons since the last [`Self::clear_hits`].
    pub fn hits(&self) -> &[LiveHit] {
        &self.hits
    }

    /// Call after the frame has consumed [`Self::hits`].
    pub fn clear_hits(&mut self) {
        self.hits.clear();
    }

    /// Whether the sustain pedal is down on any channel.
    pub fn pedal_down(&self) -> bool {
        self.pedal.iter().any(|&p| p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ev(state: &LiveState, ms: u64, status: u8, data1: u8, data2: u8) -> LiveEvent {
        LiveEvent { raw: RawMidi { status, data1, data2 }, at: state.origin + Duration::from_millis(ms) }
    }

    #[test]
    fn pairs_on_off_and_velocity_zero() {
        let mut s = LiveState::new();
        s.apply(&ev(&s, 100, 0x90, 60, 90));
        s.apply(&ev(&s, 150, 0x91, 64, 70));
        s.apply(&ev(&s, 300, 0x80, 60, 0));
        s.apply(&ev(&s, 400, 0x91, 64, 0));
        let spans: Vec<(u8, f64, Option<f64>)> = s.notes().iter().map(|n| (n.pitch, n.start_s, n.end_s)).collect();
        assert_eq!(spans, vec![(60, 0.1, Some(0.3)), (64, 0.15, Some(0.4))]);
        assert_eq!(s.hits(), &[LiveHit { pitch: 60, velocity: 90 }, LiveHit { pitch: 64, velocity: 70 }]);
        s.clear_hits();
        assert!(s.hits().is_empty());
    }

    #[test]
    fn restrike_closes_previous_and_pedal_tracks_channels() {
        let mut s = LiveState::new();
        s.apply(&ev(&s, 0, 0x90, 60, 90));
        s.apply(&ev(&s, 200, 0x90, 60, 100));
        assert_eq!(s.notes()[0].end_s, Some(0.2));
        assert_eq!(s.notes()[1].end_s, None);

        s.apply(&ev(&s, 0, 0xB3, 64, 127));
        assert!(s.pedal_down());
        s.apply(&ev(&s, 0, 0xB3, 64, 0));
        assert!(!s.pedal_down());
    }

    #[test]
    fn release_all_and_prune() {
        let mut s = LiveState::new();
        s.apply(&ev(&s, 0, 0x90, 60, 90));
        s.apply(&ev(&s, 0, 0xB0, 64, 127));
        s.release_all();
        assert!(s.notes()[0].end_s.is_some() && !s.pedal_down());
        s.prune(-1.0); // anything released "more than -1 s ago" goes
        assert!(s.notes().is_empty());
    }

    #[test]
    fn memory_is_bounded() {
        let mut s = LiveState::new();
        for i in 0..(MAX_NOTES + 10) {
            s.apply(&ev(&s, i as u64, 0x90, (i % 88) as u8 + 21, 80));
        }
        assert_eq!(s.notes().len(), MAX_NOTES);
    }
}
