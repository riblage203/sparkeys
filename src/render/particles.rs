//! GPU particle pool: fixed-size ring buffer of particles simulated by a
//! compute shader and drawn additively.
//!
//! New particles overwrite the oldest slots (a recycled pool with no free
//! list). The CPU only writes freshly spawned particles each frame, from a
//! reused staging vector, so steady state allocates nothing.

use bytemuck::{Pod, Zeroable};

use super::pipeline::{ADDITIVE, HDR_FORMAT, RenderSpec, render_pipeline, shader};

/// Hard cap on live particles.
pub const MAX_PARTICLES: usize = 100_000;
/// Most particles spawned in a single frame; the rest of a huge burst is dropped.
const MAX_SPAWN_PER_FRAME: usize = 8192;
/// Compute workgroup size (must match `particles_update.wgsl`).
const WORKGROUP: u32 = 256;

/// Mirrors `struct Particle` in the particle shaders (48 bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, Default)]
pub struct Particle {
    /// Position in pixels.
    pub pos: [f32; 2],
    /// Velocity in pixels per second.
    pub vel: [f32; 2],
    /// HDR color at birth (alpha unused).
    pub color: [f32; 4],
    /// Remaining life in seconds.
    pub life: f32,
    /// Initial life, for fading.
    pub max_life: f32,
    /// Radius in pixels.
    pub size: f32,
    /// Random 0..1 for per-particle variation.
    pub seed: f32,
}

/// The particle buffer, its pipelines and this frame's spawn list.
pub struct Particles {
    buffer: wgpu::Buffer,
    compute: wgpu::ComputePipeline,
    draw: wgpu::RenderPipeline,
    compute_group: wgpu::BindGroup,
    draw_group: wgpu::BindGroup,
    spawn: Vec<Particle>,
    cursor: usize,
    /// Whether anything was ever spawned (skip GPU work until then).
    active: bool,
}

impl Particles {
    /// Allocates the pool (zeroed = all dead).
    pub fn new(device: &wgpu::Device, globals: &wgpu::BindGroupLayout) -> Self {
        let update_module = shader(device, "particles_update.wgsl", include_str!("shaders/particles_update.wgsl"));
        let draw_module = shader(device, "particles_draw.wgsl", include_str!("shaders/particles_draw.wgsl"));
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particles"),
            size: (MAX_PARTICLES * std::mem::size_of::<Particle>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let storage_layout = |read_only: bool, visibility| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("particles storage"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            })
        };
        let compute_layout = storage_layout(false, wgpu::ShaderStages::COMPUTE);
        let draw_layout = storage_layout(true, wgpu::ShaderStages::VERTEX);
        let group = |layout: &wgpu::BindGroupLayout| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("particles storage"),
                layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            })
        };
        let compute_group = group(&compute_layout);
        let draw_group = group(&draw_layout);

        let compute_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particles update"),
            bind_group_layouts: &[Some(globals), Some(&compute_layout)],
            immediate_size: 0,
        });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("particles update"),
            layout: Some(&compute_pipeline_layout),
            module: &update_module,
            entry_point: Some("update"),
            compilation_options: Default::default(),
            cache: None,
        });
        let draw = render_pipeline(
            device,
            &RenderSpec {
                label: "particles draw",
                shader: &draw_module,
                vs: "vs_draw",
                fs: "fs_draw",
                layouts: &[globals, &draw_layout],
                buffers: &[],
                format: HDR_FORMAT,
                blend: Some(ADDITIVE),
                topology: wgpu::PrimitiveTopology::TriangleStrip,
            },
        );

        Self {
            buffer,
            compute,
            draw,
            compute_group,
            draw_group,
            spawn: Vec::with_capacity(MAX_SPAWN_PER_FRAME),
            cursor: 0,
            active: false,
        }
    }

    /// Queues a particle for this frame (dropped if the per-frame budget is spent).
    pub fn emit(&mut self, particle: Particle) {
        if self.spawn.len() < MAX_SPAWN_PER_FRAME {
            self.spawn.push(particle);
        }
    }

    /// Writes this frame's spawns into the ring, overwriting the oldest particles.
    pub fn upload(&mut self, queue: &wgpu::Queue) {
        if self.spawn.is_empty() {
            return;
        }
        self.active = true;
        let size = std::mem::size_of::<Particle>();
        let mut rest = self.spawn.as_slice();
        while !rest.is_empty() {
            let n = rest.len().min(MAX_PARTICLES - self.cursor);
            queue.write_buffer(&self.buffer, (self.cursor * size) as u64, bytemuck::cast_slice(&rest[..n]));
            self.cursor = (self.cursor + n) % MAX_PARTICLES;
            rest = &rest[n..];
        }
        self.spawn.clear();
    }

    /// Records the simulation step (globals must be up to date).
    pub fn simulate(&self, encoder: &mut wgpu::CommandEncoder, globals: &wgpu::BindGroup) {
        if !self.active {
            return;
        }
        let mut pass = encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("particles"), timestamp_writes: None });
        pass.set_pipeline(&self.compute);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(1, &self.compute_group, &[]);
        // MAX_PARTICLES is a small constant; the cast cannot truncate.
        pass.dispatch_workgroups((MAX_PARTICLES as u32).div_ceil(WORKGROUP), 1, 1);
    }

    /// Draws all particles additively (globals bound at group 0).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if !self.active {
            return;
        }
        pass.set_pipeline(&self.draw);
        pass.set_bind_group(1, &self.draw_group, &[]);
        pass.draw(0..4, 0..MAX_PARTICLES as u32);
    }
}
