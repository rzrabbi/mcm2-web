//! Grass for the enhanced drawing ([`crate::graphics`]): tufts on a grid
//! round the camera wherever the painted ground is grassy (`grass.wgsl`).
//! Only drawn: the bikes ride through it as the game has them ride over
//! the ground.

use mm2_formats::trn::Terrain;

/// Tufts on the grids round the camera: 320 a side near, 256 far.
const TUFTS: u32 = 320 * 320 + 256 * 256;

pub struct GrassRenderer {
    pipeline: wgpu::RenderPipeline,
    heights: wgpu::BindGroup,
}

impl GrassRenderer {
    /// With the terrain's own group's layout and the enhanced drawing's
    /// shared one, drawn into `format` with `samples` a pixel.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        terrain: &Terrain,
        terrain_layout: &wgpu::BindGroupLayout,
        shared_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        samples: u32,
        depth_format: wgpu::TextureFormat,
    ) -> GrassRenderer {
        use wgpu::util::DeviceExt;
        let n = terrain.size as u32;
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("terrain heights"),
                size: wgpu::Extent3d {
                    width: n,
                    height: n,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(&terrain.heights),
        );
        let heights_layout = crate::enhance::layout(
            device,
            "grass heights",
            &[(
                0,
                wgpu::ShaderStages::VERTEX,
                crate::enhance::texture(
                    wgpu::TextureViewDimension::D2,
                    wgpu::TextureSampleType::Float { filterable: false },
                ),
            )],
        );
        let heights = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grass heights"),
            layout: &heights_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass"),
            bind_group_layouts: &[Some(terrain_layout), Some(shared_layout), Some(&heights_layout)],
            immediate_size: 0,
        });
        let shader = crate::enhance::shader(device, "grass", include_str!("grass.wgsl"));
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("grass"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
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
                alpha_to_coverage_enabled: samples > 1,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        GrassRenderer { pipeline, heights }
    }

    /// Draws the grass, with the terrain's own group and the shared one.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, terrain: &wgpu::BindGroup, shared: &wgpu::BindGroup) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, terrain, &[]);
        pass.set_bind_group(1, shared, &[]);
        pass.set_bind_group(2, &self.heights, &[]);
        pass.draw(0..18, 0..TUFTS);
    }
}
