//! Instanced rectangles: one CPU-filled instance list per frame, drawn by a
//! pipeline that interprets the per-instance parameters (flat quads, notes).

use bytemuck::{Pod, Zeroable};

use super::pipeline::{HDR_FORMAT, RenderSpec, render_pipeline, shader};

/// One rectangle as uploaded to the GPU. Field meaning depends on the shader;
/// see `quad.wgsl` and `notes.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct QuadInstance {
    /// x, y (top-left), width, height in pixels.
    pub rect: [f32; 4],
    /// Linear RGB (may exceed 1.0 to glow) + alpha or brightness.
    pub color: [f32; 4],
    /// Shader-specific parameters.
    pub params: [f32; 4],
}

const ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];

/// Builds an alpha-blended instanced pipeline drawing [`QuadInstance`]s into the HDR target.
pub fn instanced_pipeline(
    device: &wgpu::Device,
    globals: &wgpu::BindGroupLayout,
    label: &str,
    source: &str,
) -> wgpu::RenderPipeline {
    let module = shader(device, label, source);
    render_pipeline(
        device,
        &RenderSpec {
            label,
            shader: &module,
            vs: "vs_main",
            fs: "fs_main",
            layouts: &[globals],
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<QuadInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &ATTRIBUTES,
            })],
            format: HDR_FORMAT,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            topology: wgpu::PrimitiveTopology::TriangleStrip,
        },
    )
}

/// Reusable instance list + GPU buffer. Reallocates only when a frame needs
/// more instances than ever before.
pub struct Batch {
    label: &'static str,
    buffer: wgpu::Buffer,
    capacity: usize,
    instances: Vec<QuadInstance>,
}

impl Batch {
    /// An empty batch with room for 4096 instances.
    pub fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let capacity = 4096;
        Self {
            label,
            buffer: create_buffer(device, label, capacity),
            capacity,
            instances: Vec::with_capacity(capacity),
        }
    }

    /// Starts a new frame.
    pub fn clear(&mut self) {
        self.instances.clear();
    }

    /// Number of instances this frame.
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Adds an instance; empty rectangles are skipped.
    pub fn push(&mut self, rect: [f32; 4], color: [f32; 4], params: [f32; 4]) {
        if rect[2] > 0.0 && rect[3] > 0.0 {
            self.instances.push(QuadInstance { rect, color, params });
        }
    }

    /// Uploads this frame's instances.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.instances.len() > self.capacity {
            self.capacity = self.instances.len().next_power_of_two();
            self.buffer = create_buffer(device, self.label, self.capacity);
        }
        if !self.instances.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.instances));
        }
    }

    /// Draws all instances with `pipeline` (globals must already be bound).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline) {
        if self.instances.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, self.buffer.slice(..));
        // Bounded by the buffer capacity, far below u32::MAX.
        pass.draw(0..4, 0..self.instances.len() as u32);
    }
}

fn create_buffer(device: &wgpu::Device, label: &str, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (capacity * std::mem::size_of::<QuadInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
