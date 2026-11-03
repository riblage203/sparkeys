//! Lock-free playback clock published by the audio thread.

use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::time::{Duration, Instant};

/// What the audio thread last reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockSnapshot {
    /// Song time of the first sample of the last rendered buffer.
    pub position_s: f64,
    /// When that sample reaches the speakers, relative to [`SharedClock::epoch`].
    pub heard_at: Duration,
    /// Playback speed in effect.
    pub speed: f64,
    /// Whether the sequencer is advancing.
    pub playing: bool,
    /// Whether playback stopped because it reached the end of the song.
    pub ended: bool,
    /// Serial of the last seek the audio thread applied.
    pub seek_serial: u64,
}

impl ClockSnapshot {
    /// Song time being heard at `now` (`now` relative to the epoch).
    pub fn position_at(&self, now: Duration) -> f64 {
        if !self.playing {
            return self.position_s;
        }
        let elapsed = now.as_secs_f64() - self.heard_at.as_secs_f64();
        self.position_s + elapsed * self.speed
    }
}

/// Single-writer seqlock: the audio callback writes without locking or
/// allocating, the render thread reads a consistent snapshot (retrying if it
/// raced a write).
pub struct SharedClock {
    epoch: Instant,
    seq: AtomicU64,
    position_bits: AtomicU64,
    heard_at_ns: AtomicU64,
    speed_bits: AtomicU64,
    /// bit 0: playing, bit 1: ended, bits 2..: seek serial.
    flags: AtomicU64,
}

impl SharedClock {
    /// A paused clock at time 0.
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            seq: AtomicU64::new(0),
            position_bits: AtomicU64::new(0f64.to_bits()),
            heard_at_ns: AtomicU64::new(0),
            speed_bits: AtomicU64::new(1f64.to_bits()),
            flags: AtomicU64::new(0),
        }
    }

    /// Reference instant for [`ClockSnapshot::heard_at`].
    pub fn epoch(&self) -> Instant {
        self.epoch
    }

    /// Publishes a new snapshot. Must only be called from one thread.
    pub fn publish(&self, snap: &ClockSnapshot) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.position_bits.store(snap.position_s.to_bits(), Ordering::Relaxed);
        // Saturates after ~584 years of uptime.
        self.heard_at_ns.store(u64::try_from(snap.heard_at.as_nanos()).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.speed_bits.store(snap.speed.to_bits(), Ordering::Relaxed);
        let flags = u64::from(snap.playing) | (u64::from(snap.ended) << 1) | (snap.seek_serial << 2);
        self.flags.store(flags, Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// Reads a consistent snapshot.
    pub fn read(&self) -> ClockSnapshot {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let position_s = f64::from_bits(self.position_bits.load(Ordering::Relaxed));
            let heard_at = Duration::from_nanos(self.heard_at_ns.load(Ordering::Relaxed));
            let speed = f64::from_bits(self.speed_bits.load(Ordering::Relaxed));
            let flags = self.flags.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return ClockSnapshot {
                    position_s,
                    heard_at,
                    speed,
                    playing: flags & 1 == 1,
                    ended: flags & 2 == 2,
                    seek_serial: flags >> 2,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_extrapolation() {
        let clock = SharedClock::new();
        let snap = ClockSnapshot {
            position_s: 12.5,
            heard_at: Duration::from_millis(1000),
            speed: 2.0,
            playing: true,
            ended: false,
            seek_serial: 7,
        };
        clock.publish(&snap);
        let read = clock.read();
        assert_eq!(read, snap);
        assert!((read.position_at(Duration::from_millis(1500)) - 13.5).abs() < 1e-9);
        // Before the buffer is heard, the heard position is still earlier.
        assert!((read.position_at(Duration::from_millis(900)) - 12.3).abs() < 1e-9);
    }

    #[test]
    fn paused_does_not_extrapolate() {
        let clock = SharedClock::new();
        clock.publish(&ClockSnapshot {
            position_s: 3.0,
            heard_at: Duration::ZERO,
            speed: 1.0,
            playing: false,
            ended: true,
            seek_serial: 1,
        });
        let read = clock.read();
        assert!(read.ended);
        assert_eq!(read.position_at(Duration::from_secs(5)), 3.0);
    }
}
