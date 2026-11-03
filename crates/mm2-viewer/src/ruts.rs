//! Lasting tyre ruts for the enhanced drawing ([`crate::graphics`]), none
//! of them the game's: each frame every tyre on the ground draws the line
//! it ran since the last into a map over the whole terrain, which the
//! ground's shader cuts and darkens and the grass shuns. Only drawn: the
//! ground the bikes ride is the game's.

use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};

/// The groove's width, ft, and how deep it is (of 1).
const WIDTH: f32 = 0.5;
const PASS: f32 = 0.7;
/// Frames a tyre may be off the ground and its rut still join up.
const HOP: u32 = 12;
/// Further than this between two frames is a reset or a respawn, ft.
const JUMP: f32 = 6.0;
/// Lines the buffer holds at first; it grows as needed.
const FIRST: usize = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Line {
    /// The line's ends on the map, 0 to 1.
    ends: [f32; 4],
    /// Its half width on the map, and how deep it is.
    shape: [f32; 4],
}

pub struct RutPainter {
    pipeline: wgpu::RenderPipeline,
    lines: wgpu::Buffer,
    capacity: usize,
    /// Where each bike's tyres last touched, drawing coordinates, and
    /// for how many frames since they have been off the ground.
    last: Vec<[Option<(Vec2, u32)>; 2]>,
    pending: Vec<Line>,
    /// The map's corner (x, z) and side, ft.
    origin: Vec2,
    size: f32,
}

impl RutPainter {
    /// Over a terrain `size` ft a side, its corner at `origin`.
    pub fn new(device: &wgpu::Device, origin: Vec2, size: f32) -> RutPainter {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ruts"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ruts.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ruts"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Line>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::enhance::RUT_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Max,
                        },
                        alpha: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::RED,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let lines = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ruts"),
            size: (FIRST * std::mem::size_of::<Line>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        RutPainter {
            pipeline,
            lines,
            capacity: FIRST,
            last: Vec::new(),
            pending: Vec::new(),
            origin,
            size,
        }
    }

    /// The map's corner and side for the shaders: x, z, side, on.
    pub fn uniform(&self) -> [f32; 4] {
        [self.origin.x, self.origin.y, self.size, 1.0]
    }

    /// Where each bike's tyres touch the ground this frame (game
    /// coordinates; none for a tyre in the air).
    pub fn tyres(&mut self, bikes: &[[Option<Vec3>; 2]]) {
        self.last.resize(bikes.len(), [None; 2]);
        // At least two texels across, else the map's texels would catch the
        // line only here and there; shallower to match.
        let half = (WIDTH * 0.5).max(1.2 * self.size / crate::enhance::RUT_SIDE as f32);
        for (last, now) in self.last.iter_mut().zip(bikes) {
            for k in 0..2 {
                let here = now[k].map(|p| {
                    let p = crate::renderer::from_game(p);
                    Vec2::new(p.x, p.z)
                });
                if let (Some((a, _)), Some(b)) = (last[k], here) {
                    let moved = a.distance(b);
                    if moved > 0.05 && moved < JUMP {
                        let (a, b) = ((a - self.origin) / self.size, (b - self.origin) / self.size);
                        self.pending.push(Line {
                            ends: [a.x, a.y, b.x, b.y],
                            shape: [half / self.size, PASS * (WIDTH * 0.5 / half), 0.0, 0.0],
                        });
                    }
                }
                // A tyre hopping on rough ground joins up again where it
                // lands, so the rut runs on unbroken; a tyre still resting
                // keeps its last place.
                last[k] = match (last[k], here) {
                    (Some((a, n)), None) => (n < HOP).then_some((a, n + 1)),
                    (Some((a, _)), Some(b)) if a.distance(b) <= 0.05 => Some((a, 0)),
                    (_, here) => here.map(|b| (b, 0)),
                };
            }
        }
    }

    /// Draws this frame's lines into the map `target`.
    pub fn paint(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        if self.pending.is_empty() {
            return;
        }
        if self.pending.len() > self.capacity {
            self.capacity = self.pending.len().next_power_of_two();
            self.lines = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ruts"),
                size: (self.capacity * std::mem::size_of::<Line>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.lines, 0, bytemuck::cast_slice(&self.pending));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ruts"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.lines.slice(..));
        pass.draw(0..6, 0..self.pending.len() as u32);
        self.pending.clear();
    }
}
