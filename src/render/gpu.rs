//! wgpu device and window surface management.

use std::sync::Arc;

use anyhow::Context as _;
use winit::window::Window;

/// The GPU device, queue and the window's swapchain surface.
pub struct Gpu {
    /// Swapchain surface; `'static` because it holds an `Arc<Window>`.
    pub surface: wgpu::Surface<'static>,
    /// Logical device.
    pub device: wgpu::Device,
    /// Submission queue.
    pub queue: wgpu::Queue,
    /// Current surface configuration (size, format, present mode).
    pub config: wgpu::SurfaceConfiguration,
    /// Adapter description shown in the UI.
    pub adapter_info: wgpu::AdapterInfo,
}

impl Gpu {
    /// Creates the device and configures a vsynced sRGB surface for `window`.
    pub fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone()).context("creating window surface")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .context("no compatible GPU adapter found")?;
        let adapter_info = adapter.get_info();
        log::info!("GPU: {} ({:?})", adapter_info.name, adapter_info.backend);

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("sparkeys device"),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .context("creating GPU device")?;

        let caps = surface.get_capabilities(&adapter);
        // Our shaders output linear color; an sRGB swapchain does the encoding for free.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| caps.formats.first().copied())
            .context("surface reports no supported formats")?;
        let alpha_mode = caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto);

        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        Ok(Self { surface, device, queue, config, adapter_info })
    }

    /// Reconfigures the surface for a new window size. Zero sizes (minimized) are ignored.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.reconfigure();
    }

    /// Re-applies the current configuration, e.g. after the surface became outdated.
    pub fn reconfigure(&self) {
        self.surface.configure(&self.device, &self.config);
    }
}
