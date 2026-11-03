//! The sky box (`RES/SKIES.RES`): a cube of tiles around the camera, turned
//! with it but never moved, drawn behind everything else. Each face is four
//! tiles by four, laid out as the game lays them (0x43ec00); only the tiles
//! a sky has are drawn, so below the horizon the clear colour shows.

use crate::hires::SkyImages;
use crate::renderer::{from_game, DEPTH_FORMAT};
use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};
use mm2_formats::cub::{Cube, TILES};
use wgpu::util::DeviceExt;

/// How far the cube's faces stand from the eye, inside the near and far
/// planes; the shader puts it at the far plane.
const RADIUS: f32 = 100.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
    face: u32,
}

pub struct SkyRenderer {
    pipeline: wgpu::RenderPipeline,
    /// The enhanced drawing's sky ([`crate::graphics`]), when on: the
    /// painted one with its glow, and the real one (`sky_real.wgsl`).
    enhanced: Option<(wgpu::RenderPipeline, wgpu::RenderPipeline, wgpu::BindGroup)>,
    vertices: wgpu::Buffer,
    count: u32,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// A point of face `face` at grid corner (`c`, `r`), 0 to 4 each, column
/// from the left and row from the top, in game coordinates.
fn corner(face: usize, c: f32, r: f32) -> Vec3 {
    let (u, v) = (c / 2.0 - 1.0, 1.0 - r / 2.0);
    match face {
        0 => Vec3::new(u, v, 1.0),
        1 => Vec3::new(-1.0, v, u),
        2 => Vec3::new(-u, v, -1.0),
        3 => Vec3::new(1.0, v, -u),
        4 => Vec3::new(u, 1.0, -v),
        _ => Vec3::new(u, -1.0, v),
    }
}

impl SkyRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cube: &Cube,
        images: &SkyImages,
        format: wgpu::TextureFormat,
        samples: u32,
        enhanced: Option<&wgpu::BindGroupLayout>,
    ) -> SkyRenderer {
        let side = images.side;
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("sky"),
                size: wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: 6,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &images.faces,
        );
        let mut vertices = Vec::new();
        for (face, tiles) in cube.faces.iter().enumerate() {
            for (t, _) in tiles.iter().enumerate().filter(|(_, t)| t.is_some()) {
                let (c, r) = ((t % TILES) as f32, (t / TILES) as f32);
                let at = |dc: f32, dr: f32| Vertex {
                    position: (from_game(corner(face, c + dc, r + dr)) * RADIUS).into(),
                    uv: [(c + dc) / TILES as f32, (r + dr) / TILES as f32],
                    face: face as u32,
                };
                let quad = [at(0.0, 0.0), at(1.0, 0.0), at(1.0, 1.0), at(0.0, 1.0)];
                vertices.extend([0, 1, 2, 0, 2, 3].map(|i| quad[i]));
            }
        }
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky globals"),
            // The turn, and for the real sky its inverse.
            size: 128,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sky"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = crate::enhance::shader(device, "sky", include_str!("sky.wgsl"));
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Uint32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
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
        let enhanced = enhanced.map(|enh| {
            use crate::enhance::{sampler, texture, uniform};
            let group_layout = crate::enhance::layout(
                device,
                "sky enhanced",
                &[
                    (0, wgpu::ShaderStages::VERTEX_FRAGMENT, uniform()),
                    (1, wgpu::ShaderStages::FRAGMENT, texture(wgpu::TextureViewDimension::D2Array, wgpu::TextureSampleType::Float { filterable: true })),
                    (2, wgpu::ShaderStages::FRAGMENT, sampler()),
                ],
            );
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sky enhanced"),
                bind_group_layouts: &[Some(&group_layout), Some(enh)],
                immediate_size: 0,
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sky enhanced"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_enhanced"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Uint32],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_enhanced"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
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
            let real = crate::enhance::shader(device, "real sky", include_str!("sky_real.wgsl"));
            let real = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("real sky"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &real,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &real,
                    entry_point: Some("fs_real"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
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
            (pipeline, real, group_layout)
        });
        let faces = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let group = |layout: &wgpu::BindGroupLayout| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sky"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: globals.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&faces),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            })
        };
        let enhanced = enhanced.map(|(pipeline, real, layout)| (pipeline, real, group(&layout)));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture.create_view(
                        &wgpu::TextureViewDescriptor {
                            dimension: Some(wgpu::TextureViewDimension::D2Array),
                            ..Default::default()
                        },
                    )),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        SkyRenderer {
            pipeline,
            enhanced,
            vertices: vertex_buffer,
            count: vertices.len() as u32,
            globals,
            bind_group,
        }
    }

    /// Points the sky with the camera: its turn alone, not its place.
    pub fn update(&self, queue: &wgpu::Queue, proj: Mat4, view: Mat4) {
        let turn = proj * Mat4::from_mat3(Mat3::from_mat4(view));
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&turn.to_cols_array()));
        if self.enhanced.is_some() {
            queue.write_buffer(&self.globals, 64, bytemuck::bytes_of(&turn.inverse().to_cols_array()));
        }
    }

    /// Draws the sky as the enhanced drawing does, with its shared group:
    /// the real sky for `real`, else the painted one with its glow.
    pub fn draw_enhanced(&self, pass: &mut wgpu::RenderPass<'_>, shared: &wgpu::BindGroup, real: bool) {
        let Some((pipeline, real_sky, group)) = &self.enhanced else {
            return self.draw(pass);
        };
        pass.set_bind_group(1, shared, &[]);
        pass.set_bind_group(0, group, &[]);
        if real {
            pass.set_pipeline(real_sky);
            pass.draw(0..3, 0..1);
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..self.count, 0..1);
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..self.count, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_meet() {
        // The top-left of the front face is the front-left of the top.
        assert_eq!(corner(0, 0.0, 0.0), Vec3::new(-1.0, 1.0, 1.0));
        assert_eq!(corner(4, 0.0, 4.0), Vec3::new(-1.0, 1.0, 1.0));
        // Going right along the front reaches the right face's left edge.
        assert_eq!(corner(0, 4.0, 2.0), corner(3, 0.0, 2.0));
        assert_eq!(corner(3, 4.0, 2.0), corner(2, 0.0, 2.0));
        assert_eq!(corner(2, 4.0, 2.0), corner(1, 0.0, 2.0));
        assert_eq!(corner(1, 4.0, 2.0), corner(0, 0.0, 2.0));
    }
}
