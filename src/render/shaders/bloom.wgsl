// HDR post-processing: threshold + dual-filter (Kawase-style) bloom chain,
// then composite and tonemap to the swapchain.

struct Params {
    // xy: texel size of the source texture, z: threshold, w: soft knee.
    a: vec4<f32>,
    // x: bloom intensity.
    b: vec4<f32>,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> params: Params;
// Composite only: the blurred bloom (level 0).
@group(0) @binding(3) var bloom_tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> VsOut {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

// 5-tap dual-filter downsample: center ×4 + four diagonal bilinear taps.
fn downsample(t: texture_2d<f32>, uv: vec2<f32>, h: vec2<f32>) -> vec3<f32> {
    var sum = textureSample(t, samp, uv).rgb * 4.0;
    sum += textureSample(t, samp, uv - h).rgb;
    sum += textureSample(t, samp, uv + h).rgb;
    sum += textureSample(t, samp, uv + vec2<f32>(h.x, -h.y)).rgb;
    sum += textureSample(t, samp, uv - vec2<f32>(h.x, -h.y)).rgb;
    return sum / 8.0;
}

// 8-tap dual-filter upsample (tent).
fn upsample(t: texture_2d<f32>, uv: vec2<f32>, h: vec2<f32>) -> vec3<f32> {
    var sum = textureSample(t, samp, uv + vec2<f32>(-2.0 * h.x, 0.0)).rgb;
    sum += textureSample(t, samp, uv + vec2<f32>(-h.x, h.y)).rgb * 2.0;
    sum += textureSample(t, samp, uv + vec2<f32>(0.0, 2.0 * h.y)).rgb;
    sum += textureSample(t, samp, uv + vec2<f32>(h.x, h.y)).rgb * 2.0;
    sum += textureSample(t, samp, uv + vec2<f32>(2.0 * h.x, 0.0)).rgb;
    sum += textureSample(t, samp, uv + vec2<f32>(h.x, -h.y)).rgb * 2.0;
    sum += textureSample(t, samp, uv + vec2<f32>(0.0, -2.0 * h.y)).rgb;
    sum += textureSample(t, samp, uv + vec2<f32>(-h.x, -h.y)).rgb * 2.0;
    return sum / 12.0;
}

@fragment
fn fs_prefilter(in: VsOut) -> @location(0) vec4<f32> {
    let c = downsample(src, in.uv, params.a.xy);
    // Soft-knee threshold on the brightest channel.
    let threshold = params.a.z;
    let knee = max(params.a.w, 1e-4);
    let brightness = max(c.r, max(c.g, c.b));
    var soft = clamp(brightness - threshold + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee);
    let contribution = max(soft, brightness - threshold) / max(brightness, 1e-4);
    // Clamp single hot pixels (tiny sparks) so the bloom doesn't flicker.
    let weight = 1.0 / (1.0 + max(brightness * contribution - 8.0, 0.0));
    return vec4<f32>(c * contribution * weight, 1.0);
}

@fragment
fn fs_down(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(downsample(src, in.uv, params.a.xy), 1.0);
}

@fragment
fn fs_up(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(upsample(src, in.uv, params.a.xy), 1.0);
}

// Highlight compression from the Khronos PBR Neutral tonemapper (without its
// black-level toe, which would crush our near-black background): linear below
// 0.76, then compresses and desaturates toward white so bright sparks roll off
// to white-hot.
fn tonemap_neutral(color_in: vec3<f32>) -> vec3<f32> {
    let start = 0.76;
    let desaturation = 0.15;
    var color = color_in;
    let peak = max(color.r, max(color.g, color.b));
    if peak < start {
        return color;
    }
    let d = 1.0 - start;
    let new_peak = 1.0 - d * d / (peak + d - start);
    color *= new_peak / peak;
    let g = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(color, vec3<f32>(new_peak), g);
}

fn hash12(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

@fragment
fn fs_composite(in: VsOut) -> @location(0) vec4<f32> {
    let hdr = textureLoad(src, vec2<i32>(in.pos.xy), 0).rgb;
    let bloom = upsample(bloom_tex, in.uv, params.a.xy);
    let color = tonemap_neutral(max(hdr + bloom * params.b.x, vec3<f32>(0.0)));
    // Sub-LSB dither against banding in dark gradients and bloom halos.
    let n = (hash12(in.pos.xy) - 0.5) / 255.0;
    return vec4<f32>(color + n * (0.5 + color), 1.0);
}
