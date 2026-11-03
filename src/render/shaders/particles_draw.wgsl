// Ember/spark particles drawn as additive soft sprites straight from the
// storage buffer simulated by particles_update.wgsl (no CPU readback).

struct Globals {
    screen: vec2<f32>,
    time: f32,
    dt: f32,
    hit_y: f32,
    unit: f32,
    _pad: vec2<f32>,
};

struct Particle {
    pos: vec2<f32>,
    vel: vec2<f32>,
    // rgb: HDR color at birth; a: unused.
    color: vec4<f32>,
    life: f32,
    max_life: f32,
    size: f32,
    seed: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;

@group(1) @binding(0) var<storage, read> particles: array<Particle>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec3<f32>,
};

@vertex
fn vs_draw(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let p = particles[ii];
    var out: VsOut;
    if p.life <= 0.0 {
        // Dead: collapse to a degenerate point off screen.
        out.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        out.uv = vec2<f32>(0.0);
        out.color = vec3<f32>(0.0);
        return out;
    }
    let age = clamp(p.life / max(p.max_life, 1e-3), 0.0, 1.0); // 1 at birth -> 0 at death
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u)) * 2.0 - 1.0;
    // Stretch slightly along the velocity for a streaky spark look.
    let speed = length(p.vel);
    let dir = select(vec2<f32>(0.0, -1.0), p.vel / max(speed, 1e-3), speed > 1e-3);
    let side = vec2<f32>(-dir.y, dir.x);
    let radius = p.size * (0.45 + 0.55 * age);
    let stretch = 1.0 + min(speed * 0.004, 1.5);
    let offset = side * corner.x * radius + dir * corner.y * radius * stretch;
    let px = p.pos + offset;
    out.pos = vec4<f32>(px.x / globals.screen.x * 2.0 - 1.0, 1.0 - px.y / globals.screen.y * 2.0, 0.0, 1.0);
    out.uv = corner;

    // Hot white-gold at birth, cooling through the note color to deep red.
    let hot = p.color.rgb + vec3<f32>(1.2, 0.9, 0.5);
    let ember = p.color.rgb * vec3<f32>(1.0, 0.45, 0.25) * 0.6;
    var c = mix(ember, mix(p.color.rgb, hot, smoothstep(0.75, 1.0, age)), smoothstep(0.0, 0.6, age));
    // Fade out at the end of life, flicker a little.
    let flicker = 0.8 + 0.2 * sin(globals.time * 23.0 + p.seed * 40.0);
    c *= smoothstep(0.0, 0.35, age) * flicker;
    out.color = c;
    return out;
}

@fragment
fn fs_draw(in: VsOut) -> @location(0) vec4<f32> {
    let d2 = dot(in.uv, in.uv);
    if d2 > 1.0 {
        discard;
    }
    let core = exp(-d2 * 5.0);
    return vec4<f32>(in.color * core, 0.0);
}
