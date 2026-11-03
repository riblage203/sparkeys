//! Per-frame uniforms shared by every scene pipeline (bind group 0).

use bytemuck::{Pod, Zeroable};

use super::pipeline::uniform_entry;

/// Mirrors `struct Globals` in the WGSL shaders.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
pub struct Globals {
    /// Target size in pixels.
    pub screen: [f32; 2],
    /// Wall-clock seconds since start (ambient animation).
    pub time: f32,
    /// Seconds since the previous frame.
    pub dt: f32,
    /// Y of the hit line in pixels.
    pub hit_y: f32,
    /// Width of a white key in pixels (resolution-independent sizing).
    pub unit: f32,
    /// x: background animation on (1) / off (0); y unused (pads to 16 bytes).
    pub flags: [f32; 2],
}

/// The uniform buffer and its bind group.
pub struct GlobalsBuffer {
    buffer: wgpu::Buffer,
    /// Layout for group 0 of every scene pipeline.
    pub layout: wgpu::BindGroupLayout,
    /// The bound buffer.
    pub bind_group: wgpu::BindGroup,
}

impl GlobalsBuffer {
    /// Allocates the buffer.
    pub fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[uniform_entry(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
            )],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });
        Self { buffer, layout, bind_group }
    }

    /// Uploads this frame's values.
    pub fn write(&self, queue: &wgpu::Queue, globals: &Globals) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(globals));
    }
}
