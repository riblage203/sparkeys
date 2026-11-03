//! Tick → seconds conversion for Standard MIDI Files.

use midly::{MetaMessage, Smf, Timing, TrackEventKind};

/// Default MIDI tempo when a file sets none: 120 BPM.
const DEFAULT_US_PER_QUARTER: u32 = 500_000;

/// A span of constant tempo starting at `tick`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    tick: u64,
    /// Absolute time of `tick`.
    sec: f64,
    sec_per_tick: f64,
}

/// Maps absolute ticks to seconds, honoring every tempo change in the file.
///
/// Metrical files (PPQ) use a piecewise-linear map built from all `Tempo`
/// meta events of all tracks; SMPTE timecode files have a fixed tick length
/// and ignore tempo events, as the spec requires.
#[derive(Debug, Clone, PartialEq)]
pub struct TempoMap {
    /// Never empty: the first segment always starts at tick 0.
    segments: Vec<Segment>,
}

impl TempoMap {
    /// Builds a metrical map from `(absolute tick, microseconds per quarter note)` changes in any order.
    pub fn metrical(ppq: u16, changes: impl IntoIterator<Item = (u64, u32)>) -> Self {
        let ppq = f64::from(ppq.max(1));
        let spt = |us: u32| f64::from(us) / 1_000_000.0 / ppq;

        let mut changes: Vec<(u64, u32)> = changes.into_iter().collect();
        // Stable sort keeps file order for simultaneous changes, so the last one wins.
        changes.sort_by_key(|&(tick, _)| tick);

        let mut segments = vec![Segment { tick: 0, sec: 0.0, sec_per_tick: spt(DEFAULT_US_PER_QUARTER) }];
        for (tick, us) in changes {
            let last = segments[segments.len() - 1];
            if tick == last.tick {
                let idx = segments.len() - 1;
                segments[idx].sec_per_tick = spt(us);
            } else {
                let sec = last.sec + (tick - last.tick) as f64 * last.sec_per_tick;
                segments.push(Segment { tick, sec, sec_per_tick: spt(us) });
            }
        }
        Self { segments }
    }

    /// Builds a map with a constant tick length (SMPTE timecode files).
    pub fn fixed(sec_per_tick: f64) -> Self {
        Self { segments: vec![Segment { tick: 0, sec: 0.0, sec_per_tick }] }
    }

    /// Builds the map for a parsed file.
    pub fn from_smf(smf: &Smf<'_>) -> Self {
        match smf.header.timing {
            Timing::Timecode(fps, subframes) => {
                let ticks_per_sec = f64::from(fps.as_f32()) * f64::from(subframes.max(1));
                Self::fixed(1.0 / ticks_per_sec)
            }
            Timing::Metrical(ppq) => {
                let changes = smf.tracks.iter().flat_map(|track| {
                    let mut tick = 0u64;
                    track.iter().filter_map(move |ev| {
                        tick += u64::from(ev.delta.as_int());
                        match ev.kind {
                            TrackEventKind::Meta(MetaMessage::Tempo(us)) => Some((tick, us.as_int())),
                            _ => None,
                        }
                    })
                });
                Self::metrical(ppq.as_int(), changes)
            }
        }
    }

    /// Absolute time in seconds of `tick`.
    pub fn tick_to_sec(&self, tick: u64) -> f64 {
        let idx = self.segments.partition_point(|s| s.tick <= tick);
        // idx >= 1 because segments[0].tick == 0 <= any tick.
        let seg = self.segments[idx.saturating_sub(1)];
        seg.sec + (tick - seg.tick) as f64 * seg.sec_per_tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn default_tempo_is_120_bpm() {
        let map = TempoMap::metrical(480, []);
        assert!(close(map.tick_to_sec(480), 0.5));
        assert!(close(map.tick_to_sec(960 * 10), 10.0));
    }

    #[test]
    fn tempo_change_mid_song() {
        // 120 BPM for two beats, then 60 BPM.
        let map = TempoMap::metrical(480, [(0, 500_000), (960, 1_000_000)]);
        assert!(close(map.tick_to_sec(960), 1.0));
        assert!(close(map.tick_to_sec(1440), 2.0));
        assert!(close(map.tick_to_sec(1920), 3.0));
    }

    #[test]
    fn many_changes_unsorted_and_other_ppq() {
        // PPQ 96: 140 BPM, 70 BPM at beat 4, 280 BPM at beat 6.
        let us_140 = 428_571;
        let map = TempoMap::metrical(96, [(576, 214_286), (0, us_140), (384, 857_143)]);
        let beat_140 = f64::from(us_140) / 1e6;
        let t4 = 4.0 * beat_140;
        let t6 = t4 + 2.0 * 0.857_143;
        assert!(close(map.tick_to_sec(384), t4));
        assert!(close(map.tick_to_sec(576), t6));
        assert!(close(map.tick_to_sec(576 + 96), t6 + 0.214_286));
    }

    #[test]
    fn simultaneous_changes_last_wins() {
        let map = TempoMap::metrical(480, [(480, 250_000), (480, 1_000_000)]);
        assert!(close(map.tick_to_sec(960), 0.5 + 1.0));
    }

    #[test]
    fn timecode_ignores_tempo() {
        // 25 fps * 40 subframes = 1000 ticks per second.
        let map = TempoMap::fixed(1.0 / 1000.0);
        assert!(close(map.tick_to_sec(1500), 1.5));
    }
}
