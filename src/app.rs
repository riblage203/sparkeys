//! Application shell: window lifecycle, input shortcuts and the frame loop.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::config::Config;
use crate::render::{Gpu, Renderer};
use crate::ui::{EguiLayer, OverlayState, draw_overlay};

/// Top-level winit application.
pub struct App {
    config: Config,
    notices: Vec<String>,
    running: Option<Running>,
    fatal: Option<anyhow::Error>,
}

/// State that exists once the window and GPU are up.
struct Running {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: Renderer,
    egui: EguiLayer,
    start: Instant,
    stats: FrameStats,
}

impl App {
    /// Loads settings and prepares an app with no window yet (created on `resumed`).
    pub fn new() -> Self {
        let (config, warning) = Config::load();
        Self { config, notices: warning.into_iter().collect(), running: None, fatal: None }
    }

    /// The unrecoverable startup error, if any, after the event loop exits.
    pub fn into_fatal_error(self) -> Option<anyhow::Error> {
        self.fatal
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<Running> {
        let attrs = Window::default_attributes()
            .with_title("sparkeys")
            .with_inner_size(LogicalSize::new(self.config.window.width, self.config.window.height))
            .with_min_inner_size(LogicalSize::new(640, 360));
        let window = Arc::new(event_loop.create_window(attrs)?);
        let gpu = Gpu::new(window.clone())?;
        let renderer = Renderer::new(&gpu);
        let egui = EguiLayer::new(&window, &gpu);
        Ok(Running { window, gpu, renderer, egui, start: Instant::now(), stats: FrameStats::new() })
    }

    fn handle_shortcut(&mut self, event: &KeyEvent) {
        if event.state != ElementState::Pressed || event.repeat {
            return;
        }
        let PhysicalKey::Code(code) = event.physical_key else { return };
        match code {
            KeyCode::KeyH => self.config.ui.visible = !self.config.ui.visible,
            KeyCode::F3 => self.config.ui.show_fps = !self.config.ui.show_fps,
            KeyCode::F11 => {
                if let Some(running) = &self.running {
                    let window = &running.window;
                    let next = match window.fullscreen() {
                        Some(_) => None,
                        None => Some(Fullscreen::Borderless(None)),
                    };
                    window.set_fullscreen(next);
                }
            }
            _ => {}
        }
    }

    fn save_config(&mut self) {
        if let Some(running) = &self.running
            && running.window.fullscreen().is_none()
        {
            let size = running.window.inner_size().to_logical::<u32>(running.window.scale_factor());
            self.config.window.width = size.width;
            self.config.window.height = size.height;
        }
        if let Err(err) = self.config.save() {
            log::warn!("could not save settings: {err:#}");
        }
    }

    fn redraw(&mut self) {
        let Some(running) = &mut self.running else { return };
        running.stats.tick();

        let frame = match running.gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                running.gpu.reconfigure();
                frame
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                running.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                running.gpu.reconfigure();
                running.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("surface validation error while acquiring frame");
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            running.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        let time = running.start.elapsed().as_secs_f32();
        running.renderer.render(&running.gpu, &mut encoder, &view, time);

        let mut overlay = OverlayState {
            ui_visible: self.config.ui.visible,
            show_fps: self.config.ui.show_fps,
            fps: running.stats.fps(),
            frame_ms: running.stats.frame_ms(),
            adapter: &running.gpu.adapter_info.name,
            notices: &mut self.notices,
        };
        let extra =
            running.egui.draw(&running.window, &running.gpu, &mut encoder, &view, |ui| draw_overlay(ui, &mut overlay));

        running.gpu.queue.submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        running.window.pre_present_notify();
        running.gpu.queue.present(frame);
        running.window.request_redraw();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        match self.init(event_loop) {
            Ok(running) => {
                running.window.request_redraw();
                self.running = Some(running);
            }
            Err(err) => {
                log::error!("startup failed: {err:#}");
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Error)
                    .set_title("sparkeys could not start")
                    .set_description(format!("{err:#}"))
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                self.fatal = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let consumed = match &mut self.running {
            Some(running) => running.egui.on_window_event(&running.window, &event),
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => {
                self.save_config();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(running) = &mut self.running {
                    running.gpu.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if !consumed => self.handle_shortcut(&event),
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

/// Exponentially smoothed frame timing for the FPS counter.
struct FrameStats {
    last: Instant,
    avg_dt: f32,
}

impl FrameStats {
    fn new() -> Self {
        Self { last: Instant::now(), avg_dt: 1.0 / 60.0 }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        self.avg_dt += (dt - self.avg_dt) * 0.05;
    }

    fn fps(&self) -> f32 {
        1.0 / self.avg_dt.max(1e-6)
    }

    fn frame_ms(&self) -> f32 {
        self.avg_dt * 1000.0
    }
}
