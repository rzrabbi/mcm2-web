//! The enhanced drawing's targets and last steps ([`crate::graphics`]):
//! the scene is drawn in HDR with anti-aliasing, then ambient occlusion and
//! bloom are worked out and the result tone mapped onto the screen.

use crate::graphics::HDR_FORMAT;
use crate::renderer::DEPTH_FORMAT;
use bytemuck::{Pod, Zeroable};
use glam::Mat4;

const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// Halvings in the bloom's chain.
const BLOOM_STEPS: usize = 6;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PostGlobals {
    inv_proj: [[f32; 4]; 4],
    size: [f32; 4],
    flags: [f32; 4],
    params: [f32; 4],
    /// The sun's place on the screen (0 to 1) and how strong its shafts
    /// are (0 for none), and its colour.
    sun: [f32; 4],
    sun_colour: [f32; 4],
    /// From this frame's screen to the last frame's, for motion blur.
    reproject: [[f32; 4]; 4],
    /// Replays' camera look: x the distance in focus (ft), y the blur's
    /// size out of focus (px), z the motion blur's strength, w 1 on.
    cinematic: [f32; 4],
    /// x 1 when the scene is drawn smaller than the screen and stretched
    /// over it.
    scaled: [f32; 4],
}

/// What the scene is drawn into, at one size.
struct Targets {
    size: (u32, u32),
    /// The multisampled colour, when anti-aliasing, resolved into `hdr`.
    msaa: Option<wgpu::TextureView>,
    hdr: wgpu::TextureView,
    depth: wgpu::TextureView,
    ao_group: wgpu::BindGroup,
    ao: wgpu::TextureView,
    blur_group: wgpu::BindGroup,
    blurred: wgpu::TextureView,
    bloom: Vec<wgpu::TextureView>,
    /// Bloom's first step, then each step down (from the one above), then
    /// each step up (from the one below).
    bloom_first: wgpu::BindGroup,
    bloom_down: Vec<wgpu::BindGroup>,
    bloom_up: Vec<wgpu::BindGroup>,
    composite: wgpu::BindGroup,
}

pub struct Post {
    samples: u32,
    globals: wgpu::Buffer,
    sampler: wgpu::Sampler,
    ao: wgpu::RenderPipeline,
    ao_blur: wgpu::RenderPipeline,
    bloom_first: wgpu::RenderPipeline,
    bloom_down: wgpu::RenderPipeline,
    bloom_up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    targets: Option<Targets>,
}

fn pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    entry: &str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: None,
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_full"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl Post {
    /// For a scene drawn with `samples` per pixel, shown on a `format`
    /// target.
    pub fn new(device: &wgpu::Device, samples: u32, format: wgpu::TextureFormat) -> Post {
        let depth_type = if samples > 1 {
            "texture_depth_multisampled_2d"
        } else {
            "texture_depth_2d"
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").replace("DEPTH_TEXTURE", depth_type).into()),
        });
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        Post {
            samples,
            globals: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("post"),
                size: std::mem::size_of::<PostGlobals>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("post"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            ao: pipeline(device, &shader, "fs_ao", AO_FORMAT, None),
            ao_blur: pipeline(device, &shader, "fs_ao_blur", AO_FORMAT, None),
            bloom_first: pipeline(device, &shader, "fs_bloom_first", HDR_FORMAT, None),
            bloom_down: pipeline(device, &shader, "fs_bloom_down", HDR_FORMAT, None),
            bloom_up: pipeline(device, &shader, "fs_bloom_up", HDR_FORMAT, Some(add)),
            composite: pipeline(device, &shader, "fs_composite", format, None),
            targets: None,
        }
    }

    fn texture(
        device: &wgpu::Device,
        label: &str,
        (width, height): (u32, u32),
        format: wgpu::TextureFormat,
        samples: u32,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    fn group(&self, device: &wgpu::Device, pipeline: &wgpu::RenderPipeline, views: &[(u32, &wgpu::TextureView)], with_globals: bool, with_sampler: bool) -> wgpu::BindGroup {
        let mut entries: Vec<wgpu::BindGroupEntry> = views
            .iter()
            .map(|(binding, view)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: wgpu::BindingResource::TextureView(view),
            })
            .collect();
        if with_globals {
            entries.push(wgpu::BindGroupEntry {
                binding: 0,
                resource: self.globals.as_entire_binding(),
            });
        }
        if with_sampler {
            entries.push(wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        })
    }

    /// Makes the targets for `size`, unless they are made.
    fn ready(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let draw = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let view = |t: wgpu::Texture| t.create_view(&Default::default());
        let msaa = (self.samples > 1).then(|| {
            view(Self::texture(device, "scene msaa", size, HDR_FORMAT, self.samples, wgpu::TextureUsages::RENDER_ATTACHMENT))
        });
        let hdr = view(Self::texture(device, "scene", size, HDR_FORMAT, 1, draw));
        let depth_texture = Self::texture(device, "scene depth", size, DEPTH_FORMAT, self.samples, draw);
        let depth = depth_texture.create_view(&Default::default());
        let depth_only = depth_texture.create_view(&wgpu::TextureViewDescriptor {
            aspect: wgpu::TextureAspect::DepthOnly,
            ..Default::default()
        });
        // The occlusion at half size.
        let half = (size.0.div_ceil(2), size.1.div_ceil(2));
        let ao = view(Self::texture(device, "ao", half, AO_FORMAT, 1, draw));
        let blurred = view(Self::texture(device, "ao blurred", half, AO_FORMAT, 1, draw));
        let mut bloom = Vec::new();
        let mut s = size;
        for _ in 0..BLOOM_STEPS {
            s = ((s.0 / 2).max(1), (s.1 / 2).max(1));
            bloom.push(view(Self::texture(device, "bloom", s, HDR_FORMAT, 1, draw)));
        }
        let ao_group = self.group(device, &self.ao, &[(1, &depth_only)], true, false);
        let blur_group = self.group(device, &self.ao_blur, &[(3, &ao)], true, true);
        let bloom_first = self.group(device, &self.bloom_first, &[(2, &hdr)], false, true);
        let bloom_down = (1..BLOOM_STEPS)
            .map(|i| self.group(device, &self.bloom_down, &[(4, &bloom[i - 1])], false, true))
            .collect();
        let bloom_up = (1..BLOOM_STEPS)
            .map(|i| self.group(device, &self.bloom_up, &[(4, &bloom[i])], false, true))
            .collect();
        let composite = self.group(device, &self.composite, &[(1, &depth_only), (2, &hdr), (3, &blurred), (6, &bloom[0])], true, true);
        self.targets = Some(Targets {
            size,
            msaa,
            hdr,
            depth,
            ao_group,
            ao,
            blur_group,
            blurred,
            bloom,
            bloom_first,
            bloom_down,
            bloom_up,
            composite,
        });
    }

    /// Where to draw the scene at `size`: the colour, what it resolves
    /// into, and the depth.
    pub fn scene(&mut self, device: &wgpu::Device, size: (u32, u32)) -> (&wgpu::TextureView, Option<&wgpu::TextureView>, &wgpu::TextureView) {
        self.ready(device, size);
        let t = self.targets.as_ref().unwrap();
        match &t.msaa {
            Some(msaa) => (msaa, Some(&t.hdr), &t.depth),
            None => (&t.hdr, None, &t.depth),
        }
    }

    /// Finishes the frame onto `target`: occlusion and bloom when on, and
    /// tone mapped unless the original look is showing.
    pub fn finish(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        proj: Mat4,
        showing: Option<crate::graphics::Enhanced>,
        sun: Option<([f32; 3], [f32; 3])>,
        cinematic: Option<(f32, Mat4)>,
        screen: (u32, u32),
    ) {
        let Some(t) = &self.targets else { return };
        let (w, h) = (t.size.0 as f32, t.size.1 as f32);
        let ao = showing.is_some_and(|e| e.ao);
        let bloom = showing.is_some_and(|e| e.bloom);
        let globals = PostGlobals {
            inv_proj: proj.inverse().to_cols_array_2d(),
            size: [w, h, 1.0 / w, 1.0 / h],
            flags: [ao as u32 as f32, bloom as u32 as f32, showing.is_some() as u32 as f32, showing.is_some_and(|e| e.grade) as u32 as f32],
            // A 3 ft reach, its strength, the bloom's, and how many pixels a foot
            // spans 1 ft away.
            params: [3.0, if showing.is_some_and(|e| e.grade) { 2.2 } else { 1.5 }, if showing.is_some_and(|e| e.grade) { 0.9 } else { 0.6 }, proj.y_axis.y * 0.5 * h],
            sun: sun.filter(|_| showing.is_some_and(|e| e.shafts)).map_or([0.0; 4], |(s, _)| [s[0], s[1], s[2], 0.0]),
            sun_colour: sun.map_or([0.0; 4], |(_, c)| [c[0], c[1], c[2], 0.0]),
            reproject: cinematic.map_or(Mat4::IDENTITY, |c| c.1).to_cols_array_2d(),
            cinematic: cinematic.map_or([0.0; 4], |(focus, _)| [focus, 0.006 * h, 0.5, 1.0]),
            scaled: [(t.size != screen) as u32 as f32, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        let pass = |encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, load: bool, pipeline: &wgpu::RenderPipeline, group: &wgpu::BindGroup| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if load { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::BLACK) },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        };
        if ao {
            pass(encoder, &t.ao, false, &self.ao, &t.ao_group);
            pass(encoder, &t.blurred, false, &self.ao_blur, &t.blur_group);
        }
        if bloom {
            pass(encoder, &t.bloom[0], false, &self.bloom_first, &t.bloom_first);
            for i in 1..BLOOM_STEPS {
                pass(encoder, &t.bloom[i], false, &self.bloom_down, &t.bloom_down[i - 1]);
            }
            for i in (1..BLOOM_STEPS).rev() {
                pass(encoder, &t.bloom[i - 1], true, &self.bloom_up, &t.bloom_up[i - 1]);
            }
        }
        pass(encoder, target, false, &self.composite, &t.composite);
    }
}
