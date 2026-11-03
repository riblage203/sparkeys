//! Animated background: dark gradient, drifting haze and vignette.

use super::pipeline::{HDR_FORMAT, RenderSpec, render_pipeline, shader};

/// Fullscreen-triangle background pass (reads only the shared globals).
pub struct Background {
    pipeline: wgpu::RenderPipeline,
}

impl Background {
    /// Builds the pipeline for the HDR target.
    pub fn new(device: &wgpu::Device, globals: &wgpu::BindGroupLayout) -> Self {
        let module = shader(device, "background.wgsl", include_str!("shaders/background.wgsl"));
        let pipeline = render_pipeline(
            device,
            &RenderSpec {
                label: "background",
                shader: &module,
                vs: "vs_main",
                fs: "fs_main",
                layouts: &[globals],
                buffers: &[],
                format: HDR_FORMAT,
                blend: None,
                topology: wgpu::PrimitiveTopology::TriangleList,
            },
        );
        Self { pipeline }
    }

    /// Draws the fullscreen triangle (globals must already be bound).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.draw(0..3, 0..1);
    }
}
