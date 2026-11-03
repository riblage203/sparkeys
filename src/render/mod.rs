//! GPU rendering: device setup and the per-frame pass graph.
//!
//! Frame graph:
//! 1. particle simulation (compute)
//! 2. HDR scene pass: background → note bars → keyboard/hit line → particles (additive)
//! 3. bloom: threshold + dual-filter down/up chain
//! 4. composite + tonemap into the swapchain (sRGB view); egui draws on top afterwards

mod background;
mod bloom;
mod globals;
mod gpu;
mod layout;
mod palette;
mod particles;
mod pipeline;
mod quads;
mod scene;

pub use gpu::Gpu;
pub use scene::SceneInput;

use background::Background;
use bloom::PostFx;
use globals::{Globals, GlobalsBuffer};
use layout::KeyboardLayout;
use palette::Palette;
use particles::Particles;
use quads::{Batch, instanced_pipeline};
use scene::{Frame, SceneOutput, SceneState};

/// Owns all scene pipelines, buffers and animation state.
pub struct Renderer {
    globals: GlobalsBuffer,
    background: Background,
    note_pipeline: wgpu::RenderPipeline,
    quad_pipeline: wgpu::RenderPipeline,
    notes: Batch,
    quads: Batch,
    particles: Particles,
    post: PostFx,
    scene: SceneState,
    layout: KeyboardLayout,
    palette: Palette,
}

impl Renderer {
    /// Creates all pipelines; the final image is written in [`Gpu::scene_format`].
    pub fn new(gpu: &Gpu) -> Self {
        let device = &gpu.device;
        let globals = GlobalsBuffer::new(device);
        Self {
            background: Background::new(device, &globals.layout),
            note_pipeline: instanced_pipeline(
                device,
                &globals.layout,
                "notes.wgsl",
                include_str!("shaders/notes.wgsl"),
            ),
            quad_pipeline: instanced_pipeline(device, &globals.layout, "quad.wgsl", include_str!("shaders/quad.wgsl")),
            notes: Batch::new(device, "note instances"),
            quads: Batch::new(device, "quad instances"),
            particles: Particles::new(device, &globals.layout),
            post: PostFx::new(device, gpu.scene_format()),
            scene: SceneState::new(),
            layout: KeyboardLayout::new(),
            palette: Palette::default_palette(),
            globals,
        }
    }

    /// Linear RGB color of part `track`, for UI swatches.
    pub fn track_color(&self, track: u16) -> [f32; 3] {
        self.palette.color(track)
    }

    /// Number of note bars drawn last frame (for the stats overlay).
    pub fn notes_drawn(&self) -> usize {
        self.notes.len()
    }

    /// Records the whole frame into `target` (a view in [`Gpu::scene_format`]).
    /// `time` is wall-clock seconds for ambient animation.
    pub fn render(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        time: f32,
        input: &SceneInput<'_>,
    ) {
        let (width, height) = (gpu.config.width, gpu.config.height);
        self.post.resize(&gpu.device, width, height);
        let frame = Frame::new(width as f32, height as f32);

        self.notes.clear();
        self.quads.clear();
        let mut out = SceneOutput { notes: &mut self.notes, quads: &mut self.quads, particles: &mut self.particles };
        self.scene.build(&mut out, &self.layout, &self.palette, input, &frame);

        self.globals.write(
            &gpu.queue,
            &Globals {
                screen: [frame.width, frame.height],
                time,
                dt: input.dt_s,
                hit_y: frame.hit_y,
                unit: frame.unit,
                _pad: [0.0; 2],
            },
        );
        self.notes.upload(&gpu.device, &gpu.queue);
        self.quads.upload(&gpu.device, &gpu.queue);
        self.particles.upload(&gpu.queue);
        self.particles.simulate(encoder, &self.globals.bind_group);

        let Some(hdr) = self.post.hdr_view() else { return };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: hdr,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals.bind_group, &[]);
            self.background.draw(&mut pass);
            self.notes.draw(&mut pass, &self.note_pipeline);
            self.quads.draw(&mut pass, &self.quad_pipeline);
            self.particles.draw(&mut pass);
        }
        self.post.run(&gpu.queue, encoder, target, input.visual.bloom);
    }
}
