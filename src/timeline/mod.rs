//! Playback transport: play/pause, seek and speed.
//!
//! The UI edits the [`Transport`]; [`AudioSync`] forwards changes to the audio
//! thread and sets the position from the audio clock (or from the wall clock
//! when no audio device is available).

mod clock;
mod sync;

pub use sync::AudioSync;

/// Seconds of silence shown before the first note so it can fall into view.
pub const LEAD_IN_S: f64 = 2.0;
/// Slowest allowed playback speed.
pub const MIN_SPEED: f64 = 0.25;
/// Fastest allowed playback speed.
pub const MAX_SPEED: f64 = 2.0;
/// Shortest loop accepted (seconds); shorter A–B ranges are ignored.
pub const MIN_LOOP_S: f64 = 0.5;

/// The playback position and state of the loaded song.
#[derive(Debug, Clone)]
pub struct Transport {
    position_s: f64,
    playing: bool,
    speed: f64,
    duration_s: f64,
    /// Incremented on every jump (seek, load, restart) so the audio thread's
    /// reports can be matched to the request they follow.
    seek_serial: u64,
    loop_a: Option<f64>,
    loop_b: Option<f64>,
    loop_enabled: bool,
}

impl Transport {
    /// An empty, paused transport at `speed`.
    pub fn new(speed: f64) -> Self {
        Self {
            position_s: -LEAD_IN_S,
            playing: false,
            speed: speed.clamp(MIN_SPEED, MAX_SPEED),
            duration_s: 0.0,
            seek_serial: 0,
            loop_a: None,
            loop_b: None,
            loop_enabled: false,
        }
    }

    /// Resets for a newly loaded song of `duration_s`, paused at the lead-in.
    pub fn load(&mut self, duration_s: f64) {
        self.duration_s = duration_s.max(0.0);
        self.position_s = -LEAD_IN_S;
        self.playing = false;
        self.seek_serial += 1;
        self.clear_loop();
    }

    /// Sets loop start at the current position (B is dropped if it is now before A).
    pub fn set_loop_a(&mut self) {
        let a = self.position_s.max(0.0);
        self.loop_a = Some(a);
        if self.loop_b.is_some_and(|b| b - a < MIN_LOOP_S) {
            self.loop_b = None;
        }
    }

    /// Sets loop end at the current position and turns looping on.
    pub fn set_loop_b(&mut self) {
        let b = self.position_s;
        let a = self.loop_a.unwrap_or(0.0);
        if b - a >= MIN_LOOP_S {
            self.loop_a = Some(a);
            self.loop_b = Some(b);
            self.loop_enabled = true;
        }
    }

    /// Turns looping on/off (keeps the A–B points).
    pub fn toggle_loop(&mut self) {
        self.loop_enabled = !self.loop_enabled && self.loop_points().is_some();
    }

    /// Forgets the loop.
    pub fn clear_loop(&mut self) {
        self.loop_a = None;
        self.loop_b = None;
        self.loop_enabled = false;
    }

    /// A and B as set (whether or not looping is on).
    pub fn loop_points(&self) -> Option<(f64, f64)> {
        Some((self.loop_a?, self.loop_b?))
    }

    /// The A–B range when looping is on.
    pub fn active_loop(&self) -> Option<(f64, f64)> {
        self.loop_points().filter(|_| self.loop_enabled)
    }

    /// Current song time in seconds (negative during the lead-in).
    pub fn position(&self) -> f64 {
        self.position_s
    }

    /// Earliest seekable time.
    pub fn start(&self) -> f64 {
        -LEAD_IN_S
    }

    /// Song length in seconds.
    pub fn duration(&self) -> f64 {
        self.duration_s
    }

    /// Whether time is advancing.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Playback speed multiplier.
    pub fn speed(&self) -> f64 {
        self.speed
    }

    /// Sets the speed, clamped to `MIN_SPEED..=MAX_SPEED`.
    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
    }

    /// Toggles play/pause; playing from the end restarts from the beginning.
    pub fn toggle(&mut self) {
        if self.playing {
            self.playing = false;
        } else {
            if self.position_s >= self.duration_s {
                self.seek(-LEAD_IN_S);
            }
            self.playing = true;
        }
    }

    /// Jumps to `time_s`, clamped to the song.
    pub fn seek(&mut self, time_s: f64) {
        self.position_s = time_s.clamp(-LEAD_IN_S, self.duration_s);
        self.seek_serial += 1;
    }

    /// Identifies the latest jump; see the field docs.
    pub fn seek_serial(&self) -> u64 {
        self.seek_serial
    }

    /// Sets the position reported by the audio clock (not a jump).
    pub fn sync_position(&mut self, time_s: f64) {
        self.position_s = time_s.clamp(-LEAD_IN_S, self.duration_s);
    }

    /// Stops at the end of the song.
    pub fn finish(&mut self) {
        self.playing = false;
        self.position_s = self.duration_s;
    }

    /// Advances by `dt_s` wall-clock seconds scaled by speed; pauses at the end.
    pub fn advance(&mut self, dt_s: f64) {
        if !self.playing {
            return;
        }
        let before = self.position_s;
        self.position_s += dt_s * self.speed;
        if let Some((a, b)) = self.active_loop()
            && before <= b
            && self.position_s >= b
        {
            self.position_s = a + (self.position_s - b);
        }
        if self.position_s >= self.duration_s {
            self.position_s = self.duration_s;
            self.playing = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_scales_with_speed_and_stops_at_end() {
        let mut t = Transport::new(2.0);
        t.load(3.0);
        t.toggle();
        t.advance(1.0);
        assert!((t.position() - 0.0).abs() < 1e-9);
        t.advance(10.0);
        assert_eq!(t.position(), 3.0);
        assert!(!t.is_playing());
        t.toggle();
        assert_eq!(t.position(), -LEAD_IN_S);
    }

    #[test]
    fn paused_does_not_move_and_seek_clamps() {
        let mut t = Transport::new(1.0);
        t.load(10.0);
        t.advance(1.0);
        assert_eq!(t.position(), -LEAD_IN_S);
        t.seek(99.0);
        assert_eq!(t.position(), 10.0);
        t.seek(-99.0);
        assert_eq!(t.position(), -LEAD_IN_S);
    }

    #[test]
    fn jumps_bump_the_serial_but_sync_does_not() {
        let mut t = Transport::new(1.0);
        let s0 = t.seek_serial();
        t.load(10.0);
        t.seek(2.0);
        assert_eq!(t.seek_serial(), s0 + 2);
        t.sync_position(3.0);
        assert_eq!(t.seek_serial(), s0 + 2);
        t.finish();
        t.toggle(); // restart from the end is a jump
        assert_eq!(t.seek_serial(), s0 + 3);
        assert_eq!(t.position(), -LEAD_IN_S);
    }

    #[test]
    fn loop_points_and_wall_clock_wrap() {
        let mut t = Transport::new(1.0);
        t.load(10.0);
        t.seek(2.0);
        t.set_loop_a();
        t.seek(2.2);
        t.set_loop_b(); // too short: ignored
        assert_eq!(t.active_loop(), None);
        t.seek(4.0);
        t.set_loop_b();
        assert_eq!(t.active_loop(), Some((2.0, 4.0)));
        t.toggle();
        t.advance(0.5);
        assert!((t.position() - 2.5).abs() < 1e-9);
        t.toggle_loop();
        assert_eq!(t.active_loop(), None);
        assert_eq!(t.loop_points(), Some((2.0, 4.0)));
    }

    #[test]
    fn speed_is_clamped() {
        let mut t = Transport::new(9.0);
        assert_eq!(t.speed(), MAX_SPEED);
        t.set_speed(0.0);
        assert_eq!(t.speed(), MIN_SPEED);
    }
}
