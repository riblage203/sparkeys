//! GPU rendering: device setup and the per-frame pass graph.
//!
//! Phase 0 draws only the background straight to the swapchain. Later phases
//! render into an HDR target (notes, keyboard, particles), run bloom, then
//! tonemap to the swapchain before egui.

mod background;
mod gpu;

pub use gpu::Gpu;

use background::Background;

/// Owns all scene render passes.
pub struct Renderer {
    background: Background,
}

impl Renderer {
    /// Creates all pipelines for the surface format in `gpu`.
    pub fn new(gpu: &Gpu) -> Self {
        Self { background: Background::new(&gpu.device, gpu.config.format) }
    }

    /// Records the scene into `target`, clearing it first.
    pub fn render(&self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, time: f32) {
        self.background.prepare(&gpu.queue, gpu.config.width, gpu.config.height, time);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.background.draw(&mut pass);
    }
}
