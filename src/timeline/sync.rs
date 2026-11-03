//! Keeps the audio thread in step with the transport and derives visual time
//! from the audio clock.

use crate::audio::{AudioCmd, AudioEngine};
use crate::midi::Song;

use super::Transport;
use super::clock::VisualClock;

/// What the audio thread has been told so far; diffed each frame so the UI
/// can mutate the transport freely and only changes are sent.
#[derive(Debug, Clone, PartialEq)]
struct Sent {
    song_id: u64,
    seek_serial: u64,
    playing: bool,
    speed: f64,
    volume: f32,
    muted: Vec<bool>,
}

/// Transport ↔ audio synchronization state.
#[derive(Debug, Default)]
pub struct AudioSync {
    sent: Option<Sent>,
    visual: VisualClock,
    sync_error_ms: Option<f64>,
}

impl AudioSync {
    /// Forgets what was sent, e.g. after the audio engine was restarted.
    pub fn reset(&mut self) {
        self.sent = None;
    }

    /// Visual minus audio time from the last frame, in milliseconds (`None` without audio).
    pub fn sync_error_ms(&self) -> Option<f64> {
        self.sync_error_ms
    }

    /// Runs once per frame. With an engine, pushes transport changes to the
    /// audio thread and sets the transport position from the smoothed audio
    /// clock; without one, falls back to advancing by wall-clock `dt_s`.
    ///
    /// `song_id` must change whenever a different song is loaded.
    pub fn update(
        &mut self,
        engine: Option<&mut AudioEngine>,
        transport: &mut Transport,
        song: Option<&Song>,
        song_id: u64,
        volume: f32,
        dt_s: f64,
    ) {
        let Some(engine) = engine else {
            transport.advance(dt_s);
            self.visual.snap(transport.position());
            self.sync_error_ms = None;
            return;
        };
        engine.collect_garbage();

        let sent = self.sent.get_or_insert_with(|| Sent {
            // Values that never match, so everything is sent on first use.
            song_id: u64::MAX,
            seek_serial: u64::MAX,
            playing: false,
            speed: f64::NAN,
            volume: f32::NAN,
            muted: Vec::new(),
        });

        if sent.song_id != song_id {
            if let Some(song) = song {
                let muted: Vec<bool> = song.tracks.iter().map(|t| t.muted).collect();
                engine.send(AudioCmd::LoadSong {
                    events: song.events.clone(),
                    muted: muted.clone(),
                    duration_s: song.duration_s,
                });
                sent.muted = muted;
            }
            sent.song_id = song_id;
            // A load stops the sequencer and needs a seek to the transport position.
            sent.seek_serial = u64::MAX;
            sent.playing = false;
        }
        if sent.seek_serial != transport.seek_serial() {
            engine.send(AudioCmd::Seek { time_s: transport.position(), serial: transport.seek_serial() });
            sent.seek_serial = transport.seek_serial();
        }
        if sent.playing != transport.is_playing() {
            engine.send(if transport.is_playing() { AudioCmd::Play } else { AudioCmd::Pause });
            sent.playing = transport.is_playing();
        }
        if sent.speed != transport.speed() {
            engine.send(AudioCmd::SetSpeed(transport.speed()));
            sent.speed = transport.speed();
        }
        if sent.volume != volume {
            engine.send(AudioCmd::SetVolume(volume));
            sent.volume = volume;
        }
        if let Some(song) = song {
            for (i, track) in song.tracks.iter().enumerate() {
                if let (Some(was), Ok(part)) = (sent.muted.get_mut(i), u16::try_from(i))
                    && *was != track.muted
                {
                    engine.send(AudioCmd::SetMute { part, muted: track.muted });
                    *was = track.muted;
                }
            }
        }

        let snap = engine.clock().read();
        if snap.seek_serial != transport.seek_serial() {
            // A seek or load is still in flight: show the requested time.
            self.visual.snap(transport.position());
            self.sync_error_ms = None;
            return;
        }
        if snap.ended && transport.is_playing() {
            transport.finish();
            sent.playing = false;
        }
        let target = snap.position_at(engine.clock().epoch().elapsed());
        let playing = transport.is_playing() && snap.playing;
        let pos = self.visual.update(target, playing, dt_s, transport.speed());
        transport.sync_position(pos);
        self.sync_error_ms = Some((pos - target) * 1000.0);
    }
}
