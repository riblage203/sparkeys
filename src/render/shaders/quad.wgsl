// Instanced rectangles for the keyboard, hit line and glows.
// color.rgb may exceed 1.0 (HDR, feeds bloom); color.a is opacity.
// params.x: vertical shade (brightness multiplied by mix(1, 1 - x, v)).
// params.y: vertical fade (opacity multiplied by mix(1 - y, 1, v)); 1 = transparent top.
// params.z: horizontal soft edges (0 = hard, 1 = fades to 0 at both sides).

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
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) params: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = inst.rect.xy + corner * inst.rect.zw;
    var out: VsOut;
    out.pos = vec4<f32>(px.x / globals.screen.x * 2.0 - 1.0, 1.0 - px.y / globals.screen.y * 2.0, 0.0, 1.0);
    out.color = inst.color;
    out.uv = corner;
    out.params = inst.params.xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let shade = mix(1.0, 1.0 - in.params.x, in.uv.y);
    let fade = mix(1.0 - in.params.y, 1.0, in.uv.y);
    let side = mix(1.0, 4.0 * in.uv.x * (1.0 - in.uv.x), in.params.z);
    return vec4<f32>(in.color.rgb * shade, in.color.a * fade * fade * side);
}
