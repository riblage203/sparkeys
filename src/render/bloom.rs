//! HDR scene target, dual-filter bloom chain and the final tonemap composite.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt as _;

use super::pipeline::{
    ADDITIVE, HDR_FORMAT, RenderSpec, render_pipeline, sampler_entry, shader, texture_entry, uniform_entry,
};

/// Most blur levels; each halves the resolution.
const MAX_LEVELS: usize = 7;
/// Stop halving below this size (pixels).
const MIN_LEVEL_SIZE: u32 = 8;
/// Brightness where bloom starts (HDR linear).
const THRESHOLD: f32 = 0.9;
/// Width of the soft transition around the threshold.
const KNEE: f32 = 0.45;
/// Normalizes the summed blur levels so intensity 1.0 is a pleasant default.
const STRENGTH: f32 = 0.35;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    a: [f32; 4],
    b: [f32; 4],
}

/// One offscreen color target.
struct Target {
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl Target {
    fn new(device: &wgpu::Device, label: &str, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self { view: texture.create_view(&Default::default()), size: (width, height) }
    }

    fn texel(&self) -> [f32; 2] {
        [1.0 / self.size.0 as f32, 1.0 / self.size.1 as f32]
    }
}

/// A fullscreen pass: reads `bind_group`, writes `dst` (index into levels, or the output).
struct Pass {
    bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
}

/// Size-dependent resources, rebuilt on resize.
struct Chain {
    hdr: Target,
    levels: Vec<Target>,
    prefilter: Pass,
    downs: Vec<Pass>,
    /// `ups[i]` writes level `i` from level `i + 1`.
    ups: Vec<Pass>,
    composite: Pass,
}

/// HDR target plus bloom and tonemapping.
pub struct PostFx {
    sampler: wgpu::Sampler,
    pass_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    prefilter: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    chain: Option<Chain>,
}

impl PostFx {
    /// Creates the pipelines; `output_format` is the swapchain view format.
    pub fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        let module = shader(device, "bloom.wgsl", include_str!("shaders/bloom.wgsl"));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bloom"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let fragment_uniform = |b| uniform_entry(b, wgpu::ShaderStages::FRAGMENT);
        let pass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom pass"),
            entries: &[texture_entry(0), sampler_entry(1), fragment_uniform(2)],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom composite"),
            entries: &[texture_entry(0), sampler_entry(1), fragment_uniform(2), texture_entry(3)],
        });

        let make = |label: &str, fs: &str, layout: &wgpu::BindGroupLayout, format, blend| {
            render_pipeline(
                device,
                &RenderSpec {
                    label,
                    shader: &module,
                    vs: "vs_fullscreen",
                    fs,
                    layouts: &[layout],
                    buffers: &[],
                    format,
                    blend,
                    topology: wgpu::PrimitiveTopology::TriangleList,
                },
            )
        };
        Self {
            prefilter: make("bloom prefilter", "fs_prefilter", &pass_layout, HDR_FORMAT, None),
            down: make("bloom down", "fs_down", &pass_layout, HDR_FORMAT, None),
            up: make("bloom up", "fs_up", &pass_layout, HDR_FORMAT, Some(ADDITIVE)),
            composite: make("bloom composite", "fs_composite", &composite_layout, output_format, None),
            sampler,
            pass_layout,
            composite_layout,
            chain: None,
        }
    }

    /// Ensures the targets match the output size. Cheap when unchanged.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if self.chain.as_ref().is_some_and(|c| c.hdr.size == (width, height)) {
            return;
        }
        let hdr = Target::new(device, "hdr scene", width, height);
        let mut levels = Vec::new();
        let (mut w, mut h) = (width / 2, height / 2);
        while levels.len() < MAX_LEVELS && w >= MIN_LEVEL_SIZE && h >= MIN_LEVEL_SIZE {
            levels.push(Target::new(device, "bloom level", w, h));
            w /= 2;
            h /= 2;
        }
        if levels.is_empty() {
            levels.push(Target::new(device, "bloom level", (width / 2).max(1), (height / 2).max(1)));
        }

        let pass = |src: &Target, texel: [f32; 2]| self.make_pass(device, &self.pass_layout, src, None, texel);
        let prefilter = pass(&hdr, hdr.texel());
        let downs = (1..levels.len()).map(|i| pass(&levels[i - 1], levels[i - 1].texel())).collect();
        let ups = (0..levels.len() - 1).map(|i| pass(&levels[i + 1], levels[i + 1].texel())).collect();
        let composite = self.make_pass(device, &self.composite_layout, &hdr, Some(&levels[0]), levels[0].texel());
        self.chain = Some(Chain { hdr, levels, prefilter, downs, ups, composite });
    }

    fn make_pass(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        src: &Target,
        bloom: Option<&Target>,
        texel: [f32; 2],
    ) -> Pass {
        let initial = Params { a: [texel[0], texel[1], THRESHOLD, KNEE], b: [1.0, 0.0, 0.0, 0.0] };
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bloom params"),
            contents: bytemuck::bytes_of(&initial),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src.view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: params.as_entire_binding() },
        ];
        if let Some(bloom) = bloom {
            entries
                .push(wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&bloom.view) });
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bloom pass"),
            layout,
            entries: &entries,
        });
        Pass { bind_group, params }
    }

    /// The HDR scene target to render into. Call [`Self::resize`] first.
    pub fn hdr_view(&self) -> Option<&wgpu::TextureView> {
        self.chain.as_ref().map(|c| &c.hdr.view)
    }

    /// Runs bloom and writes the tonemapped image to `output`.
    pub fn run(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        intensity: f32,
    ) {
        let Some(chain) = &self.chain else { return };
        let texel = chain.levels[0].texel();
        let params =
            Params { a: [texel[0], texel[1], THRESHOLD, KNEE], b: [intensity.max(0.0) * STRENGTH, 0.0, 0.0, 0.0] };
        queue.write_buffer(&chain.composite.params, 0, bytemuck::bytes_of(&params));

        let draw = |encoder: &mut wgpu::CommandEncoder, pipeline, pass: &Pass, view, load| {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(pipeline);
            rp.set_bind_group(0, &pass.bind_group, &[]);
            rp.draw(0..3, 0..1);
        };
        let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);

        if intensity > 0.0 {
            draw(encoder, &self.prefilter, &chain.prefilter, &chain.levels[0].view, clear);
            for (i, pass) in chain.downs.iter().enumerate() {
                draw(encoder, &self.down, pass, &chain.levels[i + 1].view, clear);
            }
            for (i, pass) in chain.ups.iter().enumerate().rev() {
                draw(encoder, &self.up, pass, &chain.levels[i].view, wgpu::LoadOp::Load);
            }
        }
        draw(encoder, &self.composite, &chain.composite, output, clear);
    }
}
