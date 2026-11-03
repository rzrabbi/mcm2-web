//! A crowd for the enhanced drawing ([`crate::graphics`]), none of it the
//! game's: little 3D spectators standing along an outdoor course and at a
//! stadium's rail, each dressed and sized a little differently, swaying as
//! they wait and jumping with their arms up as the bikes go by
//! (`crowd.wgsl`). Only drawn.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wgpu::util::DeviceExt;

/// Spectators at most.
pub const MOST: usize = 9000;
/// Bikes the crowd watches for.
const BIKES: usize = 8;
/// The crowd's blocks are this many ft a side.
const BLOCK: f32 = 120.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    /// What it is (0 trousers, 1 shirt, 2 skin, 3 hair) and which arm it
    /// belongs to (0 none, 1 left, 2 right).
    part: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    /// Where (drawing coordinates) and which way it faces (radians).
    place: [f32; 4],
    /// Its shirt's colour, and a number of its own.
    shirt: [f32; 4],
    /// Its trousers' colour, and how tall it is (of the usual).
    legs: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Bikes {
    /// Each bike's place (drawing coordinates), w 1 when there is one.
    at: [[f32; 4]; BIKES],
}

/// A box from `lo` to `hi` (ft, the spectator's own space).
fn cuboid(out: &mut Vec<Vertex>, lo: Vec3, hi: Vec3, part: [u32; 2]) {
    // Each face's way out and its sides, u × v = out, so the corners
    // turn counter-clockwise seen from outside.
    let faces = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (-Vec3::X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (-Vec3::Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (-Vec3::Z, Vec3::Y, Vec3::X),
    ];
    let mid = (lo + hi) / 2.0;
    let half = (hi - lo) / 2.0;
    for (n, u, v) in faces {
        let c = mid + n * half;
        let (du, dv) = (u * half, v * half);
        let corners = [c - du - dv, c + du - dv, c + du + dv, c - du + dv];
        for k in [0, 1, 2, 0, 2, 3] {
            out.push(Vertex {
                position: corners[k].into(),
                normal: n.into(),
                part,
            });
        }
    }
}

/// One spectator, facing +z.
fn figure() -> Vec<Vertex> {
    let mut v = Vec::new();
    // Legs, body, head and hair.
    cuboid(&mut v, Vec3::new(-0.62, 0.0, -0.25), Vec3::new(-0.08, 2.8, 0.25), [0, 0]);
    cuboid(&mut v, Vec3::new(0.08, 0.0, -0.25), Vec3::new(0.62, 2.8, 0.25), [0, 0]);
    cuboid(&mut v, Vec3::new(-0.7, 2.75, -0.35), Vec3::new(0.7, 4.85, 0.35), [1, 0]);
    cuboid(&mut v, Vec3::new(-0.15, 4.85, -0.15), Vec3::new(0.15, 5.0, 0.15), [2, 0]);
    cuboid(&mut v, Vec3::new(-0.36, 5.0, -0.36), Vec3::new(0.36, 5.72, 0.38), [2, 0]);
    cuboid(&mut v, Vec3::new(-0.4, 5.55, -0.42), Vec3::new(0.4, 5.85, 0.3), [3, 0]);
    // Arms, hanging from the shoulders (sleeves, then hands).
    for (side, x) in [(1u32, -0.9f32), (2, 0.9)] {
        cuboid(&mut v, Vec3::new(x - 0.2, 3.3, -0.2), Vec3::new(x + 0.2, 4.8, 0.2), [1, side]);
        cuboid(&mut v, Vec3::new(x - 0.17, 2.75, -0.17), Vec3::new(x + 0.17, 3.3, 0.17), [2, side]);
    }
    v
}

pub struct Crowd {
    pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    vertex_count: u32,
    instances: wgpu::Buffer,
    count: u32,
    /// The crowd in blocks of the track (crate::cull): each block's
    /// bounds and its spectators; and those in view this frame.
    blocks: Vec<(Vec3, Vec3, std::ops::Range<u32>)>,
    runs: Vec<std::ops::Range<u32>>,
    bikes: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl Crowd {
    /// Spectators at `spots` (game coordinates, and which way each faces
    /// as a level direction), with the terrain's own group's layout and
    /// the enhanced drawing's shared one.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        spots: &[(Vec3, Vec3)],
        terrain_layout: &wgpu::BindGroupLayout,
        shared_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        samples: u32,
        depth_format: wgpu::TextureFormat,
    ) -> Crowd {
        let mut random = 0x6c07_8965u32;
        let mut next = move || {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            (random >> 8) as f32 / (1u32 << 24) as f32
        };
        // Shirts in team and bright colours, white and black; jeans and
        // dark trousers.
        let shirts = [
            [0.6, 0.04, 0.03], [0.03, 0.12, 0.5], [0.75, 0.75, 0.72], [0.03, 0.03, 0.03], [0.8, 0.35, 0.02],
            [0.06, 0.35, 0.08], [0.7, 0.6, 0.05], [0.3, 0.05, 0.35], [0.2, 0.2, 0.22], [0.02, 0.3, 0.45],
        ];
        let legs = [[0.04, 0.07, 0.16], [0.02, 0.02, 0.025], [0.25, 0.2, 0.13], [0.08, 0.1, 0.14], [0.35, 0.33, 0.3]];
        // Thinned out evenly when there are too many.
        let every = spots.len().div_ceil(MOST).max(1);
        let instances: Vec<Instance> = spots
            .iter()
            .step_by(every)
            .map(|&(at, look)| {
                let p = crate::renderer::from_game(at);
                let l = crate::renderer::from_game(look);
                let shirt = shirts[(next() * shirts.len() as f32) as usize % shirts.len()];
                let leg = legs[(next() * legs.len() as f32) as usize % legs.len()];
                let tone = 0.8 + next() * 0.4;
                Instance {
                    place: [p.x, p.y, p.z, l.x.atan2(l.z) + (next() - 0.5) * 0.6],
                    shirt: [shirt[0] * tone, shirt[1] * tone, shirt[2] * tone, next()],
                    legs: [leg[0], leg[1], leg[2], 0.88 + next() * 0.22],
                }
            })
            .collect();
        // In blocks, nearby spectators together, so those out of view are
        // left out.
        let block = |i: &Instance| ((i.place[2] / BLOCK).floor() as i32, (i.place[0] / BLOCK).floor() as i32);
        let mut instances = instances;
        instances.sort_by_key(block);
        let mut blocks: Vec<(Vec3, Vec3, std::ops::Range<u32>)> = Vec::new();
        for (n, i) in instances.iter().enumerate() {
            let p = Vec3::new(i.place[0], i.place[1], i.place[2]);
            // A spectator's reach: arms up and a hop.
            let (lo, hi) = (p - Vec3::new(1.5, 0.5, 1.5), p + Vec3::new(1.5, 9.0, 1.5));
            match blocks.last_mut() {
                Some(b) if n > 0 && block(&instances[n - 1]) == block(i) => {
                    b.0 = b.0.min(lo);
                    b.1 = b.1.max(hi);
                    b.2.end = n as u32 + 1;
                }
                _ => blocks.push((lo, hi, n as u32..n as u32 + 1)),
            }
        }
        let figure = figure();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("crowd figure"),
            contents: bytemuck::cast_slice(&figure),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let count = instances.len() as u32;
        let instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("crowd"),
            contents: if instances.is_empty() { &[0; 48] } else { bytemuck::cast_slice(&instances) },
            usage: wgpu::BufferUsages::VERTEX,
        });
        let bikes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("crowd bikes"),
            size: std::mem::size_of::<Bikes>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = crate::enhance::layout(device, "crowd", &[(0, wgpu::ShaderStages::VERTEX, crate::enhance::uniform())]);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("crowd"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: bikes.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("crowd"),
            bind_group_layouts: &[Some(terrain_layout), Some(shared_layout), Some(&layout)],
            immediate_size: 0,
        });
        let shader = crate::enhance::shader(device, "crowd", include_str!("crowd.wgsl"));
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("crowd"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32x2],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4],
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        Crowd {
            pipeline,
            vertices,
            vertex_count: figure.len() as u32,
            instances,
            count,
            runs: blocks.iter().map(|b| b.2.clone()).collect(),
            blocks,
            bikes,
            group,
        }
    }

    /// Where the bikes are now (drawing coordinates), for the crowd to
    /// cheer them.
    pub fn watch(&self, queue: &wgpu::Queue, bikes: &[Vec3]) {
        let mut b = Bikes::zeroed();
        for (slot, p) in b.at.iter_mut().zip(bikes) {
            *slot = [p.x, p.y, p.z, 1.0];
        }
        queue.write_buffer(&self.bikes, 0, bytemuck::bytes_of(&b));
    }

    /// Picks the spectators `frustum` sees, for the draw after.
    pub fn cull(&mut self, frustum: &crate::cull::Frustum) {
        crate::cull::runs(self.blocks.iter().filter(|b| frustum.sees(b.0, b.1)).map(|b| b.2.clone()), &mut self.runs);
    }

    /// Draws the crowd, with the terrain's own group and the shared one.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, terrain: &wgpu::BindGroup, shared: &wgpu::BindGroup) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, terrain, &[]);
        pass.set_bind_group(1, shared, &[]);
        pass.set_bind_group(2, &self.group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_vertex_buffer(1, self.instances.slice(..));
        for run in &self.runs {
            pass.draw(0..self.vertex_count, run.clone());
        }
    }
}
