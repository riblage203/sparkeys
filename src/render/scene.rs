//! Per-frame scene building: note bars, keyboard, hit line, particle emission
//! and the small animations (key press depth, hit pulse) that tie them together.

use crate::config::VisualConfig;
use crate::midi::{Note, Song};

use super::layout::{BLACK_LENGTH, FIRST_KEY, KEY_COUNT, KeyGeom, KeyboardLayout, WHITE_KEY_COUNT};
use super::palette::Palette;
use super::particles::{Particle, Particles};
use super::quads::Batch;

/// Larger forward jumps than this (seconds of song time) are seeks: no bursts.
const MAX_CONTINUOUS_STEP_S: f64 = 0.25;

/// What to draw this frame.
pub struct SceneInput<'a> {
    /// The loaded song, if any.
    pub song: Option<&'a Song>,
    /// Song time at the hit line, in seconds.
    pub time_s: f64,
    /// Seconds of music visible between the top of the screen and the hit line.
    pub fall_seconds: f64,
    /// Wall-clock seconds since the previous frame (animations, particles).
    pub dt_s: f32,
    /// User visual settings.
    pub visual: &'a VisualConfig,
}

/// Screen-space placement of the keyboard, in physical pixels.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    /// Target width.
    pub width: f32,
    /// Target height.
    pub height: f32,
    /// Width of one white key.
    pub unit: f32,
    /// Y of the hit line (top of the keyboard).
    pub hit_y: f32,
    /// Keyboard height.
    pub keyboard_h: f32,
}

impl Frame {
    /// Lays out the keyboard for a `width` × `height` target.
    pub fn new(width: f32, height: f32) -> Self {
        let unit = width / WHITE_KEY_COUNT as f32;
        let keyboard_h = (unit * 5.6).min(height * 0.25).max(24.0);
        Self { width, height, unit, hit_y: height - keyboard_h, keyboard_h }
    }
}

/// Where the scene is drawn into.
pub struct SceneOutput<'a> {
    /// Note bars (notes pipeline).
    pub notes: &'a mut Batch,
    /// Keyboard, hit line, glows (quad pipeline).
    pub quads: &'a mut Batch,
    /// Particle spawns.
    pub particles: &'a mut Particles,
}

/// Color, velocity (0..1) and start time of the newest note holding a key.
type Held = Option<([f32; 3], f32, f64)>;

/// Animation state carried between frames.
pub struct SceneState {
    /// Key press depth, 0 (up) .. 1 (fully down), eased.
    press: [f32; KEY_COUNT],
    /// Last color that pressed each key (kept while it releases).
    key_color: [[f32; 3]; KEY_COUNT],
    /// Fractional particle budget per held key for the sustain trickle.
    trickle: [f32; KEY_COUNT],
    /// Hit-line energy, bumped by note-ons and decaying.
    pulse: f32,
    last_time_s: f64,
    rng: Rng,
}

impl SceneState {
    /// Fresh state: no keys down.
    pub fn new() -> Self {
        Self {
            press: [0.0; KEY_COUNT],
            key_color: [[1.0; 3]; KEY_COUNT],
            trickle: [0.0; KEY_COUNT],
            pulse: 0.0,
            last_time_s: f64::NEG_INFINITY,
            rng: Rng(0x9E37_79B9_7F4A_7C15),
        }
    }

    /// Builds this frame's geometry and particle spawns.
    pub fn build(
        &mut self,
        out: &mut SceneOutput<'_>,
        layout: &KeyboardLayout,
        palette: &Palette,
        input: &SceneInput<'_>,
        frame: &Frame,
    ) {
        let dt = input.dt_s.max(0.0);
        let mut held: [Held; KEY_COUNT] = [None; KEY_COUNT];

        if let Some(song) = input.song {
            draw_notes(out.notes, layout, palette, song, input, frame, &mut held);
            let step = input.time_s - self.last_time_s;
            if step > 0.0 && step < MAX_CONTINUOUS_STEP_S {
                self.emit_bursts(out.particles, layout, palette, song, input, frame);
            }
        }
        self.last_time_s = input.time_s;

        self.animate_keys(out.particles, layout, &held, input.visual.particles, frame, dt);
        self.pulse = (self.pulse * (-dt * 5.0).exp()).min(3.0);

        draw_keyboard(out.quads, layout, frame, &self.press, &self.key_color);
        if input.visual.show_hit_line {
            draw_hit_line(out.quads, frame, self.pulse);
        }
        if let Some(song) = input.song {
            draw_pedal(out.quads, frame, song.pedal_down_at(input.time_s));
        }
    }

    /// Bursts of embers for every note that started since the previous frame.
    fn emit_bursts(
        &mut self,
        particles: &mut Particles,
        layout: &KeyboardLayout,
        palette: &Palette,
        song: &Song,
        input: &SceneInput<'_>,
        frame: &Frame,
    ) {
        let amount = input.visual.particles.max(0.0);
        let from = self.last_time_s;
        for note in song.notes_in_window(from, input.time_s) {
            if note.start_s <= from || note.start_s > input.time_s || !part_visible(song, note) {
                continue;
            }
            let Some(key) = layout.key(note.pitch) else { continue };
            let v = f32::from(note.velocity) / 127.0;
            self.pulse += 0.12 + 0.3 * v;
            let count = ((10.0 + 70.0 * v.powf(1.5)) * amount).round() as usize;
            let (x, w) = note_lane(key, frame);
            let color = palette.color(note.track);
            for _ in 0..count {
                let p = self.spark(x, w, frame, color, v, 1.0);
                particles.emit(p);
            }
        }
    }

    /// Eases key press depth and emits the sustain trickle from held keys.
    fn animate_keys(
        &mut self,
        particles: &mut Particles,
        layout: &KeyboardLayout,
        held: &[Held; KEY_COUNT],
        amount: f32,
        frame: &Frame,
        dt: f32,
    ) {
        let rate = 22.0 * amount.max(0.0);
        for (i, (_, key)) in layout.iter().enumerate() {
            let target = if held[i].is_some() { 1.0 } else { 0.0 };
            let speed = if target > self.press[i] { 40.0 } else { 9.0 };
            self.press[i] += (target - self.press[i]) * (1.0 - (-speed * dt).exp());
            let Some((color, v, _)) = held[i] else {
                self.trickle[i] = 0.0;
                continue;
            };
            self.key_color[i] = color;
            self.trickle[i] += rate * (0.35 + 0.65 * v) * dt;
            let (x, w) = note_lane(key, frame);
            while self.trickle[i] >= 1.0 {
                self.trickle[i] -= 1.0;
                let p = self.spark(x, w, frame, color, v * 0.5, 0.55);
                particles.emit(p);
            }
        }
    }

    /// One ember rising from the hit line within the lane `[x, x + w]`.
    fn spark(&mut self, x: f32, w: f32, frame: &Frame, color: [f32; 3], v: f32, energy: f32) -> Particle {
        let scale = frame.height / 1080.0;
        let r = &mut self.rng;
        let speed = (110.0 + 420.0 * v) * (0.3 + 0.7 * r.next()) * scale * energy;
        let size = (1.4 + 3.2 * r.next() * r.next()) * (frame.unit / 30.0).max(0.8);
        let life = (0.55 + 1.2 * v) * (0.5 + 0.5 * r.next()) * energy.sqrt();
        let glow = 3.0 + 4.0 * v;
        Particle {
            pos: [x + r.next() * w, frame.hit_y - 2.0 - r.next() * 4.0],
            vel: [(r.next() - 0.5) * speed * 0.7, -speed],
            color: [color[0] * glow, color[1] * glow, color[2] * glow, 1.0],
            life,
            max_life: life,
            size,
            seed: r.next(),
        }
    }
}

fn part_visible(song: &Song, note: &Note) -> bool {
    song.tracks.get(usize::from(note.track)).is_none_or(|tr| tr.visible)
}

/// Horizontal extent of a note bar for `key`.
fn note_lane(key: KeyGeom, frame: &Frame) -> (f32, f32) {
    let lane_w = key.width * frame.unit;
    let w = if key.black { lane_w * 0.92 } else { lane_w * 0.84 };
    (key.x * frame.unit + (lane_w - w) / 2.0, w)
}

fn draw_notes(
    batch: &mut Batch,
    layout: &KeyboardLayout,
    palette: &Palette,
    song: &Song,
    input: &SceneInput<'_>,
    frame: &Frame,
    held: &mut [Held; KEY_COUNT],
) {
    let t = input.time_s;
    let fall = input.fall_seconds.max(0.1);
    let px_per_s = f64::from(frame.hit_y) / fall;
    let notes = song.notes_in_window(t, t + fall);

    // White-key notes first so black-key notes, which straddle two lanes, stay on top.
    for black_pass in [false, true] {
        for note in notes {
            let Some(key) = layout.key(note.pitch) else { continue };
            if key.black != black_pass || note.end_s < t || !part_visible(song, note) {
                continue;
            }
            let base = palette.color(note.track);
            let v = f32::from(note.velocity) / 127.0;
            let active = note.start_s <= t && t < note.end_s;
            if active {
                let slot = &mut held[usize::from(note.pitch - FIRST_KEY)];
                if slot.is_none_or(|(_, _, start)| note.start_s >= start) {
                    *slot = Some((base, v, note.start_s));
                }
            }

            // Bars extend under the keyboard, which covers their lower end.
            let bottom = (f64::from(frame.hit_y) - (note.start_s - t) * px_per_s) as f32;
            let top = (f64::from(frame.hit_y) - (note.end_s - t) * px_per_s) as f32;
            if top >= frame.hit_y {
                continue;
            }
            let h = (bottom - top).max(3.0);
            let (x, w) = note_lane(key, frame);
            // Brief flash for ~80 ms after the note reaches the hit line.
            let flash = if active { (1.0 - (t - note.start_s) as f32 / 0.08).max(0.0) } else { 0.0 };
            batch.push(
                [x, bottom - h, w, h],
                [base[0], base[1], base[2], 0.55 + 0.45 * v],
                [input.visual.corner_radius, f32::from(u8::from(active)), flash, 0.0],
            );
        }
    }
}

fn draw_keyboard(
    batch: &mut Batch,
    layout: &KeyboardLayout,
    frame: &Frame,
    press: &[f32; KEY_COUNT],
    key_color: &[[f32; 3]; KEY_COUNT],
) {
    let gap = (frame.unit * 0.06).max(1.0);
    let dip = frame.unit * 0.07;
    batch.push([0.0, frame.hit_y, frame.width, frame.keyboard_h], [0.003, 0.003, 0.004, 1.0], [0.0; 4]);

    for black_pass in [false, true] {
        for (i, (_, key)) in layout.iter().enumerate().filter(|(_, (_, k))| k.black == black_pass) {
            let p = press[i];
            let c = key_color[i];
            let x = key.x * frame.unit;
            let w = key.width * frame.unit;
            // Blend the idle key color toward the (HDR-boosted) note color.
            let lit = |base: [f32; 3], gain: f32, amount: f32| -> [f32; 4] {
                [
                    base[0] + (c[0] * gain - base[0]) * amount,
                    base[1] + (c[1] * gain - base[1]) * amount,
                    base[2] + (c[2] * gain - base[2]) * amount,
                    1.0,
                ]
            };
            if key.black {
                let h = frame.keyboard_h * BLACK_LENGTH;
                let color = lit([0.010, 0.010, 0.013], 1.4, p * 0.9);
                batch.push([x, frame.hit_y + p * dip * 0.6, w, h], color, [-1.8 + p, 0.0, 0.0, 0.0]);
            } else {
                let color = lit([0.80, 0.79, 0.77], 1.5, p * 0.85);
                batch.push(
                    [x + gap / 2.0, frame.hit_y + p * dip, w - gap, frame.keyboard_h - gap - p * dip],
                    color,
                    [0.14, 0.0, 0.0, 0.0],
                );
            }
            if p > 0.01 {
                // Light spilling up from the pressed key into the note area.
                let glow_h = frame.unit * 3.2;
                let gc = [c[0] * 1.4, c[1] * 1.4, c[2] * 1.4, 0.55 * p];
                batch.push([x - w * 0.6, frame.hit_y - glow_h, w * 2.2, glow_h], gc, [0.0, 1.0, 1.0, 0.0]);
            }
        }
    }
}

fn draw_hit_line(batch: &mut Batch, frame: &Frame, pulse: f32) {
    let w = frame.width;
    let y = frame.hit_y;
    // Red felt strip, a soft glow band that swells with hits, then a bright HDR core.
    batch.push([0.0, y, w, 3.0], [0.10, 0.008, 0.01, 1.0], [0.0; 4]);
    let band_h = 10.0 + 10.0 * pulse.min(2.0);
    let e = 0.8 + pulse;
    batch.push(
        [0.0, y - band_h, w, band_h],
        [e, 0.38 * e, 0.10 * e, 0.10 + 0.08 * pulse.min(2.0)],
        [0.0, 1.0, 0.0, 0.0],
    );
    let core = 1.1 + 1.6 * pulse;
    batch.push([0.0, y - 2.0, w, 2.0], [core, 0.55 * core, 0.25 * core, 1.0], [0.0; 4]);
}

/// Small pill above the left end of the keyboard that lights while the sustain pedal is down.
fn draw_pedal(batch: &mut Batch, frame: &Frame, down: bool) {
    let (w, h) = (frame.unit * 2.0, 4.0);
    let (x, y) = (frame.unit * 0.5, frame.hit_y - 18.0);
    if down {
        batch.push([x - 3.0, y - 3.0, w + 6.0, h + 6.0], [1.0, 0.45, 0.12, 0.18], [0.0; 4]);
        batch.push([x, y, w, h], [1.6, 0.9, 0.45, 0.9], [0.0; 4]);
    } else {
        batch.push([x, y, w, h], [0.6, 0.6, 0.65, 0.12], [0.0; 4]);
    }
}

/// Tiny xorshift PRNG; quality is irrelevant for sparks, speed and no deps matter.
struct Rng(u64);

impl Rng {
    /// Uniform in [0, 1).
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        // Top 24 bits → exact f32 in [0, 1).
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_in_unit_range() {
        let mut r = Rng(1);
        for _ in 0..10_000 {
            let x = r.next();
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn frame_layout_keeps_keyboard_on_screen() {
        let f = Frame::new(1920.0, 1080.0);
        assert!((f.hit_y + f.keyboard_h - 1080.0).abs() < 1e-3);
        assert!(f.keyboard_h <= 1080.0 * 0.25);
    }
}
