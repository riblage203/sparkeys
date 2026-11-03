// Falling/rising note bars: anti-aliased rounded rectangles with a vertical
// gradient, a thin bright rim and an emissive boost while the note sounds.
//
// rect:   x, y (top-left), width, height in pixels
// color:  rgb = part color (linear), a = velocity brightness (0..1)
// params: x = corner radius px, y = active (0..1), z = hit flash (0..1)

struct Globals {
    screen: vec2<f32>,
    time: f32,
    dt: f32,
    hit_y: f32,
    unit: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) params: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // Pixel position relative to the rect center.
    @location(0) local: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) params: vec4<f32>,
};

// Grow the quad by this many pixels so the anti-aliased edge isn't clipped.
const PAD: f32 = 1.5;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let half_size = inst.rect.zw * 0.5;
    let center = inst.rect.xy + half_size;
    let local = (corner * 2.0 - 1.0) * (half_size + PAD);
    let px = center + local;
    var out: VsOut;
    out.pos = vec4<f32>(px.x / globals.screen.x * 2.0 - 1.0, 1.0 - px.y / globals.screen.y * 2.0, 0.0, 1.0);
    out.local = local;
    out.half_size = half_size;
    out.color = inst.color;
    out.params = inst.params;
    return out;
}

fn rounded_box_sdf(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + radius;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let radius = clamp(in.params.x, 0.0, min(in.half_size.x, in.half_size.y));
    let d = rounded_box_sdf(in.local, in.half_size, radius);
    let coverage = clamp(0.5 - d, 0.0, 1.0);
    if coverage <= 0.0 {
        discard;
    }

    let base = in.color.rgb;
    let brightness = in.color.a;
    let is_active = in.params.y;
    let flash = in.params.z;

    // 0 at the top of the bar, 1 at the bottom (the leading edge when falling).
    let v = clamp(in.local.y / max(in.half_size.y, 1.0) * 0.5 + 0.5, 0.0, 1.0);
    var fill = base * mix(0.38, 0.95, v * v) * brightness;
    // Faint horizontal sheen so wide notes don't look flat.
    let u = in.local.x / max(in.half_size.x, 1.0);
    fill *= 1.0 - 0.18 * u * u;

    // Thin bright rim just inside the edge.
    let rim = smoothstep(-2.2, -0.4, d);
    var color = mix(fill, base * (1.15 + 0.6 * brightness) + vec3<f32>(0.08), rim * 0.85);

    // Sounding notes glow (HDR, picked up by bloom); a quick flash on impact.
    color *= 1.0 + is_active * 0.9 + flash * 1.5;

    return vec4<f32>(color, coverage);
}
