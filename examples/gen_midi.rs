//! Generates test MIDI files so the visualizer can be tried without
//! downloading anything.
//!
//! ```text
//! cargo run --example gen_midi -- demo   demo.mid          # two hands, tempo changes, ~2 min
//! cargo run --example gen_midi -- stress stress.mid 60000  # dense random notes for profiling
//! ```

use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

const PPQ: u32 = 480;

/// Absolute-tick event list that is turned into delta-time track events.
#[derive(Default)]
struct TrackBuilder {
    events: Vec<(u32, TrackEventKind<'static>)>,
}

impl TrackBuilder {
    fn note(&mut self, channel: u8, key: u8, vel: u8, start: u32, len: u32) {
        let channel = u4::new(channel);
        let key = u7::new(key.min(127));
        self.events.push((
            start,
            TrackEventKind::Midi { channel, message: MidiMessage::NoteOn { key, vel: u7::new(vel.clamp(1, 127)) } },
        ));
        // Velocity-0 note-on as note-off, like many real files.
        self.events.push((
            start + len.max(1),
            TrackEventKind::Midi { channel, message: MidiMessage::NoteOn { key, vel: u7::new(0) } },
        ));
    }

    fn sustain(&mut self, channel: u8, tick: u32, down: bool) {
        let message = MidiMessage::Controller { controller: u7::new(64), value: u7::new(if down { 127 } else { 0 }) };
        self.events.push((tick, TrackEventKind::Midi { channel: u4::new(channel), message }));
    }

    fn meta(&mut self, tick: u32, meta: MetaMessage<'static>) {
        self.events.push((tick, TrackEventKind::Meta(meta)));
    }

    fn build(mut self) -> Vec<TrackEvent<'static>> {
        // Note-offs (vel 0) before note-ons at the same tick so repeated notes re-trigger cleanly.
        self.events.sort_by_key(|(tick, kind)| {
            let is_on = matches!(kind, TrackEventKind::Midi { message: MidiMessage::NoteOn { vel, .. }, .. } if vel.as_int() > 0);
            (*tick, is_on)
        });
        let mut last = 0;
        let mut out: Vec<TrackEvent<'static>> = self
            .events
            .into_iter()
            .map(|(tick, kind)| {
                let delta = tick - last;
                last = tick;
                TrackEvent { delta: u28::new(delta), kind }
            })
            .collect();
        out.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
        out
    }
}

/// Tiny deterministic PRNG (no extra dependency needed for test data).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + self.next() % (hi - lo).max(1)
    }
}

fn demo() -> Vec<Vec<TrackEvent<'static>>> {
    let beat = PPQ;
    let bar = beat * 4;
    let mut tempo = TrackBuilder::default();
    tempo.meta(0, MetaMessage::TrackName(b"Tempo"));
    tempo.meta(0, MetaMessage::Tempo(u24::new(600_000))); // 100 BPM
    tempo.meta(bar * 16, MetaMessage::Tempo(u24::new(461_538))); // 130 BPM
    tempo.meta(bar * 32, MetaMessage::Tempo(u24::new(750_000))); // 80 BPM

    let mut right = TrackBuilder::default();
    let mut left = TrackBuilder::default();
    right.meta(0, MetaMessage::TrackName(b"Right hand"));
    left.meta(0, MetaMessage::TrackName(b"Left hand"));

    // i–VI–III–VII in A minor, as root notes.
    let roots = [57u8, 53, 48, 55];
    let chord = |root: u8, minor: bool| [root, root + if minor { 3 } else { 4 }, root + 7];
    let mut rng = Lcg(7);

    for b in 0..44u32 {
        let start = b * bar;
        let root = roots[(b % 4) as usize];
        let minor = b % 4 == 0;
        let tones = chord(root, minor);

        // Left hand: low octave + broken chord eighths.
        left.sustain(0, start, true);
        left.sustain(0, start + bar - 10, false);
        left.note(0, root - 24, 90, start, bar - beat / 4);
        for i in 0..8 {
            let key = tones[i % 3] - 12 + if i >= 4 { 12 } else { 0 };
            left.note(0, key, 60 + (i as u8 * 5), start + i as u32 * beat / 2, beat / 2);
        }

        // Right hand: arpeggio runs that climb and fall, plus chord stabs every other bar.
        let section = b / 16;
        let step = if section == 1 { beat / 4 } else { beat / 2 };
        let count = bar / step;
        for i in 0..count {
            let octave = 12 * ((i / 3) % 3) as u8;
            let key = tones[(i % 3) as usize] + 12 + octave;
            let vel = 55 + rng.range(0, 60) as u8;
            right.note(1, key, vel, start + i * step, step * 2);
        }
        if b % 2 == 1 {
            for (j, &t) in tones.iter().enumerate() {
                right.note(1, t + 24, 110, start + bar - beat, beat + j as u32 * 20);
            }
        }
    }
    // Final low-to-high glissando across the whole keyboard.
    let end = 44 * bar;
    for (i, key) in (21u8..=108).enumerate() {
        right.note(1, key, 50 + (i as u8 / 2), end + i as u32 * 30, beat * 2);
    }
    vec![tempo.build(), right.build(), left.build()]
}

fn stress(notes: u32) -> Vec<Vec<TrackEvent<'static>>> {
    let mut rng = Lcg(42);
    let tracks = 4;
    let mut builders: Vec<TrackBuilder> = (0..tracks).map(|_| TrackBuilder::default()).collect();
    builders[0].meta(0, MetaMessage::Tempo(u24::new(500_000)));
    // ~40 notes per second.
    for i in 0..notes {
        let t = &mut builders[(i % tracks) as usize];
        let start = i * PPQ / 20 + rng.range(0, PPQ / 20);
        let len = rng.range(PPQ / 16, PPQ * 4);
        t.note((i % tracks) as u8, rng.range(21, 109) as u8, rng.range(20, 128) as u8, start, len);
    }
    builders.into_iter().map(TrackBuilder::build).collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("demo");
    let path = args.get(1).cloned().unwrap_or_else(|| format!("{mode}.mid"));

    let tracks = match mode {
        "demo" => demo(),
        "stress" => stress(args.get(2).and_then(|n| n.parse().ok()).unwrap_or(60_000)),
        other => return Err(format!("unknown mode {other:?}; use demo or stress").into()),
    };
    let mut smf = Smf::new(Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ as u16))));
    smf.tracks = tracks;
    smf.save(&path)?;
    println!("wrote {path}");
    Ok(())
}
