//! Rain for the enhanced drawing ([`crate::graphics`]), none of it the
//! game's: streaks falling past the eye (`rain.wgsl`), drawn over the scene
//! when the weather is rain.

pub struct RainRenderer {
    pipeline: wgpu::RenderPipeline,
}

impl RainRenderer {
    /// With the terrain's own group's layout and the enhanced drawing's
    /// shared one.
    pub fn new(
        device: &wgpu::Device,
        terrain_layout: &wgpu::BindGroupLayout,
        shared_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        samples: u32,
        depth_format: wgpu::TextureFormat,
    ) -> RainRenderer {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rain"),
            bind_group_layouts: &[Some(terrain_layout), Some(shared_layout)],
            immediate_size: 0,
        });
        let shader = crate::enhance::shader(device, "rain", include_str!("rain.wgsl"));
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rain"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_rain"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
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
        RainRenderer { pipeline }
    }

    /// Draws the rain, with the terrain's own group and the shared one.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, terrain: &wgpu::BindGroup, shared: &wgpu::BindGroup) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, terrain, &[]);
        pass.set_bind_group(1, shared, &[]);
        pass.draw(0..3, 0..1);
    }
}
