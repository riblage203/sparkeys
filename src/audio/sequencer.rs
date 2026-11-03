//! Audio-thread playback of a song's MIDI events into a synthesizer.
//!
//! Everything here runs inside the audio callback: no allocation, no locks,
//! no blocking. Replaced data leaves through the garbage callback so it is
//! freed on the main thread.

use std::sync::Arc;

use crate::midi::MidiEvent;

/// Frames rendered between event dispatches. Matches rustysynth's internal
/// block size, so events land exactly on synth block boundaries (1.3 ms at 48 kHz).
pub const SUB_BLOCK: usize = 64;

/// Something that turns MIDI messages into stereo samples.
pub trait Synth {
    /// Handles one channel message (`status` is the high nibble, e.g. 0x90).
    fn midi(&mut self, channel: u8, status: u8, data1: u8, data2: u8);
    /// Renders `left.len()` frames (both slices have the same length).
    fn render(&mut self, left: &mut [f32], right: &mut [f32]);
    /// Releases all notes; `immediate` cuts them without release tails.
    fn all_notes_off(&mut self, immediate: bool);
    /// Resets all channels (programs, controllers) and voices.
    fn reset(&mut self);
}

impl Synth for rustysynth::Synthesizer {
    fn midi(&mut self, channel: u8, status: u8, data1: u8, data2: u8) {
        self.process_midi_message(i32::from(channel), i32::from(status), i32::from(data1), i32::from(data2));
    }
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        rustysynth::Synthesizer::render(self, left, right);
    }
    fn all_notes_off(&mut self, immediate: bool) {
        self.note_off_all(immediate);
    }
    fn reset(&mut self) {
        rustysynth::Synthesizer::reset(self);
    }
}

/// Outputs silence; used until a SoundFont is loaded so the clock still runs.
pub struct Silence;

impl Synth for Silence {
    fn midi(&mut self, _: u8, _: u8, _: u8, _: u8) {}
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
    }
    fn all_notes_off(&mut self, _: bool) {}
    fn reset(&mut self) {}
}

/// A raw channel message from a live input (status byte includes the channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMidi {
    /// Full status byte, 0x80..=0xEF.
    pub status: u8,
    /// First data byte.
    pub data1: u8,
    /// Second data byte (0 if unused).
    pub data2: u8,
}

/// Commands from the main thread to the audio thread.
pub enum AudioCmd {
    /// Attach the queue a live MIDI input writes into (replaces any previous one).
    SetLiveInput(rtrb::Consumer<RawMidi>),
    /// Release every sounding note and lift the pedal (live device lost or mode change).
    AllNotesOff,
    /// Replace the song. Followed by a `Seek` from the main thread.
    LoadSong {
        /// All channel events, sorted by time.
        events: Arc<[MidiEvent]>,
        /// Mute flag per part (pre-sized so `SetMute` never allocates).
        muted: Vec<bool>,
        /// Song length.
        duration_s: f64,
    },
    /// Replace the synthesizer (SoundFont change).
    SetSynth(Box<rustysynth::Synthesizer>),
    /// Start advancing.
    Play,
    /// Stop advancing; sounding notes are released.
    Pause,
    /// Jump to a time; `serial` identifies the request in the published clock.
    Seek {
        /// Target song time.
        time_s: f64,
        /// Main-thread seek counter.
        serial: u64,
    },
    /// Playback speed multiplier.
    SetSpeed(f64),
    /// Loop between two song times (`None` = no loop).
    SetLoop(Option<(f64, f64)>),
    /// Output gain, 0..=1.
    SetVolume(f32),
    /// Mute or unmute one part.
    SetMute {
        /// Part index.
        part: u16,
        /// New state.
        muted: bool,
    },
}

/// Objects the audio thread hands back so they are dropped off the real-time thread.
#[expect(dead_code, reason = "payloads are never read, only dropped on the main thread")]
pub enum Garbage {
    /// A replaced event list.
    Events(Arc<[MidiEvent]>),
    /// A replaced mute table.
    Muted(Vec<bool>),
    /// A replaced synthesizer.
    Synth(Box<rustysynth::Synthesizer>),
    /// A replaced live input queue.
    LiveInput(rtrb::Consumer<RawMidi>),
}

/// Playback cursor over a song's events.
pub struct Sequencer {
    events: Arc<[MidiEvent]>,
    muted: Vec<bool>,
    duration_s: f64,
    cursor: usize,
    position_s: f64,
    playing: bool,
    ended: bool,
    speed: f64,
    seek_serial: u64,
    loop_range: Option<(f64, f64)>,
}

impl Sequencer {
    /// An empty, paused sequencer.
    pub fn new() -> Self {
        Self {
            events: Arc::from(Vec::new()),
            muted: Vec::new(),
            duration_s: 0.0,
            cursor: 0,
            position_s: 0.0,
            playing: false,
            ended: false,
            speed: 1.0,
            seek_serial: 0,
            loop_range: None,
        }
    }

    /// Song time at the start of the next rendered frame.
    pub fn position(&self) -> f64 {
        self.position_s
    }

    /// Whether time is advancing.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Whether playback stopped at the end of the song.
    pub fn has_ended(&self) -> bool {
        self.ended
    }

    /// Current speed multiplier.
    pub fn speed(&self) -> f64 {
        self.speed
    }

    /// Serial of the last applied seek.
    pub fn seek_serial(&self) -> u64 {
        self.seek_serial
    }

    /// Applies a song/transport command. `SetSynth` and `SetVolume` belong to
    /// the engine and are ignored here.
    pub fn apply(&mut self, cmd: AudioCmd, synth: &mut dyn Synth, garbage: &mut dyn FnMut(Garbage)) {
        match cmd {
            AudioCmd::LoadSong { events, muted, duration_s } => {
                garbage(Garbage::Events(std::mem::replace(&mut self.events, events)));
                garbage(Garbage::Muted(std::mem::replace(&mut self.muted, muted)));
                self.duration_s = duration_s;
                self.cursor = 0;
                self.position_s = 0.0;
                self.playing = false;
                self.ended = false;
                synth.all_notes_off(true);
                synth.reset();
            }
            AudioCmd::Play => {
                self.restore_pedals(synth);
                self.playing = true;
                self.ended = false;
            }
            AudioCmd::Pause => {
                self.playing = false;
                for channel in 0..16 {
                    synth.midi(channel, 0xB0, 64, 0);
                }
                synth.all_notes_off(false);
            }
            AudioCmd::Seek { time_s, serial } => self.seek(time_s, serial, synth),
            AudioCmd::SetSpeed(speed) => self.speed = speed,
            AudioCmd::SetLoop(range) => self.loop_range = range.filter(|(a, b)| b > a),
            AudioCmd::SetMute { part, muted } => {
                if let Some(slot) = self.muted.get_mut(usize::from(part)) {
                    *slot = muted;
                }
            }
            AudioCmd::AllNotesOff => {
                for channel in 0..16 {
                    synth.midi(channel, 0xB0, 64, 0);
                }
                synth.all_notes_off(false);
            }
            AudioCmd::SetSynth(_) | AudioCmd::SetVolume(_) | AudioCmd::SetLiveInput(_) => {}
        }
    }

    /// Jumps to `time_s` and rebuilds the synth's channel state (programs,
    /// controllers, pitch bend, pedal) as it would be at that point, without
    /// replaying notes.
    fn seek(&mut self, time_s: f64, serial: u64, synth: &mut dyn Synth) {
        self.position_s = time_s;
        self.seek_serial = serial;
        self.ended = false;
        self.cursor = self.events.partition_point(|e| e.time_s < time_s);

        synth.all_notes_off(true);
        synth.reset();
        let mut pedal = [0u8; 16];
        for e in &self.events[..self.cursor] {
            match e.status {
                0xB0 if e.data1 == 64 => pedal[usize::from(e.channel)] = e.data2,
                0xB0 | 0xC0 | 0xE0 => synth.midi(e.channel, e.status, e.data1, e.data2),
                _ => {}
            }
        }
        for (channel, &value) in (0u8..).zip(pedal.iter()) {
            if value >= 64 {
                synth.midi(channel, 0xB0, 64, value);
            }
        }
    }

    /// Rebuilds a freshly swapped synth's channel state at the current position.
    pub fn resync(&mut self, synth: &mut dyn Synth) {
        self.seek(self.position_s, self.seek_serial, synth);
    }

    /// Re-applies the sustain pedal state at the cursor (it is lifted on pause).
    fn restore_pedals(&self, synth: &mut dyn Synth) {
        let mut found = [false; 16];
        for e in self.events[..self.cursor].iter().rev() {
            if e.status == 0xB0 && e.data1 == 64 && !found[usize::from(e.channel)] {
                found[usize::from(e.channel)] = true;
                synth.midi(e.channel, 0xB0, 64, e.data2);
                if found.iter().all(|&f| f) {
                    break;
                }
            }
        }
    }

    /// Renders `left.len()` frames, dispatching due events every [`SUB_BLOCK`] frames.
    pub fn render(&mut self, synth: &mut dyn Synth, left: &mut [f32], right: &mut [f32], sample_rate: f64) {
        let len = left.len().min(right.len());
        let mut offset = 0;
        while offset < len {
            let n = SUB_BLOCK.min(len - offset);
            if self.playing {
                let mut end = self.position_s + n as f64 / sample_rate * self.speed;
                // Crossing the loop end: play up to B, then continue from A.
                if let Some((a, b)) = self.loop_range
                    && self.position_s <= b
                    && end >= b
                {
                    self.dispatch_before(b, synth);
                    self.wrap_to(a, synth);
                    end = a + (end - b);
                }
                self.dispatch_before(end, synth);
                self.position_s = end;
                if end >= self.duration_s {
                    self.position_s = self.duration_s;
                    self.playing = false;
                    self.ended = true;
                    synth.all_notes_off(false);
                }
            }
            synth.render(&mut left[offset..offset + n], &mut right[offset..offset + n]);
            offset += n;
        }
    }

    /// Loop jump: release what is sounding (with natural tails) and continue at `time_s`.
    fn wrap_to(&mut self, time_s: f64, synth: &mut dyn Synth) {
        for channel in 0..16 {
            synth.midi(channel, 0xB0, 64, 0);
        }
        synth.all_notes_off(false);
        self.position_s = time_s;
        self.cursor = self.events.partition_point(|e| e.time_s < time_s);
        self.restore_pedals(synth);
    }

    fn dispatch_before(&mut self, end_s: f64, synth: &mut dyn Synth) {
        while let Some(e) = self.events.get(self.cursor) {
            if e.time_s >= end_s {
                break;
            }
            self.cursor += 1;
            let is_note_on = e.status == 0x90 && e.data2 > 0;
            if is_note_on && self.muted.get(usize::from(e.track)).copied().unwrap_or(false) {
                continue;
            }
            synth.midi(e.channel, e.status, e.data1, e.data2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    /// Records every message with the frame index at which it arrived.
    #[derive(Default)]
    struct Recorder {
        frame: usize,
        log: Vec<(usize, u8, u8, u8, u8)>,
        resets: usize,
    }

    impl Synth for Recorder {
        fn midi(&mut self, channel: u8, status: u8, data1: u8, data2: u8) {
            self.log.push((self.frame, channel, status, data1, data2));
        }
        fn render(&mut self, left: &mut [f32], _right: &mut [f32]) {
            self.frame += left.len();
        }
        fn all_notes_off(&mut self, _: bool) {}
        fn reset(&mut self) {
            self.resets += 1;
        }
    }

    fn ev(time_s: f64, track: u16, channel: u8, status: u8, data1: u8, data2: u8) -> MidiEvent {
        MidiEvent { time_s, track, channel, status, data1, data2 }
    }

    fn loaded(events: Vec<MidiEvent>, parts: usize, duration_s: f64, synth: &mut Recorder) -> Sequencer {
        let mut seq = Sequencer::new();
        let cmd = AudioCmd::LoadSong { events: events.into(), muted: vec![false; parts], duration_s };
        seq.apply(cmd, synth, &mut |_| {});
        seq.apply(AudioCmd::Seek { time_s: 0.0, serial: 1 }, synth, &mut |_| {});
        synth.log.clear();
        seq
    }

    fn run(seq: &mut Sequencer, synth: &mut Recorder, frames: usize) {
        let mut l = vec![0.0; 300];
        let mut r = vec![0.0; 300];
        let mut done = 0;
        while done < frames {
            // Odd buffer sizes, like a real device might use.
            let n = 300.min(frames - done);
            seq.render(synth, &mut l[..n], &mut r[..n], SR);
            done += n;
        }
    }

    #[test]
    fn events_land_on_time_at_every_speed() {
        for speed in [0.5, 1.0, 2.0] {
            let mut synth = Recorder::default();
            let mut seq = loaded(vec![ev(1.0, 0, 0, 0x90, 60, 100)], 1, 10.0, &mut synth);
            seq.apply(AudioCmd::SetSpeed(speed), &mut synth, &mut |_| {});
            seq.apply(AudioCmd::Play, &mut synth, &mut |_| {});
            let frames = (3.0 * SR) as usize;
            run(&mut seq, &mut synth, frames);

            // Song position tracks rendered samples exactly.
            assert!((seq.position() - frames as f64 / SR * speed).abs() < 1e-9, "speed {speed}");
            // The note-on arrives within one sub-block of its ideal sample.
            let ideal = 1.0 / speed * SR;
            let (frame, ..) = synth.log.iter().find(|m| m.2 == 0x90).copied().unwrap();
            let error = (frame as f64 - ideal).abs();
            assert!(error <= SUB_BLOCK as f64, "speed {speed}: frame {frame} vs {ideal}");
        }
    }

    #[test]
    fn loop_replays_the_section_and_never_ends() {
        let events = vec![ev(0.5, 0, 0, 0x90, 60, 100), ev(1.5, 0, 0, 0x90, 62, 100)];
        let mut synth = Recorder::default();
        let mut seq = loaded(events, 1, 3.0, &mut synth);
        seq.apply(AudioCmd::SetLoop(Some((0.25, 1.0))), &mut synth, &mut |_| {});
        seq.apply(AudioCmd::Play, &mut synth, &mut |_| {});
        run(&mut seq, &mut synth, (4.0 * SR) as usize);

        let note_ons: Vec<u8> = synth.log.iter().filter(|m| m.2 == 0x90).map(|m| m.3).collect();
        // 4 s of a 0.75 s loop starting at 0: the note at 0.5 s plays 5 times, the one past B never.
        assert_eq!(note_ons, vec![60; 5]);
        assert!(seq.is_playing() && !seq.has_ended());
        assert!((0.25..1.0).contains(&seq.position()));
    }

    #[test]
    fn stops_and_flags_end() {
        let mut synth = Recorder::default();
        let mut seq = loaded(vec![ev(0.1, 0, 0, 0x90, 60, 100)], 1, 0.5, &mut synth);
        seq.apply(AudioCmd::Play, &mut synth, &mut |_| {});
        run(&mut seq, &mut synth, SR as usize);
        assert!(!seq.is_playing() && seq.has_ended());
        assert_eq!(seq.position(), 0.5);
    }

    #[test]
    fn seek_restores_controllers_and_pedal_but_not_notes() {
        let events = vec![
            ev(0.0, 0, 0, 0xC0, 5, 0),
            ev(0.1, 0, 0, 0xB0, 64, 127),
            ev(0.2, 0, 0, 0x90, 60, 100),
            ev(0.3, 0, 0, 0xB0, 7, 90),
            ev(0.4, 0, 0, 0x80, 60, 0),
            ev(2.0, 0, 0, 0x90, 62, 100),
        ];
        let mut synth = Recorder::default();
        let mut seq = loaded(events, 1, 5.0, &mut synth);
        let resets = synth.resets;
        seq.apply(AudioCmd::Seek { time_s: 1.0, serial: 2 }, &mut synth, &mut |_| {});
        assert_eq!(synth.resets, resets + 1);
        let sent: Vec<(u8, u8, u8)> = synth.log.iter().map(|m| (m.2, m.3, m.4)).collect();
        assert_eq!(sent, vec![(0xC0, 5, 0), (0xB0, 7, 90), (0xB0, 64, 127)]);
        assert_eq!(seq.seek_serial(), 2);
    }

    #[test]
    fn muted_parts_skip_note_ons_only() {
        let events = vec![ev(0.0, 0, 0, 0x90, 60, 100), ev(0.0, 1, 1, 0x90, 64, 100), ev(0.01, 1, 1, 0x80, 64, 0)];
        let mut synth = Recorder::default();
        let mut seq = loaded(events, 2, 1.0, &mut synth);
        seq.apply(AudioCmd::SetMute { part: 1, muted: true }, &mut synth, &mut |_| {});
        seq.apply(AudioCmd::Play, &mut synth, &mut |_| {});
        run(&mut seq, &mut synth, 4800);
        let notes: Vec<(u8, u8)> = synth.log.iter().filter(|m| m.2 != 0xB0).map(|m| (m.2, m.3)).collect();
        assert_eq!(notes, vec![(0x90, 60), (0x80, 64)]);
    }

    #[test]
    fn load_returns_old_data_as_garbage() {
        let mut synth = Recorder::default();
        let mut seq = Sequencer::new();
        let mut collected = 0;
        let cmd = AudioCmd::LoadSong { events: Arc::from(Vec::new()), muted: vec![], duration_s: 1.0 };
        seq.apply(cmd, &mut synth, &mut |_| collected += 1);
        assert_eq!(collected, 2);
    }
}
