//! Smooth ground round the eye for the enhanced drawing
//! ([`crate::graphics`]), none of it the game's: a patch of the terrain
//! cut finer, its heights a smooth curve through the game's samples, but
//! never more than a few inches off the game's own triangles
//! (`terrain.wgsl` `vs_smooth`). Hills and jump faces lose their facets.
//! Only drawn: the bikes ride the game's ground.

use mm2_formats::trn::Terrain;
use wgpu::util::DeviceExt;

/// Each cell is cut into this many a side (`SMOOTH_SPLIT` in the shader).
const SPLIT: u32 = 3;
/// How far the patch reaches each way from the eye, ft.
const REACH: f32 = 420.0;

pub struct SmoothGround {
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    indices: wgpu::Buffer,
    /// The patch in blocks (fine points from the corner: x, z from and
    /// to) and their indices, and the runs of those in view this frame.
    blocks: Vec<([u32; 4], std::ops::Range<u32>)>,
    runs: Vec<std::ops::Range<u32>>,
    /// Cells a side, and the terrain's samples a side.
    cells: u32,
    size: u32,
    scale: f32,
}

impl SmoothGround {
    /// With the terrain's own enhanced group's layout, the shared one and
    /// the terrain's shader.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        terrain: &Terrain,
        shader: &wgpu::ShaderModule,
        terrain_layout: &wgpu::BindGroupLayout,
        shared_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        samples: u32,
        depth_format: wgpu::TextureFormat,
    ) -> SmoothGround {
        let n = terrain.size as u32;
        let size = wgpu::Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: 1,
        };
        let texture = |label, format, data: &[u8]| {
            device
                .create_texture_with_data(
                    queue,
                    &wgpu::TextureDescriptor {
                        label: Some(label),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    data,
                )
                .create_view(&Default::default())
        };
        let heights = texture("smooth heights", wgpu::TextureFormat::R32Float, bytemuck::cast_slice(&terrain.heights));
        let normals: Vec<[i8; 4]> = terrain.normals.iter().map(|n| [n[0], n[1], n[2], 0]).collect();
        let normals = texture("smooth normals", wgpu::TextureFormat::Rgba8Snorm, bytemuck::cast_slice(&normals));
        let unfiltered = crate::enhance::texture(wgpu::TextureViewDimension::D2, wgpu::TextureSampleType::Float { filterable: false });
        let layout = crate::enhance::layout(
            device,
            "smooth ground",
            &[(0, wgpu::ShaderStages::VERTEX, unfiltered), (1, wgpu::ShaderStages::VERTEX, unfiltered)],
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("smooth ground"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&heights),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&normals),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("smooth ground"),
            bind_group_layouts: &[Some(terrain_layout), Some(shared_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("smooth ground"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_smooth"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_smooth"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
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
        let cells = ((2.0 * REACH / terrain.scale).round() as u32).clamp(8, n.saturating_sub(1).max(8));
        let side = cells * SPLIT + 1;
        // In blocks of the patch, so those out of view can be left out.
        let step = (side - 1).div_ceil(8);
        let mut indices = Vec::with_capacity((side as usize - 1).pow(2) * 6);
        let mut blocks = Vec::new();
        for bz in (0..side - 1).step_by(step as usize) {
            for bx in (0..side - 1).step_by(step as usize) {
                let start = indices.len() as u32;
                let (ex, ez) = ((bx + step).min(side - 1), (bz + step).min(side - 1));
                for z in bz..ez {
                    for x in bx..ex {
                        let i = z * side + x;
                        let below = i + side;
                        indices.extend_from_slice(&[i, below, below + 1, i, below + 1, i + 1]);
                    }
                }
                blocks.push(([bx, bz, ex, ez], start..indices.len() as u32));
            }
        }
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("smooth ground"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        SmoothGround {
            pipeline,
            group,
            indices,
            runs: blocks.iter().map(|b| b.1.clone()).collect(),
            blocks,
            cells,
            size: n,
            scale: terrain.scale,
        }
    }

    /// The patch round the eye at `eye` (drawing coordinates) for the
    /// shaders: its corner in terrain units, cells a side, on.
    pub fn uniform(&self, eye: glam::Vec3) -> [f32; 4] {
        if self.size <= self.cells + 1 {
            return [0.0, 0.0, self.cells as f32, 1.0];
        }
        let most = (self.size - 1 - self.cells) as f32;
        let half = self.cells as f32 / 2.0;
        let x = (eye.x / self.scale - half).floor().clamp(0.0, most);
        let z = (-eye.z / self.scale - half).floor().clamp(0.0, most);
        [x, z, self.cells as f32, 1.0]
    }

    /// Picks the blocks of the patch round `eye` that `frustum` sees, for
    /// the draw after.
    pub fn cull(&mut self, eye: glam::Vec3, frustum: &crate::cull::Frustum) {
        let [x, z, ..] = self.uniform(eye);
        let at = |fx: u32, fz: u32| (x + fx as f32 / SPLIT as f32, z + fz as f32 / SPLIT as f32);
        let s = self.scale;
        let blocks = self.blocks.iter().filter(|([x0, z0, x1, z1], _)| {
            let ((ax, az), (bx, bz)) = (at(*x0, *z0), at(*x1, *z1));
            // Every height: only the sides matter.
            frustum.sees(glam::Vec3::new(ax * s, -1e6, -bz * s), glam::Vec3::new(bx * s, 1e6, -az * s))
        });
        crate::cull::runs(blocks.map(|b| b.1.clone()), &mut self.runs);
    }

    /// Draws the patch, with the terrain's own group and the shared one.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, terrain: &wgpu::BindGroup, shared: &wgpu::BindGroup) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, terrain, &[]);
        pass.set_bind_group(1, shared, &[]);
        pass.set_bind_group(2, &self.group, &[]);
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        for run in &self.runs {
            pass.draw_indexed(run.clone(), 0, 0..1);
        }
    }
}
