// Fullscreen dark gradient with a slowly drifting warm haze above the
// keyboard, faint smoke-like noise and a soft vignette. Outputs linear HDR.

struct Globals {
    screen: vec2<f32>,
    time: f32,
    dt: f32,
    hit_y: f32,
    unit: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One oversized triangle covering the screen; uv (0,0) is top-left.
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn hash12(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash12(i);
    let b = hash12(i + vec2<f32>(1.0, 0.0));
    let c = hash12(i + vec2<f32>(0.0, 1.0));
    let d = hash12(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let aspect = globals.screen.x / max(globals.screen.y, 1.0);
    let hit_v = globals.hit_y / max(globals.screen.y, 1.0);

    let top = vec3<f32>(0.0030, 0.0035, 0.0090);
    let bottom = vec3<f32>(0.0140, 0.0080, 0.0200);
    var col = mix(top, bottom, smoothstep(0.0, hit_v, in.uv.y));

    // Smoke: two octaves of slowly rising value noise, strongest near the keys.
    let t = globals.time;
    let p = vec2<f32>(in.uv.x * aspect, in.uv.y) * 3.0;
    let smoke = value_noise(p + vec2<f32>(t * 0.02, t * 0.06)) * 0.65
        + value_noise(p * 2.3 - vec2<f32>(t * 0.03, t * 0.11)) * 0.35;
    let near_keys = smoothstep(0.0, hit_v, in.uv.y);
    col += vec3<f32>(0.012, 0.006, 0.010) * smoke * near_keys;

    // Warm haze just above the hit line, breathing slowly.
    let h = vec2<f32>((in.uv.x - 0.5) * aspect * 0.55, (in.uv.y - hit_v) * 2.2);
    let breathe = 0.5 + 0.5 * sin(t * 0.4);
    col += vec3<f32>(1.0, 0.40, 0.12) * exp(-dot(h, h) * 3.0) * (0.018 + 0.007 * breathe);

    let d = (in.uv - 0.5) * vec2<f32>(aspect, 1.0);
    col *= 1.0 - 0.6 * smoothstep(0.45, 1.3, length(d));

    return vec4<f32>(col, 1.0);
}
