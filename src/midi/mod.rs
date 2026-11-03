//! MIDI file model: tempo map (ticks → seconds), note pairing and the `Song`
//! consumed by the renderer (and, from phase 2, the audio sequencer).

mod parse;
mod tempo;

use std::sync::Arc;

pub use parse::load_song;
pub use tempo::TempoMap;

/// One sounding note, in absolute seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    /// Note-on time.
    pub start_s: f64,
    /// Note-off time (`>= start_s`). Not lengthened by the sustain pedal.
    pub end_s: f64,
    /// MIDI key number (21 = A0, 108 = C8).
    pub pitch: u8,
    /// Note-on velocity, 1..=127.
    pub velocity: u8,
    /// MIDI channel, 0..=15.
    pub channel: u8,
    /// Index into [`Song::tracks`].
    pub track: u16,
}

/// A displayable part of the song.
///
/// One per (file track, channel) pair that contains notes, so format-0 files
/// (all channels in one track) still split into separate, colorable parts.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackInfo {
    /// Track name from the file, or a generated one.
    pub name: String,
    /// MIDI channel of this part.
    pub channel: u8,
    /// Number of notes in this part.
    pub note_count: usize,
    /// Whether the part's notes are drawn.
    pub visible: bool,
    /// Whether the part's notes are silenced.
    pub muted: bool,
}

/// [`MidiEvent::track`] of events on a channel that has no notes in its file track.
pub const NO_PART: u16 = u16::MAX;

/// A raw channel message at an absolute time, as fed to the synthesizer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidiEvent {
    /// Absolute time in seconds.
    pub time_s: f64,
    /// Part index (see [`TrackInfo`]) or [`NO_PART`].
    pub track: u16,
    /// MIDI channel, 0..=15.
    pub channel: u8,
    /// Status high nibble: 0x80 note-off, 0x90 note-on, 0xB0 controller, ...
    pub status: u8,
    /// First data byte.
    pub data1: u8,
    /// Second data byte (0 if unused).
    pub data2: u8,
}

/// A time span during which the sustain pedal is down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PedalSpan {
    /// Pedal down.
    pub start_s: f64,
    /// Pedal up.
    pub end_s: f64,
}

/// A parsed MIDI file ready for playback.
#[derive(Debug, Clone, Default)]
pub struct Song {
    /// Display name (usually the file stem).
    pub title: String,
    /// All notes sorted by `start_s` (then pitch).
    pub notes: Vec<Note>,
    /// Parts referenced by [`Note::track`].
    pub tracks: Vec<TrackInfo>,
    /// Longest note duration, used to widen the culling window.
    pub max_note_len_s: f64,
    /// Time of the last event in the file.
    pub duration_s: f64,
    /// All channel messages sorted by time; shared with the audio thread.
    pub events: Arc<[MidiEvent]>,
    /// Sustain pedal spans (any channel), sorted, for the visual indicator.
    pub pedal: Vec<PedalSpan>,
}

impl Song {
    /// Notes that may overlap `[from_s, to_s]`: every note starting in
    /// `[from_s - max_note_len_s, to_s]`. Two binary searches, never a full scan.
    ///
    /// Callers still need to check `end_s >= from_s` for each returned note.
    pub fn notes_in_window(&self, from_s: f64, to_s: f64) -> &[Note] {
        let earliest = from_s - self.max_note_len_s;
        let lo = self.notes.partition_point(|n| n.start_s < earliest);
        let hi = self.notes.partition_point(|n| n.start_s <= to_s).max(lo);
        &self.notes[lo..hi]
    }

    /// Whether the sustain pedal is down at `time_s`.
    pub fn pedal_down_at(&self, time_s: f64) -> bool {
        let idx = self.pedal.partition_point(|p| p.start_s <= time_s);
        idx > 0 && time_s < self.pedal[idx - 1].end_s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start_s: f64, end_s: f64) -> Note {
        Note { start_s, end_s, pitch: 60, velocity: 100, channel: 0, track: 0 }
    }

    #[test]
    fn window_includes_long_notes_started_earlier() {
        let song = Song {
            notes: vec![note(0.0, 10.0), note(1.0, 1.5), note(5.0, 5.5), note(9.0, 9.5), note(20.0, 21.0)],
            max_note_len_s: 10.0,
            ..Default::default()
        };
        let window = song.notes_in_window(8.0, 12.0);
        let visible: Vec<f64> = window.iter().filter(|n| n.end_s >= 8.0).map(|n| n.start_s).collect();
        assert_eq!(visible, vec![0.0, 9.0]);
        assert!(window.iter().all(|n| n.start_s <= 12.0));
    }

    #[test]
    fn window_empty_song_and_inverted_range() {
        let song = Song::default();
        assert!(song.notes_in_window(0.0, 5.0).is_empty());
        let song = Song { notes: vec![note(1.0, 2.0)], max_note_len_s: 1.0, ..Default::default() };
        assert!(song.notes_in_window(5.0, 0.0).is_empty());
    }
}
