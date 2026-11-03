// Ember/spark particle simulation (compute). Drawn by particles_draw.wgsl.

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

@group(1) @binding(0) var<storage, read_write> particles: array<Particle>;

@compute @workgroup_size(256)
fn update(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= arrayLength(&particles) {
        return;
    }
    var p = particles[i];
    if p.life <= 0.0 {
        return;
    }
    let dt = globals.dt;
    let t = globals.time;
    let scale = globals.screen.y / 1080.0;

    // Cheap turbulence: layered sines of position, time and a per-particle seed.
    let s = p.seed * 6.2831;
    let swirl = vec2<f32>(
        sin(p.pos.y * 0.012 + t * 1.6 + s) + 0.5 * sin(p.pos.y * 0.037 - t * 2.9 + s * 1.7),
        0.35 * cos(p.pos.x * 0.010 + t * 1.2 + s * 0.6)
    );
    p.vel += swirl * 90.0 * scale * dt;
    // Buoyancy: embers keep drifting upward (screen y grows downward).
    p.vel.y -= 55.0 * scale * dt;
    // Air drag.
    p.vel *= pow(0.42, dt);
    p.pos += p.vel * dt;
    p.life -= dt;
    particles[i] = p;
}
