//! What the enhanced drawing ([`crate::graphics`]) shares between the
//! terrain, the objects and the sky: the sun's shadow map in three
//! cascades, and the settings each shader reads.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4Swizzles};

/// The cascades' depth, as the sun sees the objects.
pub const SHADOW_DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Texels a side of each cascade.
pub const SHADOW_SIDE: u32 = 2048;
pub const CASCADES: usize = 3;
/// How far each cascade reaches from the camera, ft; past the last the
/// shadows fade out.
const SPLITS: [f32; CASCADES] = [36.0, 140.0, 520.0];
/// How far beyond a cascade toward the sun a caster may stand, ft.
const SUNWARD: f32 = 1500.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct EnhGlobals {
    /// From the drawing to each cascade's map.
    pub cascades: [[[f32; 4]; 4]; CASCADES],
    /// Each cascade's reach, and in w where the shadows end.
    pub splits: [f32; 4],
    /// Which upgrades show: x lighting, y shadows, z haze, w sky.
    pub flags: [f32; 4],
    /// The haze: where it starts, how thick it is (per ft) and the far
    /// plane; w how strongly the new ground's detail shows.
    pub haze: [f32; 4],
    /// The sun's colour and the way to it, and the fog's colour (linear).
    pub sun: [f32; 4],
    pub to_sun: [f32; 4],
    pub fog: [f32; 4],
    /// Each cascade's texel, ft.
    pub texel: [f32; 4],
    /// The clock (s), how wet the ground is, how hard it rains, and how
    /// much of the sky is cloud, 0 to 1.
    pub world: [f32; 4],
    /// The ruts' map: where its corner is (x, z, drawing coordinates), how
    /// many ft it spans, and whether it is drawn.
    pub ruts: [f32; 4],
    /// Which of the bigger upgrades show: x the new ground, y grass, z the
    /// real sky, w the bikes' paint.
    pub more: [f32; 4],
    /// The photo materials' average colours (linear).
    pub material_average: [[f32; 4]; crate::materials::COUNT],
    /// The real sky: x the screen's width over its height, y whether the
    /// bikes get muddy, z cloud drift (ft), w the exposure.
    pub sky: [f32; 4],
    /// The sky's colour overhead and at the horizon, linear.
    pub zenith: [f32; 4],
    pub horizon: [f32; 4],
    /// The smooth ground's patch: its corner (terrain units), cells a
    /// side, and whether it is drawn.
    pub smooth: [f32; 4],
    /// x whether the trial track's objects get the photo materials.
    pub surfaces: [f32; 4],
}

/// The enhanced drawing's shared parts.
pub struct Shared {
    pub layout: wgpu::BindGroupLayout,
    pub buffer: wgpu::Buffer,
    pub group: wgpu::BindGroup,
    /// Each cascade's layer of the map, to draw the casters into.
    pub layers: Vec<wgpu::TextureView>,
    pub material_average: [[f32; 4]; crate::materials::COUNT],
    /// The ruts' map, to draw the tyres into: its view and texels a side.
    pub ruts: (wgpu::TextureView, u32),
}

/// Texels a side of the ruts' map, over the whole terrain.
pub const RUT_SIDE: u32 = 8192;
pub const RUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

impl Shared {
    /// With `ruts`, a map of tyre ruts over the terrain.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, ruts: bool) -> Shared {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("enhanced"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: texture(wgpu::TextureViewDimension::D2Array, wgpu::TextureSampleType::Float { filterable: true }),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: texture(wgpu::TextureViewDimension::D2Array, wgpu::TextureSampleType::Float { filterable: true }),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: sampler(),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: texture(wgpu::TextureViewDimension::D2, wgpu::TextureSampleType::Float { filterable: true }),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: sampler(),
                    count: None,
                },
            ],
        });
        let materials = crate::materials::Materials::new(device, queue);
        let rut_side = if ruts { RUT_SIDE } else { 1 };
        let rut_map = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ruts"),
            size: wgpu::Extent3d {
                width: rut_side,
                height: rut_side,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: RUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let rut_view = rut_map.create_view(&Default::default());
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ground materials"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ruts"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("enhanced"),
            size: std::mem::size_of::<EnhGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let map = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sun shadows"),
            size: wgpu::Extent3d {
                width: SHADOW_SIDE,
                height: SHADOW_SIDE,
                depth_or_array_layers: CASCADES as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_DEPTH,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let layers = (0..CASCADES as u32)
            .map(|layer| {
                map.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sun shadows"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("enhanced"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&map.create_view(
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&materials.albedo),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&materials.normal),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&repeat),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&rut_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&clamp),
                },
            ],
        });
        Shared {
            layout,
            buffer,
            group,
            layers,
            material_average: materials.average,
            ruts: (rut_view, rut_side),
        }
    }
}

/// The cascades for a camera at `view` with `fovy` and `aspect`, lit from
/// `to_sun`: each a box along the light round a slice of what the camera
/// sees, moved only in whole texels so the shadows' edges hold still as
/// the camera moves. Returns each one's matrix and its texel, ft.
pub fn cascades(view: Mat4, fovy: f32, aspect: f32, to_sun: Vec3) -> ([Mat4; CASCADES], [f32; CASCADES]) {
    let inverse = view.inverse();
    let (ty, tx) = ((fovy / 2.0).tan(), (fovy / 2.0).tan() * aspect);
    let up = if to_sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let light = Mat4::look_at_rh(Vec3::ZERO, -to_sun, up);
    let mut near = 0.5;
    let mut out = [Mat4::IDENTITY; CASCADES];
    let mut texels = [0.0; CASCADES];
    for (i, &far) in SPLITS.iter().enumerate() {
        // The slice's bounding sphere, so its size never changes as the
        // camera turns.
        let corners: Vec<Vec3> = [near, far]
            .iter()
            .flat_map(|&d| {
                [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
                    .map(|(sx, sy)| (inverse * glam::Vec4::new(sx * tx * d, sy * ty * d, -d, 1.0)).xyz())
            })
            .collect();
        let centre = corners.iter().copied().sum::<Vec3>() / corners.len() as f32;
        let radius = corners.iter().map(|c| c.distance(centre)).fold(0.0, f32::max);
        let radius = (radius * 16.0).ceil() / 16.0;
        let texel = 2.0 * radius / SHADOW_SIDE as f32;
        // Snap the centre to the texel grid as the light sees it.
        let mut c = light.transform_point3(centre);
        c.x = (c.x / texel).round() * texel;
        c.y = (c.y / texel).round() * texel;
        let snapped = light.inverse().transform_point3(c);
        let eye = snapped + to_sun * (radius + SUNWARD);
        let look = Mat4::look_at_rh(eye, snapped, up);
        let proj = Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.0, 2.0 * radius + SUNWARD);
        out[i] = proj * look;
        texels[i] = texel;
        near = far;
    }
    (out, texels)
}

/// Each cascade's reach, and where the shadows end.
pub fn splits() -> [f32; 4] {
    [SPLITS[0], SPLITS[1], SPLITS[2], SPLITS[2]]
}

/// Makes a shader from `source`, with the enhanced drawing's shared
/// declarations put in where it asks for them.
pub fn shader(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    let source = source
        .replace("#include \"enhanced.wgsl\"", include_str!("enhanced.wgsl"))
        .replace("#include \"enhanced_fns.wgsl\"", include_str!("enhanced_fns.wgsl"));
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

/// A bind group layout of `entries`: each binding, where it is seen, and
/// what it is. The enhanced pipelines name their layouts, since a layout
/// a pipeline works out for itself fits no other pipeline.
pub fn layout(device: &wgpu::Device, label: &str, entries: &[(u32, wgpu::ShaderStages, wgpu::BindingType)]) -> wgpu::BindGroupLayout {
    let entries: Vec<wgpu::BindGroupLayoutEntry> = entries
        .iter()
        .map(|&(binding, visibility, ty)| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        })
        .collect();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}

pub fn uniform() -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    }
}

pub fn texture(dimension: wgpu::TextureViewDimension, sample_type: wgpu::TextureSampleType) -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type,
        view_dimension: dimension,
        multisampled: false,
    }
}

pub fn sampler() -> wgpu::BindingType {
    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
}

/// The enhanced drawing's light and sky for the hour and weather it is
/// asked for ([`crate::graphics::Enhanced`]): none of it the game's.
pub struct Daylight {
    pub lights: crate::renderer::SceneLights,
    /// The sky's colour overhead and at the horizon (linear).
    pub zenith: Vec3,
    pub horizon: Vec3,
    /// How much of the sky is cloud, how wet the ground is and how hard
    /// it rains, each 0 to 1.
    pub cover: f32,
    pub wet: f32,
    pub rain: f32,
}

/// The light and sky from the scene's own `lights` at `time` (an hour,
/// 0-24; at the scene's own sun when none) in `weather`.
pub fn daylight(lights: &crate::renderer::SceneLights, time: Option<f32>, weather: crate::graphics::Weather) -> Daylight {
    use crate::graphics::Weather;
    let mut l = *lights;
    let sun_strength = Vec3::from(lights.sun).max_element().max(0.3);
    if let Some(hour) = time {
        // Up at 6 and down at 18.6, highest at half past noon; round from
        // the east through the south as the scene's sun is at noon.
        let flat = Vec3::new(lights.to_sun.x, 0.0, lights.to_sun.z).normalize_or(Vec3::X);
        let day = ((hour - 6.0) / 12.6).clamp(-0.1, 1.1);
        let high = (day * std::f32::consts::PI).sin() * 62f32.to_radians();
        let round = (day - 0.5) * std::f32::consts::PI * 0.9;
        let (s, c) = round.sin_cos();
        let way = Vec3::new(flat.x * c - flat.z * s, 0.0, flat.x * s + flat.z * c);
        l.to_sun = (way * high.cos() + Vec3::Y * high.sin()).normalize();
        let up = l.to_sun.y;
        // Low, the sun is redder and weaker; the ambient bluer and dimmer.
        let warm = Vec3::new(1.0, 0.55, 0.32).lerp(Vec3::new(1.0, 0.96, 0.9), smoothstep(0.02, 0.45, up));
        let strength = sun_strength * smoothstep(-0.04, 0.18, up);
        l.sun = (warm * strength).into();
        let ambient = Vec3::from(lights.ambient).max_element().max(0.25);
        let tint = Vec3::new(0.75, 0.8, 1.0).lerp(Vec3::ONE, smoothstep(0.05, 0.4, up));
        l.ambient = (tint * ambient * (0.5 + 0.5 * smoothstep(-0.1, 0.3, up))).into();
    }
    let (cover, wet, rain) = match weather {
        Weather::Clear => (0.42, 0.0, 0.0),
        Weather::Overcast => (0.85, 0.15, 0.0),
        Weather::Rain => (0.97, 1.0, 1.0),
    };
    // Under cloud, less sun and more of the sky's light round about.
    let dim = 1.0 - smoothstep(0.5, 1.0, cover) * 0.7;
    let sun = Vec3::from(l.sun);
    l.sun = (sun * dim).into();
    l.ambient = (Vec3::from(l.ambient) + sun * (1.0 - dim) * 0.35).into();
    let up = l.to_sun.y;
    let day = smoothstep(-0.08, 0.25, up);
    let dusk = smoothstep(0.35, 0.02, up) * smoothstep(-0.12, 0.0, up);
    let zenith = Vec3::new(0.02, 0.025, 0.06).lerp(Vec3::new(0.13, 0.27, 0.62), day);
    let horizon = Vec3::new(0.03, 0.035, 0.06).lerp(Vec3::new(0.58, 0.68, 0.82), day).lerp(Vec3::new(0.95, 0.5, 0.28), dusk * 0.8);
    let grey = Vec3::splat(0.5 * (0.08 + 0.92 * day)) * Vec3::new(0.95, 0.98, 1.05);
    let overcast = smoothstep(0.5, 1.0, cover);
    Daylight {
        lights: l,
        zenith: zenith.lerp(grey * 0.9, overcast),
        horizon: horizon.lerp(grey, overcast),
        cover,
        wet,
        rain,
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
