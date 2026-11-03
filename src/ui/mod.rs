//! egui overlay: integration with winit/wgpu plus the app's panels.

mod panels;

pub use panels::{FALL_SECONDS_RANGE, OverlayState, draw_overlay};

use winit::window::Window;

use crate::render::Gpu;

/// egui context, its winit input state and its wgpu renderer.
pub struct EguiLayer {
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
}

impl EguiLayer {
    /// Creates the egui integration for `window`, rendering to [`Gpu::ui_format`].
    pub fn new(window: &Window, gpu: &Gpu) -> Self {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::Theme::Dark);
        let state = egui_winit::State::new(
            ctx.clone(),
            ctx.viewport_id(),
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let renderer = egui_wgpu::Renderer::new(&gpu.device, gpu.ui_format(), egui_wgpu::RendererOptions::default());
        Self { ctx, state, renderer }
    }

    /// Feeds a window event to egui. Returns `true` if egui consumed it
    /// (e.g. typing in a text field), in which case app shortcuts should be skipped.
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    /// Runs the UI closure and records egui's draw on top of `target`.
    ///
    /// Returns extra command buffers produced by egui paint callbacks; submit
    /// them before `encoder`.
    pub fn draw(
        &mut self,
        window: &Window,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        build_ui: impl FnMut(&mut egui::Ui),
    ) -> Vec<wgpu::CommandBuffer> {
        let raw_input = self.state.take_egui_input(window);
        let output = self.ctx.run_ui(raw_input, build_ui);
        self.state.handle_platform_output(window, output.platform_output);

        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: output.pixels_per_point,
        };

        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        let extra = self.renderer.update_buffers(&gpu.device, &gpu.queue, encoder, &primitives, &screen);

        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                // egui-wgpu requires a 'static pass; the encoder is not touched until the pass drops.
                .forget_lifetime();
            self.renderer.render(&mut pass, &primitives, &screen);
        }

        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        extra
    }
}
