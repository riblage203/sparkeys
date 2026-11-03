//! Smooth visual time derived from the (bursty) audio clock.

/// Errors larger than this are treated as a jump (seek, stall) and snapped.
const SNAP_THRESHOLD_S: f64 = 0.15;
/// Time constant of the correction toward the audio clock.
const CORRECTION_TAU_S: f64 = 0.15;

/// Visual playback time: advances by frame time × speed and is gently pulled
/// toward the audio clock, so frames never jitter and never drift.
#[derive(Debug, Clone, Default)]
pub struct VisualClock {
    position_s: f64,
}

impl VisualClock {
    /// Jumps straight to `time_s`.
    pub fn snap(&mut self, time_s: f64) {
        self.position_s = time_s;
    }

    /// Advances one frame of `dt_s` seconds toward the audio clock's `target_s`.
    /// While playing the result never moves backwards (except on a snap).
    pub fn update(&mut self, target_s: f64, playing: bool, dt_s: f64, speed: f64) -> f64 {
        if !playing {
            self.position_s = target_s;
            return self.position_s;
        }
        let predicted = self.position_s + dt_s * speed;
        let error = target_s - predicted;
        if error.abs() > SNAP_THRESHOLD_S {
            self.position_s = target_s;
        } else {
            let k = 1.0 - (-dt_s / CORRECTION_TAU_S).exp();
            self.position_s = (predicted + error * k).max(self.position_s);
        }
        self.position_s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_on_jittery_audio_clock_without_going_backwards() {
        let mut clock = VisualClock::default();
        let dt = 1.0 / 144.0;
        let speed = 1.5;
        let mut prev = 0.0;
        for frame in 0..2000 {
            let true_time = frame as f64 * dt * speed;
            // Audio reports in ~5 ms bursts plus a constant 20 ms offset to absorb.
            let quantized = (true_time / 0.005).floor() * 0.005 + 0.02;
            let pos = clock.update(quantized, true, dt, speed);
            assert!(pos >= prev, "went backwards at frame {frame}");
            prev = pos;
        }
        let final_target = 1999.0 * dt * speed + 0.02;
        assert!((prev - final_target).abs() < 0.006);
    }

    #[test]
    fn snaps_on_large_jumps_and_when_paused() {
        let mut clock = VisualClock::default();
        assert_eq!(clock.update(10.0, true, 0.016, 1.0), 10.0);
        assert_eq!(clock.update(3.0, false, 0.016, 1.0), 3.0);
    }
}
