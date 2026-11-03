// Fullscreen dark gradient with a slowly breathing warm haze near the
// keyboard and a soft vignette. Outputs linear color.

struct Globals {
    resolution: vec2<f32>,
    time: f32,
    _pad: f32,
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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let top = vec3<f32>(0.0035, 0.0040, 0.0100);
    let bottom = vec3<f32>(0.0150, 0.0090, 0.0240);
    var col = mix(top, bottom, smoothstep(0.0, 1.0, in.uv.y));

    let aspect = globals.resolution.x / max(globals.resolution.y, 1.0);
    let p = vec2<f32>((in.uv.x - 0.5) * aspect * 0.6, in.uv.y - 1.0);
    let breathe = 0.5 + 0.5 * sin(globals.time * 0.4);
    let haze = exp(-dot(p, p) * 4.0) * (0.020 + 0.008 * breathe);
    col += vec3<f32>(1.0, 0.42, 0.12) * haze;

    let d = (in.uv - 0.5) * vec2<f32>(aspect, 1.0);
    col *= 1.0 - 0.55 * smoothstep(0.45, 1.25, length(d));

    // Tiny dither to break up 8-bit banding in the dark gradient.
    col += (hash12(in.pos.xy) - 0.5) * (1.5 / 255.0) * col * 8.0;

    return vec4<f32>(col, 1.0);
}
