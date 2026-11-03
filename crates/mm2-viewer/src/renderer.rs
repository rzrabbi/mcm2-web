//! Draws a track's terrain and objects with wgpu.

use crate::hires::Image;
use crate::objects::{ObjectMesh, ObjectRenderer};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use mm2_formats::trn::Terrain;
use wgpu::util::DeviceExt;

/// With a stencil, which the player's shadow uses so that it darkens
/// each pixel once.
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

pub const SKY: wgpu::Color = wgpu::Color {
    r: 0.36,
    g: 0.5,
    b: 0.74,
    a: 1.0,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    height: f32,
    normal: [i8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Globals {
    pub view_proj: [[f32; 4]; 4],
    pub camera: [f32; 4],
    pub grid: [f32; 4],
    /// The fog's colour (linear) and its start and end, ft; the end is
    /// the far plane too.
    pub fog: [f32; 4],
    pub fog_range: [f32; 4],
    /// The scene's lights, [`SceneLights`].
    pub ambient: [f32; 4],
    pub sun: [f32; 4],
    pub to_sun: [f32; 4],
    /// The player's shadow: from the drawing to its mask's texture, and
    /// whether it falls (x).
    pub shadow_view: [[f32; 4]; 4],
    pub shadow_on: [f32; 4],
}

/// The ground's blocks are this many cells a side.
const BLOCK: u32 = 32;
/// The shadow's mask, as 0x4defa0 makes it: 128 texels a side.
const SHADOW_SIZE: u32 = 128;
/// Half the side of the square the shadow's camera sees round the bike
/// and rider, ft, and how far back toward the sun it stands.
const SHADOW_RADIUS: f32 = 4.5;
const SHADOW_REACH: f32 = 100.0;

/// The scene's two lights as the game lights each vertex with them
/// (0x4a0e40, from `[Lights]` via 0x4f0300): the ambient colour, plus the
/// sun's colour times how squarely the vertex faces it, each 0..=255 and
/// clamped there. Colours are 0..1 of the game's 0..255.
#[derive(Debug, Clone, Copy)]
pub struct SceneLights {
    pub ambient: [f32; 3],
    pub sun: [f32; 3],
    /// The way to the sun, in viewer coordinates.
    pub to_sun: Vec3,
}

impl SceneLights {
    /// From a scene's `[Lights]`. The game's lighting takes one ambient
    /// light and one other; a scene without lights gets the game's
    /// defaults (0x4f0300): grey 90 ambient and a 200 grey sun placed at
    /// infinity towards (−2000, 4000, 1252).
    pub fn from_scene(lights: &[mm2_formats::scn::Light]) -> SceneLights {
        use mm2_formats::scn::LightKind;
        let colour = |c: [f32; 3]| c.map(|v| v.clamp(0.0, 255.0) / 255.0);
        let ambient = lights
            .iter()
            .find(|l| l.kind == LightKind::Ambient)
            .map_or([90.0 / 255.0; 3], |l| colour(l.color));
        let sun = lights.iter().find_map(|l| {
            // A directional light (type 4) shines along its look vector;
            // a point at infinity (type 2) from where it is, towards each
            // object, which the track's size makes one direction.
            let to = match l.kind {
                LightKind::Directional => -Vec3::from(l.direction?),
                LightKind::Ambient => return None,
                _ => Vec3::from(l.position?),
            };
            Some((colour(l.color), to))
        });
        let (sun, to) = sun.unwrap_or(([200.0 / 255.0; 3], Vec3::new(-2000.0, 4000.0, 1252.0)));
        SceneLights {
            ambient,
            sun,
            to_sun: from_game(to).normalize_or(Vec3::Y),
        }
    }
}

impl Default for SceneLights {
    /// The port's own light, for views without a scene (the menus).
    fn default() -> SceneLights {
        SceneLights {
            ambient: [0.5; 3],
            sun: [0.6; 3],
            to_sun: Vec3::new(0.4, 0.8, 0.3).normalize(),
        }
    }
}

impl SceneLights {
    pub(crate) fn globals(&self) -> ([f32; 4], [f32; 4], [f32; 4]) {
        let [a, b, c] = self.ambient;
        let [r, g, bl] = self.sun;
        (
            [a, b, c, 0.0],
            [r, g, bl, 0.0],
            self.to_sun.extend(0.0).into(),
        )
    }
}

pub struct TerrainRenderer {
    pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// The ground's blocks: their bounds (drawing coordinates) and indices.
    blocks: Vec<(Vec3, Vec3, std::ops::Range<u32>)>,
    /// The runs of indices in view this frame.
    runs: Vec<std::ops::Range<u32>>,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// The terrain's textures, tile map and sampler, kept to rebind with
    /// a detail texture.
    terrain_views: (wgpu::TextureView, wgpu::TextureView, wgpu::Sampler),
    /// The player's shadow's mask, as the sun sees the bike and rider.
    shadow_mask: wgpu::TextureView,
    detail: wgpu::TextureView,
    grid: [f32; 4],
    depth: Option<(wgpu::TextureView, u32, u32)>,
    objects: Option<ObjectRenderer>,
    sky: Option<crate::sky::SkyRenderer>,
    clear: wgpu::Color,
    /// The fog: its colour, and where it starts and ends (the far plane);
    /// a scene without a sky box has none (0x4e2d70).
    fog: [f32; 3],
    fog_range: (f32, f32),
    lights: SceneLights,
    /// The dirt the bikes throw up.
    pub particles: crate::particles::Particles,
    /// The Sky and Particles switches (F8, F9): off, the sky is the
    /// clear colour and no dirt is drawn.
    pub sky_on: bool,
    pub particles_on: bool,
    /// The Shadows switch (F10).
    pub shadows_on: bool,
    shadow: Option<([usize; 2], Vec3, Vec3)>,
    shadow_casters: Option<[usize; 2]>,
    /// A desert track, for the enhanced drawing's weather.
    desert: bool,
    /// In a replay, the distance (ft) to the rider the camera follows: the
    /// enhanced drawing then blurs what is out of focus and what the
    /// camera swings past.
    pub cinematic: Option<f32>,
    /// The last frame's view and projection, for that motion blur.
    last_view_proj: Option<Mat4>,
    /// What the scene is drawn in: the target's format, or HDR with the
    /// enhanced drawing; and the samples a pixel.
    scene_format: wgpu::TextureFormat,
    samples: u32,
    /// The last frame's size.
    size: (u32, u32),
    /// The enhanced drawing ([`crate::graphics`]), when on.
    enhanced: Option<Enhanced>,
}

/// The terrain renderer's part of the enhanced drawing.
struct Enhanced {
    shared: crate::enhance::Shared,
    terrain: wgpu::RenderPipeline,
    /// The terrain's own group for it, the same as the game's drawing's.
    layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
    grass: Option<crate::grass::GrassRenderer>,
    smooth: Option<crate::smooth::SmoothGround>,
    crowd: Option<crate::crowd::Crowd>,
    /// Each bike's place last seen on the ground (drawing coordinates),
    /// for the crowd.
    bikes: Vec<Option<Vec3>>,
    ruts: Option<crate::ruts::RutPainter>,
    dust: Option<crate::dust::DustRenderer>,
    rain: Option<crate::rain::RainRenderer>,
    /// How dirty each bike and its rider are, 0 to 1, and where the bike
    /// was last frame (drawing coordinates).
    mud: Vec<(f32, Option<Vec3>)>,
    post: crate::post::Post,
}

impl TerrainRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        terrain: &Terrain,
        textures: &[Image],
        objects: &ObjectMesh,
        format: wgpu::TextureFormat,
    ) -> Self {
        let n = terrain.size;
        let vertices: Vec<Vertex> = terrain
            .heights
            .iter()
            .zip(&terrain.normals)
            .map(|(&height, n)| Vertex {
                height,
                normal: [n[0], n[1], n[2], 0],
            })
            .collect();
        // In blocks of BLOCK cells a side, each block's triangles together,
        // so the blocks out of view can be left out (crate::cull).
        let mut indices = Vec::with_capacity((n - 1) * (n - 1) * 6);
        let mut blocks = Vec::new();
        let cells = n as u32 - 1;
        for bz in (0..cells).step_by(BLOCK as usize) {
            for bx in (0..cells).step_by(BLOCK as usize) {
                let start = indices.len() as u32;
                let (mut low, mut high) = (f32::MAX, f32::MIN);
                for z in bz..(bz + BLOCK).min(cells) {
                    for x in bx..(bx + BLOCK).min(cells) {
                        let i = z * n as u32 + x;
                        let below = i + n as u32;
                        for v in [i, i + 1, below, below + 1] {
                            low = low.min(terrain.heights[v as usize]);
                            high = high.max(terrain.heights[v as usize]);
                        }
                        // Split as the ground is sampled (ride::ground).
                        if (x ^ z) & 1 == 0 {
                            indices.extend_from_slice(&[i, below, below + 1, i, below + 1, i + 1]);
                        } else {
                            indices.extend_from_slice(&[i, below, i + 1, i + 1, below, below + 1]);
                        }
                    }
                }
                let s = terrain.scale;
                let far = (bx + BLOCK).min(cells) as f32;
                let near = (bz + BLOCK).min(cells) as f32;
                blocks.push((
                    Vec3::new(bx as f32 * s, low * s, -near * s),
                    Vec3::new(far * s, high * s, -(bz as f32) * s),
                    start..indices.len() as u32,
                ));
            }
        }
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Every tile is the same size; pack textures into one array.
        let tile_size = terrain.tiles.first().map_or(64, |t| t.size);
        let tiles_per_side = (n as u32 - 1).div_ceil(tile_size).max(1);
        let mut tile_map = vec![0xffffu16; (tiles_per_side * tiles_per_side) as usize];
        for t in &terrain.tiles {
            let (tx, tz) = (t.x / tile_size, t.z / tile_size);
            if tx < tiles_per_side && tz < tiles_per_side {
                tile_map[(tz * tiles_per_side + tx) as usize] = t.texture;
            }
        }
        let textures = upload_textures(device, queue, textures);
        let tile_map = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("tile map"),
                size: wgpu::Extent3d {
                    width: tiles_per_side,
                    height: tiles_per_side,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R16Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(&tile_map),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: crate::graphics::anisotropy(),
            ..Default::default()
        });
        let grid = [
            n as f32,
            terrain.scale,
            tile_size as f32,
            tiles_per_side as f32,
        ];
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let upgrades = crate::graphics::enhanced();
        let (scene_format, samples) = match upgrades {
            Some(e) => (crate::graphics::HDR_FORMAT, crate::graphics::samples(e.msaa)),
            None => (format, 1),
        };
        let target_format = format;
        let format = scene_format;
        let shader = crate::enhance::shader(device, "terrain", include_str!("terrain.wgsl"));
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32, 1 => Snorm8x4],
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
        let enhanced = upgrades.map(|_| {
            let shared = crate::enhance::Shared::new(device, queue, upgrades.is_some_and(|e| e.ruts));
            use crate::enhance::{sampler, texture, uniform};
            use wgpu::{ShaderStages as S, TextureSampleType as T, TextureViewDimension as D};
            let float = T::Float { filterable: true };
            let group_layout = crate::enhance::layout(
                device,
                "terrain enhanced",
                &[
                    (0, S::VERTEX_FRAGMENT, uniform()),
                    // The grass reads these as it is placed, too.
                    (1, S::VERTEX_FRAGMENT, texture(D::D2Array, float)),
                    (2, S::VERTEX_FRAGMENT, sampler()),
                    (3, S::VERTEX_FRAGMENT, texture(D::D2, T::Uint)),
                    (4, S::FRAGMENT, texture(D::D2, float)),
                    (5, S::FRAGMENT, sampler()),
                    (6, S::FRAGMENT, texture(D::D2, float)),
                    (7, S::FRAGMENT, sampler()),
                ],
            );
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("terrain enhanced"),
                bind_group_layouts: &[Some(&group_layout), Some(&shared.layout)],
                immediate_size: 0,
            });
            let terrain = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("terrain enhanced"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32, 1 => Snorm8x4],
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
            (shared, terrain, group_layout)
        });
        let terrain_views = (
            textures.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            }),
            tile_map.create_view(&Default::default()),
            sampler,
        );
        // No detail texture until the scene gives one: clear, so the
        // ground shows as it is.
        let none = mm2_formats::tga::Image {
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        };
        let detail = detail_view(device, queue, &none);
        let shadow_mask = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("shadow mask"),
                size: wgpu::Extent3d {
                    width: SHADOW_SIZE,
                    height: SHADOW_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::objects::SHADOW_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let bind_group = terrain_bind_group(device, &pipeline.get_bind_group_layout(0), &globals, &terrain_views, &detail, &shadow_mask);
        let enhanced = enhanced.map(|(shared, terrain_pipeline, layout)| Enhanced {
            post: crate::post::Post::new(device, samples, target_format),
            group: terrain_bind_group(device, &layout, &globals, &terrain_views, &detail, &shadow_mask),
            ruts: upgrades.is_some_and(|e| e.ruts).then(|| {
                // Over the whole terrain: x from 0, z up to 0.
                let size = (n as f32 - 1.0) * terrain.scale;
                crate::ruts::RutPainter::new(device, glam::Vec2::new(0.0, -size), size)
            }),
            mud: Vec::new(),
            crowd: None,
            bikes: Vec::new(),
            rain: upgrades.is_some_and(|e| e.weather == crate::graphics::Weather::Rain).then(|| {
                crate::rain::RainRenderer::new(device, &layout, &shared.layout, format, samples, DEPTH_FORMAT)
            }),
            dust: upgrades.is_some_and(|e| e.dust).then(|| {
                crate::dust::DustRenderer::new(device, &layout, &shared.layout, format, samples, DEPTH_FORMAT)
            }),
            grass: upgrades.is_some_and(|e| e.grass).then(|| {
                crate::grass::GrassRenderer::new(device, queue, terrain, &layout, &shared.layout, format, samples, DEPTH_FORMAT)
            }),
            smooth: upgrades.is_some_and(|e| e.smooth).then(|| {
                crate::smooth::SmoothGround::new(device, queue, terrain, &shader, &layout, &shared.layout, format, samples, DEPTH_FORMAT)
            }),
            terrain: terrain_pipeline,
            shared,
            layout,
        });
        let objects = ObjectRenderer::new(
            device,
            queue,
            objects,
            &globals,
            format,
            DEPTH_FORMAT,
            samples,
            enhanced.as_ref().map(|e| &e.shared.layout),
        );
        TerrainRenderer {
            pipeline,
            vertices,
            indices,
            blocks,
            runs: Vec::new(),
            globals,
            bind_group,
            terrain_views,
            shadow_mask,
            detail,
            grid,
            depth: None,
            objects,
            sky: None,
            clear: SKY,
            fog: [0.62, 0.74, 0.88],
            fog_range: NO_FOG,
            lights: SceneLights::default(),
            particles: crate::particles::Particles::new(),
            sky_on: true,
            particles_on: true,
            shadows_on: true,
            shadow: None,
            shadow_casters: None,
            desert: false,
            cinematic: None,
            last_view_proj: None,
            scene_format,
            samples,
            size: (0, 0),
            enhanced,
        }
    }

    /// Lays the detail texture over the terrain (0x50b140, 0x50b990): each
    /// texel is blended to the detail's colour by the detail's alpha, the
    /// way the game's second texture stage does (BLENDTEXTUREALPHA). The
    /// detail spans one terrain texture tile.
    pub fn set_detail(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, image: &mm2_formats::tga::Image) {
        self.detail = detail_view(device, queue, image);
        if let Some(e) = &mut self.enhanced {
            e.group = terrain_bind_group(device, &e.layout, &self.globals, &self.terrain_views, &self.detail, &self.shadow_mask);
        }
        self.bind_group = terrain_bind_group(
            device,
            &self.pipeline.get_bind_group_layout(0),
            &self.globals,
            &self.terrain_views,
            &self.detail,
            &self.shadow_mask,
        );
    }

    /// Draws `cube` as the sky.
    pub fn set_sky(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cube: &mm2_formats::cub::Cube,
        images: &crate::hires::SkyImages,
        _format: wgpu::TextureFormat,
    ) {
        self.sky = Some(crate::sky::SkyRenderer::new(
            device,
            queue,
            cube,
            images,
            self.scene_format,
            self.samples,
            self.enhanced.as_ref().map(|e| &e.shared.layout),
        ));
    }

    /// Draws the bikes' dirt from `sheet`, the scene's particle texture.
    pub fn set_dirt(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sheet: &mm2_formats::tga::Image,
        _format: wgpu::TextureFormat,
    ) {
        self.particles.upload(device, queue, sheet, self.scene_format, self.samples);
    }

    /// Fills what neither ground nor sky covers with the scene's fog
    /// colour (0–255 sRGB), as the game clears to it.
    pub fn set_fog(&mut self, rgb: [f32; 3]) {
        let linear = |c: f32| ((c / 255.0) as f64).powf(2.2);
        self.clear = wgpu::Color {
            r: linear(rgb[0]),
            g: linear(rgb[1]),
            b: linear(rgb[2]),
            a: 1.0,
        };
        self.fog = rgb.map(|c| linear(c) as f32);
    }

    /// Marks the track a desert one: the enhanced drawing's rain is
    /// overcast there.
    pub fn set_desert(&mut self, desert: bool) {
        self.desert = desert;
    }

    /// Lights the terrain and objects with the scene's lights.
    pub fn set_lights(&mut self, lights: SceneLights) {
        self.lights = lights;
    }

    /// The fog's reach at the Visibility detail `quality`, 0 to 9, for the
    /// scene's `[Fog]` Visibility and Haziness (0x463d00, 0x463700, the Fog
    /// made at 0x4e2d70 with 768, 256 and 128 ft): the far plane is
    /// `256 + 512 × (visibility − (9 − quality) / 10)`, at least 256, and
    /// the fog starts `1 − haziness` of the way from 128 ft out to it.
    pub fn set_visibility(&mut self, quality: u32, visibility: f32, haziness: f32) {
        self.fog_range = fog_range(quality, visibility, haziness);
    }

    /// The last frame's width over its height.
    pub fn aspect(&self) -> f32 {
        match self.size {
            (0, _) => 4.0 / 3.0,
            (w, h) => w as f32 / h.max(1) as f32,
        }
    }

    /// Moves animated object `index` (see [`ObjectMesh::sources`]) to
    /// `place`, in game coordinates.
    pub fn set_object_place(&mut self, index: usize, place: glam::Mat4) {
        if let Some(objects) = &mut self.objects {
            objects.set_place(index, place);
        }
    }

    /// The player's shadow: the objects that cast it, the middle of
    /// them and the way the light travels, game coordinates.
    pub fn set_shadow(&mut self, casters: [usize; 2], centre: Vec3, light: Vec3) {
        self.shadow = Some((casters, from_game(centre), from_game(light)));
    }

    /// Where animated object `index`'s frame `name` is at `time`, in game
    /// coordinates.
    pub fn object_frame(&self, index: usize, name: &str, time: f32) -> Option<Vec3> {
        self.objects.as_ref()?.frame_at(index, name, time)
    }

    /// Animated object `index`'s frame called `name`.
    pub fn object_frame_index(&self, index: usize, name: &str) -> Option<usize> {
        self.objects.as_ref()?.frame_index(index, name)
    }

    /// Where animated object `index`'s frames are at `time`, game
    /// coordinates.
    pub fn object_frames(&self, index: usize, frames: &[usize], time: f32) -> Vec<Option<glam::Mat4>> {
        match &self.objects {
            Some(o) => o.frames_at(index, frames, time),
            None => vec![None; frames.len()],
        }
    }

    /// Draws animated object `index` see-through, or solid.
    pub fn set_object_ghost(&mut self, index: usize, ghost: bool) {
        if let Some(objects) = &mut self.objects {
            objects.set_ghost(index, ghost);
        }
    }

    /// Plays motion `name` on animated object `index` from `start` seconds.
    pub fn set_motion(&mut self, index: usize, name: &str, start: f32) {
        if let Some(objects) = &mut self.objects {
            objects.set_motion(index, name, start);
        }
    }

    /// Plays motion `name` on animated object `index` from `start`, fading
    /// in from the pose before over `span` seconds.
    pub fn fade_motion(&mut self, index: usize, name: &str, start: f32, span: f32) {
        if let Some(objects) = &mut self.objects {
            objects.fade_to(index, name, start, span);
        }
    }

    /// Turns animated object `index`'s frame `name` by `angle` about its
    /// own x axis: a bike's wheel.
    pub fn set_spin(&mut self, index: usize, name: &str, angle: f32) {
        if let Some(o) = &mut self.objects {
            o.set_spin(index, name, angle);
        }
    }

    /// Mixes motions on animated object `index`, each at a phase and weight.
    pub fn set_blend(&mut self, index: usize, mix: &[(&str, f32, f32, u8)]) {
        if let Some(o) = &mut self.objects {
            o.set_blend(index, mix);
        }
    }

    /// Where each bike's tyres touch the ground (game coordinates) and
    /// how it kicks up dust there, for the enhanced drawing's ruts and
    /// dust; `dt` is the step, s.
    pub fn tyres(&mut self, feed: &[([Option<Vec3>; 2], crate::dust::Kick)], dt: f32) {
        let Some(e) = self.enhanced.as_mut() else { return };
        let bikes: Vec<[Option<Vec3>; 2]> = feed.iter().map(|f| f.0).collect();
        let bikes = &bikes[..];
        if let Some(ruts) = &mut e.ruts {
            ruts.tyres(bikes);
        }
        e.bikes.resize(bikes.len(), None);
        for (seen, tyres) in e.bikes.iter_mut().zip(bikes) {
            if let Some(p) = tyres[0].or(tyres[1]) {
                *seen = Some(from_game(p));
            }
        }
        let upgrades = crate::graphics::enhanced();
        let wet = upgrades.is_some_and(|u| u.weather == crate::graphics::Weather::Rain && !self.desert);
        if let Some(dust) = &mut e.dust {
            dust.step(feed, dt, wet);
        }
        // Dirt builds up as a bike runs on the ground: slowly in the dry,
        // fast on wet ground.
        if upgrades.is_some_and(|u| u.mud) && dt > 0.0 {
            let start = upgrades.map_or(0.0, |u| u.mud_start);
            e.mud.resize(bikes.len(), (start, None));
            for (m, tyres) in e.mud.iter_mut().zip(bikes) {
                let here = tyres[1].or(tyres[0]).map(from_game);
                if let (Some(a), Some(b)) = (m.1, here) {
                    let speed = a.distance(b) / dt;
                    if speed < 200.0 {
                        let rate = if wet { 0.02 } else { 0.006 };
                        m.0 = (m.0 + rate * (speed / 40.0).min(1.5) * dt).min(1.0);
                    }
                }
                m.1 = here;
            }
        }
    }

    /// The enhanced drawing's spectators: where each stands (game
    /// coordinates) and which way it looks.
    pub fn set_crowd(&mut self, device: &wgpu::Device, spots: &[(Vec3, Vec3)]) {
        let Some(e) = self.enhanced.as_mut() else { return };
        e.crowd = (!spots.is_empty()).then(|| {
            let samples = crate::graphics::enhanced().map_or(1, |u| crate::graphics::samples(u.msaa));
            crate::crowd::Crowd::new(device, spots, &e.layout, &e.shared.layout, crate::graphics::HDR_FORMAT, samples, DEPTH_FORMAT)
        });
        eprintln!("{} spectators", spots.len().div_ceil(spots.len().div_ceil(crate::crowd::MOST).max(1)));
    }

    /// Draws bike `bike`'s models (its bike and its rider) as dirty as it
    /// has got.
    pub fn show_mud(&mut self, bike: usize, models: [usize; 2]) {
        let start = crate::graphics::enhanced().map_or(0.0, |u| u.mud_start);
        let mud = self.enhanced.as_ref().and_then(|e| e.mud.get(bike)).map_or(start, |m| m.0);
        if let Some(objects) = &mut self.objects {
            for m in models {
                objects.set_mud(m, mud);
            }
        }
    }

    /// Holds animated object `index` on its first frame until `time`.
    pub fn set_motion_start(&mut self, index: usize, time: f32) {
        if let Some(objects) = &mut self.objects {
            objects.set_motion_start(index, time);
        }
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        (width, height): (u32, u32),
        camera: &crate::camera::Camera,
        time: f32,
    ) -> wgpu::CommandBuffer {
        self.size = (width, height);
        // The enhanced drawing as it shows now: none while F3 shows the
        // original look on it.
        let showing = self.enhanced.as_ref().and(crate::graphics::showing()).map(|mut e| {
            // No rain in the desert: overcast instead.
            if self.desert && e.weather == crate::graphics::Weather::Rain {
                e.weather = crate::graphics::Weather::Overcast;
            }
            e
        });
        if self.enhanced.is_none()
            && self
                .depth
                .as_ref()
                .is_none_or(|d| (d.1, d.2) != (width, height))
        {
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some((depth.create_view(&Default::default()), width, height));
        }
        let aspect = width as f32 / height.max(1) as f32;
        // The size the enhanced scene is drawn at, before it is stretched
        // over the screen.
        let scene = showing.map_or((width, height), |e| e.scene_size((width, height)));
        let (start, end) = self.fog_range;
        // The enhanced haze: half as thick as the game's fog at its end,
        // and the view reaching on till it has all but hidden the land.
        let haze = (showing.is_some_and(|e| e.distance) && self.fog_range != NO_FOG).then(|| {
            let span = (end - start).max(1.0);
            (start, std::f32::consts::LN_2 / span, (start + 7.0 * span).clamp(end, 20_000.0))
        });
        let far = haze.map_or(end, |h| h.2);
        let proj = Mat4::perspective_rh(camera.fovy(), aspect, 1.0, far);
        // The shadow (0x4df8c0): a camera at the sun, 80000 ft away and so
        // as good as parallel, fitted round the casters, their middle and
        // a radius about them; what it sees is laid over the ground.
        // The enhanced drawing's own shadows take its place.
        let sun_shadows = showing.is_some_and(|e| e.shadows) && self.objects.is_some();
        let shadow = self.shadow.filter(|_| self.shadows_on && self.objects.is_some() && !sun_shadows);
        let shadow_view = shadow.map_or(Mat4::IDENTITY, |(_, centre, light)| {
            let to_sun = (-light).normalize_or(Vec3::Y);
            let up = if to_sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
            let view = Mat4::look_at_rh(centre + to_sun * SHADOW_REACH, centre, up);
            let r = SHADOW_RADIUS;
            Mat4::orthographic_rh(-r, r, -r, r, 0.0, SHADOW_REACH * 2.0) * view
        });
        // The enhanced drawing's hour and weather light the scene in
        // place of its own lights, and its real sky's colour is the haze's.
        let day = showing.map(|e| crate::enhance::daylight(&self.lights, e.time, e.weather));
        let real_sky = showing.is_some_and(|e| e.clouds && e.sky) && self.sky.is_some() && self.sky_on;
        let lights = day.as_ref().map_or(self.lights, |d| d.lights);
        let globals = Globals {
            view_proj: (proj * camera.view()).to_cols_array_2d(),
            camera: camera.position.extend(1.0).into(),
            grid: self.grid,
            fog: [self.fog[0], self.fog[1], self.fog[2], 1.0],
            fog_range: [start, end, 0.0, 0.0],
            ambient: lights.globals().0,
            sun: lights.globals().1,
            to_sun: lights.globals().2,
            shadow_view: shadow_view.to_cols_array_2d(),
            shadow_on: [shadow.is_some() as u32 as f32, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        let seen = crate::cull::Frustum::new(proj * camera.view());
        crate::cull::runs(
            self.blocks.iter().filter(|b| seen.sees(b.0, b.1)).map(|b| b.2.clone()),
            &mut self.runs,
        );
        let (cascades, texels) = crate::enhance::cascades(camera.view(), camera.fovy(), aspect, lights.to_sun);
        if let (Some(e), Some(on)) = (&self.enhanced, showing) {
            let flag = |b: bool| b as u32 as f32;
            let (h_start, density, _) = haze.unwrap_or((0.0, 0.0, far));
            let enh = crate::enhance::EnhGlobals {
                cascades: cascades.map(|c| c.to_cols_array_2d()),
                splits: crate::enhance::splits(),
                flags: [flag(on.lighting), flag(sun_shadows), flag(haze.is_some()), flag(on.sky)],
                haze: [h_start, density, far, on.detail],
                sun: lights.globals().1,
                to_sun: lights.globals().2,
                fog: match (&day, real_sky) {
                    (Some(d), true) => d.horizon.extend(1.0).into(),
                    _ => [self.fog[0], self.fog[1], self.fog[2], 1.0],
                },
                texel: [texels[0], texels[1], texels[2], 0.0],
                world: day.as_ref().map_or([time, 0.0, 0.0, 0.0], |d| [time, d.wet, d.rain, d.cover]),
                ruts: e.ruts.as_ref().filter(|_| on.ruts).map_or([0.0; 4], |r| r.uniform()),
                more: [flag(on.ground), flag(on.grass), flag(real_sky), flag(on.bikes)],
                material_average: e.shared.material_average,
                // The clouds drift 30 ft a second.
                sky: [aspect, flag(on.mud), time * 30.0, 1.0],
                zenith: day.as_ref().map_or([0.0; 4], |d| d.zenith.extend(1.0).into()),
                horizon: day.as_ref().map_or([0.0; 4], |d| d.horizon.extend(1.0).into()),
                smooth: e.smooth.as_ref().filter(|_| on.smooth).map_or([0.0; 4], |g| g.uniform(camera.position)),
                surfaces: [flag(on.surfaces), 0.0, 0.0, 0.0],
            };
            queue.write_buffer(&e.shared.buffer, 0, bytemuck::bytes_of(&enh));
        }
        if let Some(sky) = &self.sky {
            sky.update(queue, proj, camera.view());
        }
        if let Some(objects) = &mut self.objects {
            objects.extras_shown = showing.is_some_and(|e| e.scenery);
            objects.update(queue, time);
            objects.cull(0, &seen);
            if let Some((casters, ..)) = shadow {
                if self.shadow_casters != Some(casters) {
                    objects.set_casters(device, &casters);
                    self.shadow_casters = Some(casters);
                }
                objects.set_shadow_view(queue, shadow_view);
            }
            if sun_shadows {
                objects.set_cascades(queue, &cascades);
                for (i, c) in cascades.iter().enumerate() {
                    objects.cull(1 + i, &crate::cull::Frustum::new(*c));
                }
            }
        }
        {
            // A particle faces the camera, each corner lit by the scene's
            // lights along its own normal, out to its corner and a quarter
            // toward the camera (0x4bec20's +0x2024, by 0x49fa50).
            let view = camera.view();
            let inverse = view.inverse();
            let [right, up, back] = [inverse.x_axis, inverse.y_axis, inverse.z_axis].map(|a| a.truncate());
            let l = &lights;
            let light = |x: f32, y: f32| {
                let n = (right * x + up * y + back * 0.25).normalize_or(back);
                let lit = Vec3::from(l.ambient) + Vec3::from(l.sun) * n.dot(l.to_sun).max(0.0);
                lit.min(Vec3::ONE)
            };
            self.particles.set_light([light(1.0, 1.0), light(-1.0, 1.0), light(1.0, -1.0), light(-1.0, -1.0)]);
            // In the haze, the dirt fades over about the same distance.
            let range = match haze {
                Some(_) => (start, start + 2.5 * (end - start)),
                None => self.fog_range,
            };
            // The enhanced dust clouds stand in for the tyres' dust trail.
            self.particles.hide_trail = showing.is_some_and(|e| e.dust);
            self.particles.prepare(queue, proj, view, (self.fog, range));
        }

        let mut encoder = device.create_command_encoder(&Default::default());
        if let (Some(objects), Some(_)) = (&self.objects, shadow) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.shadow_mask,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            objects.draw_casters(&mut pass);
        }
        if let Some(e) = &mut self.enhanced {
            if let Some(ruts) = &mut e.ruts {
                ruts.paint(device, queue, &mut encoder, &e.shared.ruts.0);
            }
            if let Some(smooth) = &mut e.smooth {
                smooth.cull(camera.position, &seen);
            }
            if let Some(crowd) = &mut e.crowd {
                let bikes: Vec<Vec3> = e.bikes.iter().flatten().copied().collect();
                crowd.watch(queue, &bikes);
                crowd.cull(&seen);
            }
            if let Some(dust) = &mut e.dust {
                dust.prepare(queue, camera.position);
            }
        }
        if let (Some(objects), Some(e), true) = (&self.objects, &self.enhanced, sun_shadows) {
            for (cascade, layer) in e.shared.layers.iter().enumerate() {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("sun shadows"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: layer,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                // Indoors (a stadium, with no sky box) only the moving
                // models cast.
                objects.draw_sun_depth(&mut pass, cascade, self.sky.is_none());
            }
        }
        {
            let (colour, resolve, depth, store) = match &mut self.enhanced {
                Some(e) => {
                    let (colour, resolve, depth) = e.post.scene(device, scene);
                    (colour, resolve, depth, wgpu::StoreOp::Store)
                }
                None => (target, None, &self.depth.as_ref().unwrap().0, wgpu::StoreOp::Discard),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: colour,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store,
                    }),
                    stencil_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0),
                        store: wgpu::StoreOp::Discard,
                    }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let shared = self.enhanced.as_ref().filter(|_| showing.is_some()).map(|e| (&e.shared.group, &e.terrain, &e.group));
            let grass = self
                .enhanced
                .as_ref()
                .filter(|_| showing.is_some_and(|e| e.grass))
                // None in a stadium (it has a crowd): its floor is dirt.
                .filter(|e| e.crowd.is_none())
                .and_then(|e| Some((e.grass.as_ref()?, &e.group, &e.shared.group)));
            if let Some(sky) = self.sky.as_ref().filter(|_| self.sky_on) {
                match shared {
                    Some((group, ..)) => sky.draw_enhanced(&mut pass, group, real_sky),
                    None => sky.draw(&mut pass),
                }
            }
            match shared {
                Some((group, terrain, own)) => {
                    pass.set_pipeline(terrain);
                    pass.set_bind_group(0, own, &[]);
                    pass.set_bind_group(1, group, &[]);
                }
                None => {
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &self.bind_group, &[]);
                }
            }
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
            for run in &self.runs {
                pass.draw_indexed(run.clone(), 0, 0..1);
            }
            if let Some((crowd, own, group)) = self
                .enhanced
                .as_ref()
                .filter(|_| showing.is_some_and(|e| e.scenery))
                .and_then(|e| Some((e.crowd.as_ref()?, &e.group, &e.shared.group)))
            {
                crowd.draw(&mut pass, own, group);
            }
            if let Some((smooth, own, group)) = self
                .enhanced
                .as_ref()
                .filter(|_| showing.is_some_and(|e| e.smooth))
                .and_then(|e| Some((e.smooth.as_ref()?, &e.group, &e.shared.group)))
            {
                smooth.draw(&mut pass, own, group);
            }
            if let Some((grass, own, group)) = grass {
                grass.draw(&mut pass, own, group);
            }
            if let Some(objects) = &self.objects {
                match shared {
                    Some((group, ..)) => objects.draw_enhanced(&mut pass, group),
                    None => objects.draw(&mut pass),
                }
            }
            if let Some((dust, own, group)) = self
                .enhanced
                .as_ref()
                .filter(|_| showing.is_some_and(|e| e.dust))
                .and_then(|e| Some((e.dust.as_ref()?, &e.group, &e.shared.group)))
            {
                dust.draw(&mut pass, own, group);
            }
            if self.particles_on {
                self.particles.draw(&mut pass);
            }
            if let Some((rain, own, group)) = self
                .enhanced
                .as_ref()
                .filter(|_| showing.is_some_and(|e| e.weather == crate::graphics::Weather::Rain))
                .and_then(|e| Some((e.rain.as_ref()?, &e.group, &e.shared.group)))
            {
                rain.draw(&mut pass, own, group);
            }
        }
        if let Some(e) = &self.enhanced {
            // The sun's place on the screen, for its shafts: strongest
            // looking toward it, fading as it leaves the screen, and none
            // indoors.
            let sun = showing.filter(|_| self.sky.is_some() && self.sky_on).and_then(|_| {
                let clip = proj * camera.view() * (camera.position + lights.to_sun * 10_000.0).extend(1.0);
                if clip.w <= 0.0 {
                    return None;
                }
                let ndc = clip.truncate() / clip.w;
                let (x, y) = (ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
                let off = (x - x.clamp(0.0, 1.0)).abs().max((y - y.clamp(0.0, 1.0)).abs());
                let strength = 1.6 * (1.0 - (off / 0.4).clamp(0.0, 1.0)) * (1.0 - day.as_ref().map_or(0.0, |d| d.cover * 0.8));
                (strength > 0.0).then(|| ([x, y, strength], lights.sun))
            });
            let view_proj = proj * camera.view();
            let cinematic = self.cinematic.filter(|_| showing.is_some_and(|e| e.cinematic)).map(|focus| {
                let last = self.last_view_proj.unwrap_or(view_proj);
                (focus, last * view_proj.inverse())
            });
            self.last_view_proj = Some(view_proj);
            e.post.finish(queue, &mut encoder, target, proj, showing, sun, cinematic, (width, height));
        }
        encoder.finish()
    }
}

/// Uploads every terrain texture with its mips into one texture array.
fn terrain_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    (textures, tile_map, sampler): &(wgpu::TextureView, wgpu::TextureView, wgpu::Sampler),
    detail: &wgpu::TextureView,
    shadow_mask: &wgpu::TextureView,
) -> wgpu::BindGroup {
    let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("shadow sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let detail_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("detail sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: crate::graphics::anisotropy(),
        ..Default::default()
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terrain"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(textures),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(tile_map),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(detail),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(&detail_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(shadow_mask),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Sampler(&shadow_sampler),
            },
        ],
    })
}

/// The detail texture with its mips, as the game's own 0..255 values (not
/// sRGB), since it blends in the game's colour space.
fn detail_view(device: &wgpu::Device, queue: &wgpu::Queue, image: &mm2_formats::tga::Image) -> wgpu::TextureView {
    let (w, h) = (image.width.max(1), image.height.max(1));
    let mips = w.min(h).ilog2() + 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("detail"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut level = image.rgba.clone();
    let (mut lw, mut lh) = (w, h);
    for mip in 0..mips {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: mip,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &level,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * lw),
                rows_per_image: Some(lh),
            },
            wgpu::Extent3d {
                width: lw,
                height: lh,
                depth_or_array_layers: 1,
            },
        );
        // A box filter down to the next level.
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let mut next = vec![0u8; (4 * nw * nh) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |dx: u32, dy: u32| {
                        let (sx, sy) = ((2 * x + dx).min(lw - 1), (2 * y + dy).min(lh - 1));
                        level[(4 * (sy * lw + sx) + c) as usize] as u32
                    };
                    next[(4 * (y * nw + x) + c) as usize] = ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
                }
            }
        }
        level = next;
        (lw, lh) = (nw, nh);
    }
    texture.create_view(&Default::default())
}

fn upload_textures(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    textures: &[Image],
) -> wgpu::Texture {
    let size = textures.iter().map(|t| t.size).max().unwrap_or(1);
    let layers = textures.len().max(1) as u32;
    let mip_count = size.trailing_zeros() + 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terrain textures"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layers,
        },
        mip_level_count: mip_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, tex) in textures.iter().enumerate() {
        // Smaller textures fill only the top mips; they are not used by tiles.
        let skip = (size / tex.size).trailing_zeros();
        for (level, data) in tex.levels.iter().enumerate() {
            let side = tex.size >> level;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32 + skip,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(side * 4),
                    rows_per_image: Some(side),
                },
                wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    texture
}

/// Converts game coordinates (left-handed, +z north) to the viewer's
/// right-handed world by flipping z, so the world is not mirrored.
/// No fog: the camera's own far plane (0x42f800's 100000).
const NO_FOG: (f32, f32) = (100_000.0, 100_000.0);

/// The fog's start and end, ft, as [`TerrainRenderer::set_visibility`].
pub fn fog_range(quality: u32, visibility: f32, haziness: f32) -> (f32, f32) {
    const FAR: f32 = 768.0;
    const NEAR: f32 = 256.0;
    const CLEAR: f32 = 128.0;
    let mut cut = (9 - quality.min(9)) as f32 * 0.1;
    if visibility - cut < 0.0 {
        cut = visibility;
    }
    let end = (FAR - NEAR) * (visibility - cut) + NEAR;
    let start = (1.0 - haziness) * (end - CLEAR);
    (start, end)
}

pub fn from_game(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, -v.z)
}

/// A camera that starts above the middle of the terrain, looking along it.
pub fn start_camera(terrain: &Terrain) -> crate::camera::Camera {
    let mid = (terrain.size / 2) as f32;
    let ground = terrain.height(terrain.size / 2, terrain.size / 2);
    let s = terrain.scale;
    crate::camera::Camera {
        position: from_game(Vec3::new(mid, ground + 60.0, mid - 160.0) * s),
        yaw: -std::f32::consts::FRAC_PI_2,
        pitch: -0.45,
        fov: crate::camera::FOV,
    }
}
