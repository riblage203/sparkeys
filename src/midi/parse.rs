//! Standard MIDI File → [`Song`].

use std::collections::VecDeque;
use std::path::Path;

use midly::{MetaMessage, MidiMessage, Smf, TrackEventKind};

use super::{MidiEvent, NO_PART, Note, PedalSpan, Song, TempoMap, TrackInfo};

/// Why a MIDI file could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    /// The file could not be read.
    #[error("could not read file: {0}")]
    Io(#[from] std::io::Error),
    /// The bytes are not a valid Standard MIDI File.
    #[error("not a valid MIDI file: {0}")]
    Parse(#[from] midly::Error),
    /// The file parsed but contains no notes.
    #[error("the file contains no notes")]
    NoNotes,
}

/// Reads and parses a `.mid` file; the title is the file stem.
pub fn load_song(path: &Path) -> Result<Song, MidiError> {
    let bytes = std::fs::read(path)?;
    let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    parse_song(&bytes, title)
}

/// Parses an in-memory Standard MIDI File.
///
/// Pairing rules:
/// - a note-on with velocity 0 is a note-off;
/// - overlapping notes on the same (track, channel, key) are closed first-in
///   first-out, so each note-off ends the oldest still-open note;
/// - notes still open at the end of their track end there.
///
/// Every channel message is also kept, in time order, in [`Song::events`] for
/// the audio sequencer; sustain pedal (CC64) spans go to [`Song::pedal`].
pub fn parse_song(bytes: &[u8], title: String) -> Result<Song, MidiError> {
    let smf = Smf::parse(bytes)?;
    let tempo = TempoMap::from_smf(&smf);

    let mut song = Song { title, ..Default::default() };
    // Open note-ons per (channel, key): (start tick, velocity). Reused across tracks.
    let mut open: Vec<VecDeque<(u64, u8)>> = vec![VecDeque::new(); 16 * 128];
    let mut track_notes: Vec<Note> = Vec::new();
    let mut track_events: Vec<MidiEvent> = Vec::new();
    let mut events: Vec<MidiEvent> = Vec::new();

    for (track_idx, track) in smf.tracks.iter().enumerate() {
        let mut tick = 0u64;
        let mut name: Option<String> = None;
        track_notes.clear();
        track_events.clear();

        let close = |channel: u8, key: u8, start_tick: u64, end_tick: u64, velocity: u8, out: &mut Vec<Note>| {
            out.push(Note {
                start_s: tempo.tick_to_sec(start_tick),
                end_s: tempo.tick_to_sec(end_tick),
                pitch: key,
                velocity,
                channel,
                // Temporarily the channel; remapped to a part index below.
                track: u16::from(channel),
            });
        };

        for event in track {
            tick += u64::from(event.delta.as_int());
            match event.kind {
                TrackEventKind::Midi { channel, message } => {
                    let channel = channel.as_int();
                    let (status, data1, data2) = encode(message);
                    track_events.push(MidiEvent {
                        time_s: tempo.tick_to_sec(tick),
                        track: u16::from(channel),
                        channel,
                        status,
                        data1,
                        data2,
                    });
                    let (key, on_velocity) = match message {
                        MidiMessage::NoteOn { key, vel } => (key.as_int(), vel.as_int()),
                        MidiMessage::NoteOff { key, .. } => (key.as_int(), 0),
                        _ => continue,
                    };
                    let slot = &mut open[usize::from(channel) * 128 + usize::from(key)];
                    if on_velocity > 0 {
                        slot.push_back((tick, on_velocity));
                    } else if let Some((start, velocity)) = slot.pop_front() {
                        close(channel, key, start, tick, velocity, &mut track_notes);
                    }
                }
                TrackEventKind::Meta(MetaMessage::TrackName(raw)) if name.is_none() => {
                    let text = String::from_utf8_lossy(raw).trim().to_owned();
                    if !text.is_empty() {
                        name = Some(text);
                    }
                }
                _ => {}
            }
        }

        // Close dangling notes at the track end.
        for (idx, slot) in open.iter_mut().enumerate() {
            for (start, velocity) in slot.drain(..) {
                // idx < 16 * 128, so both casts fit in u8.
                close((idx / 128) as u8, (idx % 128) as u8, start, tick, velocity, &mut track_notes);
            }
        }
        song.duration_s = song.duration_s.max(tempo.tick_to_sec(tick));

        // One part per channel used by this track.
        let mut channels: Vec<u8> = track_notes.iter().map(|n| n.channel).collect();
        channels.sort_unstable();
        channels.dedup();
        let base = name.unwrap_or_else(|| format!("Track {}", track_idx + 1));
        let mut part_of_channel = [NO_PART; 16];
        for &channel in &channels {
            let part = u16::try_from(song.tracks.len()).unwrap_or(u16::MAX);
            let name = if channels.len() > 1 { format!("{base} · ch {}", channel + 1) } else { base.clone() };
            let mut count = 0;
            for note in track_notes.iter_mut().filter(|n| n.channel == channel) {
                note.track = part;
                count += 1;
            }
            part_of_channel[usize::from(channel)] = part;
            song.tracks.push(TrackInfo { name, channel, note_count: count, visible: true, muted: false });
        }
        for event in &mut track_events {
            event.track = part_of_channel[usize::from(event.channel)];
        }
        song.notes.extend_from_slice(&track_notes);
        events.extend_from_slice(&track_events);
    }

    if song.notes.is_empty() {
        return Err(MidiError::NoNotes);
    }
    song.notes.sort_by(|a, b| a.start_s.total_cmp(&b.start_s).then(a.pitch.cmp(&b.pitch)));
    song.max_note_len_s = song.notes.iter().map(|n| n.end_s - n.start_s).fold(0.0, f64::max);
    song.duration_s = song.notes.iter().map(|n| n.end_s).fold(song.duration_s, f64::max);
    // Stable: simultaneous events keep their file order (e.g. program change before note-on).
    events.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    song.pedal = pedal_spans(&events, song.duration_s);
    song.events = events.into();
    Ok(song)
}

/// Raw (status high nibble, data1, data2) bytes of a channel message.
fn encode(message: MidiMessage) -> (u8, u8, u8) {
    match message {
        MidiMessage::NoteOff { key, vel } => (0x80, key.as_int(), vel.as_int()),
        MidiMessage::NoteOn { key, vel } => (0x90, key.as_int(), vel.as_int()),
        MidiMessage::Aftertouch { key, vel } => (0xA0, key.as_int(), vel.as_int()),
        MidiMessage::Controller { controller, value } => (0xB0, controller.as_int(), value.as_int()),
        MidiMessage::ProgramChange { program } => (0xC0, program.as_int(), 0),
        MidiMessage::ChannelAftertouch { vel } => (0xD0, vel.as_int(), 0),
        MidiMessage::PitchBend { bend } => {
            let raw = bend.0.as_int();
            // 14-bit value split into 7-bit LSB / MSB, both < 128.
            (0xE0, (raw & 0x7F) as u8, (raw >> 7) as u8)
        }
    }
}

/// Spans where the sustain pedal is down on at least one channel.
fn pedal_spans(events: &[MidiEvent], end_s: f64) -> Vec<PedalSpan> {
    let mut down = [false; 16];
    let mut spans = Vec::new();
    let mut open: Option<f64> = None;
    for e in events.iter().filter(|e| e.status == 0xB0 && e.data1 == 64) {
        down[usize::from(e.channel)] = e.data2 >= 64;
        let any = down.iter().any(|&d| d);
        match (open, any) {
            (None, true) => open = Some(e.time_s),
            (Some(start), false) => {
                spans.push(PedalSpan { start_s: start, end_s: e.time_s });
                open = None;
            }
            _ => {}
        }
    }
    if let Some(start) = open {
        spans.push(PedalSpan { start_s: start, end_s: end_s.max(start) });
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use midly::num::{u4, u7, u15, u24, u28};
    use midly::{Format, Header, Timing, TrackEvent};

    /// (delta ticks, event) shorthand for building test files.
    enum Ev {
        On(u8, u8, u8),
        Off(u8, u8),
        Tempo(u32),
        Name(&'static str),
        Cc(u8, u8, u8),
    }

    fn smf_bytes(ppq: u16, tracks: &[&[(u32, Ev)]]) -> Vec<u8> {
        let header = Header::new(
            if tracks.len() == 1 { Format::SingleTrack } else { Format::Parallel },
            Timing::Metrical(u15::new(ppq)),
        );
        let mut smf = Smf::new(header);
        for track in tracks {
            let mut events: Vec<TrackEvent<'static>> = track
                .iter()
                .map(|(delta, ev)| {
                    let kind = match *ev {
                        Ev::On(ch, key, vel) => TrackEventKind::Midi {
                            channel: u4::new(ch),
                            message: MidiMessage::NoteOn { key: u7::new(key), vel: u7::new(vel) },
                        },
                        Ev::Off(ch, key) => TrackEventKind::Midi {
                            channel: u4::new(ch),
                            message: MidiMessage::NoteOff { key: u7::new(key), vel: u7::new(64) },
                        },
                        Ev::Tempo(us) => TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us))),
                        Ev::Name(name) => TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())),
                        Ev::Cc(ch, controller, value) => TrackEventKind::Midi {
                            channel: u4::new(ch),
                            message: MidiMessage::Controller { controller: u7::new(controller), value: u7::new(value) },
                        },
                    };
                    TrackEvent { delta: u28::new(*delta), kind }
                })
                .collect();
            events.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
            smf.tracks.push(events);
        }
        let mut out = Vec::new();
        smf.write_std(&mut out).unwrap();
        out
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn tempo_track_applies_to_other_tracks() {
        let bytes = smf_bytes(
            480,
            &[
                &[(0, Ev::Tempo(500_000)), (960, Ev::Tempo(1_000_000))],
                &[
                    (0, Ev::Name("Piano")),
                    (0, Ev::On(0, 60, 100)),
                    (960, Ev::Off(0, 60)),
                    (0, Ev::On(0, 62, 80)),
                    (480, Ev::Off(0, 62)),
                ],
            ],
        );
        let song = parse_song(&bytes, "t".into()).unwrap();
        assert_eq!(song.notes.len(), 2);
        let (a, b) = (song.notes[0], song.notes[1]);
        assert!(close(a.start_s, 0.0) && close(a.end_s, 1.0));
        // Second note is at 60 BPM: 480 ticks = 1 s.
        assert!(close(b.start_s, 1.0) && close(b.end_s, 2.0));
        assert_eq!(b.velocity, 80);
        assert_eq!(song.tracks.len(), 1);
        assert_eq!(song.tracks[0].name, "Piano");
        assert!(close(song.max_note_len_s, 1.0));
    }

    #[test]
    fn velocity_zero_is_note_off() {
        let bytes = smf_bytes(480, &[&[(0, Ev::On(0, 64, 90)), (240, Ev::On(0, 64, 0))]]);
        let song = parse_song(&bytes, String::new()).unwrap();
        assert_eq!(song.notes.len(), 1);
        assert!(close(song.notes[0].end_s, 0.25));
        assert_eq!(song.notes[0].velocity, 90);
    }

    #[test]
    fn overlapping_same_pitch_is_fifo() {
        let bytes = smf_bytes(
            480,
            &[&[(0, Ev::On(0, 60, 100)), (480, Ev::On(0, 60, 50)), (480, Ev::Off(0, 60)), (480, Ev::Off(0, 60))]],
        );
        let song = parse_song(&bytes, String::new()).unwrap();
        let spans: Vec<(f64, f64, u8)> = song.notes.iter().map(|n| (n.start_s, n.end_s, n.velocity)).collect();
        assert_eq!(spans, vec![(0.0, 1.0, 100), (0.5, 1.5, 50)]);
    }

    #[test]
    fn dangling_notes_end_at_track_end() {
        let bytes = smf_bytes(480, &[&[(0, Ev::On(0, 60, 100)), (960, Ev::On(1, 72, 100))]]);
        let song = parse_song(&bytes, String::new()).unwrap();
        assert_eq!(song.notes.len(), 2);
        assert!(song.notes.iter().all(|n| close(n.end_s, 1.0)));
        assert!(close(song.duration_s, 1.0));
    }

    #[test]
    fn channels_in_one_track_become_separate_parts() {
        let bytes = smf_bytes(
            480,
            &[&[
                (0, Ev::Name("Mix")),
                (0, Ev::On(0, 40, 100)),
                (0, Ev::On(3, 80, 100)),
                (480, Ev::Off(0, 40)),
                (0, Ev::Off(3, 80)),
            ]],
        );
        let song = parse_song(&bytes, String::new()).unwrap();
        let names: Vec<&str> = song.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["Mix · ch 1", "Mix · ch 4"]);
        let low = song.notes.iter().find(|n| n.pitch == 40).unwrap();
        let high = song.notes.iter().find(|n| n.pitch == 80).unwrap();
        assert_eq!((low.track, high.track), (0, 1));
    }

    #[test]
    fn notes_sorted_by_start_across_tracks() {
        let bytes = smf_bytes(
            480,
            &[&[(480, Ev::On(0, 60, 100)), (480, Ev::Off(0, 60))], &[(0, Ev::On(1, 50, 100)), (1440, Ev::Off(1, 50))]],
        );
        let song = parse_song(&bytes, String::new()).unwrap();
        assert!(song.notes.windows(2).all(|w| w[0].start_s <= w[1].start_s));
        assert_eq!(song.notes[0].pitch, 50);
        assert_eq!(song.notes[0].track, 1);
    }

    #[test]
    fn events_are_timed_sorted_and_tagged_with_parts() {
        let bytes = smf_bytes(
            480,
            &[
                &[(0, Ev::Tempo(1_000_000))],
                &[(0, Ev::Cc(2, 7, 100)), (0, Ev::On(2, 60, 90)), (480, Ev::Off(2, 60))],
                &[(240, Ev::Cc(5, 10, 64))],
            ],
        );
        let song = parse_song(&bytes, String::new()).unwrap();
        let summary: Vec<(f64, u8, u8, u16)> =
            song.events.iter().map(|e| (e.time_s, e.status, e.channel, e.track)).collect();
        // Channel 5 has no notes, so its controller event belongs to no part.
        assert_eq!(summary, vec![(0.0, 0xB0, 2, 0), (0.0, 0x90, 2, 0), (0.5, 0xB0, 5, NO_PART), (1.0, 0x80, 2, 0)]);
    }

    #[test]
    fn pedal_spans_merge_channels() {
        let bytes = smf_bytes(
            480,
            &[&[
                (0, Ev::On(0, 60, 90)),
                (0, Ev::Cc(0, 64, 127)),
                (480, Ev::Cc(1, 64, 127)),
                (480, Ev::Cc(0, 64, 0)),
                (480, Ev::Cc(1, 64, 0)),
                (480, Ev::Cc(0, 64, 100)),
                (480, Ev::Off(0, 60)),
            ]],
        );
        let song = parse_song(&bytes, String::new()).unwrap();
        let spans: Vec<(f64, f64)> = song.pedal.iter().map(|p| (p.start_s, p.end_s)).collect();
        // Down 0..1.5 (two overlapping channels), down again from 2.0 until the end (2.5).
        assert_eq!(spans, vec![(0.0, 1.5), (2.0, 2.5)]);
        assert!(song.pedal_down_at(0.7) && !song.pedal_down_at(1.7) && song.pedal_down_at(2.2));
    }

    #[test]
    fn rejects_garbage_and_empty_files() {
        assert!(matches!(parse_song(b"not midi", String::new()), Err(MidiError::Parse(_))));
        let bytes = smf_bytes(480, &[&[(0, Ev::Tempo(400_000))]]);
        assert!(matches!(parse_song(&bytes, String::new()), Err(MidiError::NoNotes)));
    }
}
