//! Dirt, dust, exhaust and sparks (Particles.cpp): the `ParticleManager`
//! (0x4beae0, updated by 0x4bf240, drawn by 0x4bf3b0) holds up to 1000
//! particles, cut from the scene's `ParticleTextureMapName` sheet in
//! `RES/GLOBAL.RES` (64×64; 0x4bec20 lays out its cells: 0–13 the 16×16
//! clods, 14–17 the faint 8×8 puffs, 29–44 the 4×4 chunks, 45–60 the 4×4
//! sparks). Each bike feeds its emitters once a frame (0x507340):
//!
//! * `DustParticleEmitter` (0x4bd150/0x4bd200) from a tyre on dusty ground
//!   (0x52f430): a trail of growing, fading clods along the way the tyre
//!   went, sized by its speed.
//! * `DirtChunkParticleEmitter` (0x4bd540/0x4bd690) from the spinning rear
//!   tyre on dirt (0x52f650): 200 chunks a second thrown out sideways and
//!   up, falling.
//! * `SteamParticleEmitter` (0x4be690/0x4be7c0) at the exhaust: puffs when
//!   the throttle opens below 22 ft/s (0x496880).
//! * `SparkParticleEmitter` (0x4bdfc0/0x4be130) where the bike hits a
//!   solid (0x405fb0): 2400 a second for that frame.
//! * The thrown rider sliding on the ground leaves dust (0x537690).
//!
//! Not ported: the dust and exhaust darken in shadow (0x507340 casts a
//! ray to the sun every sixth frame), and the crashed bike's frame
//! scraping the ground raises dust (0x52f430's third pass).

use crate::renderer::{from_game, DEPTH_FORMAT};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

/// The manager's pool (0x4bec20 allocates 1000).
const MOST: usize = 1000;
/// The manager's pull on falling particles, ft/s² (+0x1f98).
const GRAVITY: Vec3 = Vec3::new(0.0, -64.0, 0.0);

/// What a particle does (+0x34): fades from half (2), moves (4), grows
/// (8), falls (0x10), and is drawn in its own colour instead of lit (0x800).
const FADES: u32 = 2;
const MOVES: u32 = 4;
const GROWS: u32 = 8;
const FALLS: u32 = 0x10;
const COLOURED: u32 = 0x800;
/// The port's own mark on the tyres' dust trail, which the enhanced
/// drawing's dust clouds can stand in for.
const TRAIL: u32 = 0x1_0000;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
    colour: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [f32; 16],
    camera: [f32; 4],
    fog: [f32; 4],
    fog_range: [f32; 4],
}

/// One particle (0x48 bytes in the game).
#[derive(Debug, Clone, Copy)]
struct Particle {
    /// Game coordinates, ft and ft/s.
    position: Vec3,
    velocity: Vec3,
    /// RGB, 0 to 1 (+0xc), for a `COLOURED` one.
    colour: Vec3,
    cell: usize,
    /// Age and life, ms (+0x14, +0x18).
    age: f32,
    life: f32,
    /// Half its width, ft (+0x1c), and its growth, ft/s (+0x20).
    size: f32,
    growth: f32,
    flags: u32,
}

/// Cell `c`'s corners on the sheet, 0 to 1 (0x4bec20's table at 0x68f298).
fn cell_uv(c: usize) -> ([f32; 2], f32) {
    match c {
        0..=13 => ([(c % 4) as f32 * 0.25, (c / 4) as f32 * 0.25], 0.25),
        14..=17 => {
            let k = c - 14;
            ([0.25 + (k % 2) as f32 * 0.125, 0.75 + (k / 2) as f32 * 0.125], 0.125)
        }
        29..=44 => {
            let k = c - 29;
            ([0.5 + (k % 4) as f32 * 0.0625, 0.75 + (k / 4) as f32 * 0.0625], 0.0625)
        }
        45..=60 => {
            let k = c - 45;
            ([0.75 + (k % 4) as f32 * 0.0625, 0.75 + (k / 4) as f32 * 0.0625], 0.0625)
        }
        // Cells 18–28 are left empty in the game's table.
        _ => ([0.0, 0.0], 0.0),
    }
}

/// The C library's `rand()`, which fills the emitters' tables.
#[derive(Debug, Clone)]
struct Rand(u32);

impl Rand {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7fff) as f32 * (1.0 / 32768.0)
    }
}

/// What a tyre on the ground stands on.
#[derive(Debug, Clone, Copy)]
pub struct Tyre {
    pub at: Vec3,
    /// The surface's GenerateDust and GenerateDirt.
    pub dust: bool,
    pub dirt: bool,
}

/// One bike this frame, as 0x507340 feeds its emitters.
#[derive(Debug, Clone, Copy, Default)]
pub struct Feed {
    /// The tyres on the ground, front then rear (the rear is driven).
    pub tyres: [Option<Tyre>; 2],
    /// The rear's traction (+0x2b8), spin (+0x2bc) and throw (+0x290 over
    /// the step).
    pub traction: f32,
    pub spin: f32,
    pub throw: f32,
    /// The bike's right, and the bars' turn (`f_4b8`).
    pub right: Vec3,
    pub bars: f32,
    /// The dust and dirt run (0x496dc0): the bike moves along itself at
    /// 0.2 ft/s or more.
    pub running: bool,
    /// Where the exhaust is; the throttle (a computer rider's speed),
    /// which puffs it as it opens; and whether it can: not crashed, below
    /// 22 ft/s (0x496880).
    pub exhaust: Vec3,
    pub gas: f32,
    pub gas_ok: bool,
    /// Where the bike met a solid object.
    pub hit: Option<Vec3>,
    /// The thrown rider's body while it touches the ground.
    pub slide: Option<Vec3>,
    /// The bike was put somewhere new: no trail from where it was.
    pub moved: bool,
}

/// The dust trail (0x4bd150).
#[derive(Debug, Clone, Default)]
struct Dust {
    first: bool,
    at: Vec3,
    last: Vec3,
    active: bool,
    started: bool,
    size: f32,
    last_size: f32,
    /// The size factor from the tyre's spin (+0x78; 0 for 1).
    factor: f32,
    cell: usize,
}

/// A throwing emitter: chunks or sparks (0x4bd540, 0x4bdfc0).
#[derive(Debug, Clone, Default)]
struct Thrower {
    first: bool,
    at: Vec3,
    last: Vec3,
    active: bool,
    /// The base velocity (+0x74 or +0x78).
    velocity: Vec3,
    /// Seconds idle (+0x64, sparks only).
    idle: f32,
    cell: usize,
    sizes: Vec<f32>,
    size_at: usize,
    scales: Vec<Vec3>,
    scale_at: usize,
}

/// The exhaust (0x4be690).
#[derive(Debug, Clone, Default)]
struct Steam {
    first: bool,
    at: Vec3,
    last: Vec3,
    on: bool,
    /// Seconds left to puff (+0x44), and five puffs at once (+0x70).
    left: f32,
    burst: bool,
    cell: usize,
    growths: Vec<f32>,
    growth_at: usize,
}

#[derive(Debug, Clone)]
struct Emitters {
    dust: Dust,
    chunks: Thrower,
    sparks: Thrower,
    steam: Steam,
    /// Throttle opening last frame (+0x20e), and the throttle (+0x5b0).
    opening: bool,
    gas: f32,
    /// The thrown rider's last place and the clod it was on (+0x120).
    slide: Option<Vec3>,
    slide_cell: usize,
}

impl Emitters {
    fn new(rand: &mut Rand) -> Emitters {
        let chunks = Thrower {
            first: true,
            cell: 29,
            sizes: (0..256).map(|_| rand.next() * 0.25).collect(),
            scales: (0..32)
                .map(|_| Vec3::new(rand.next() + 0.5, rand.next() + 0.5, rand.next() + 0.5))
                .collect(),
            ..Default::default()
        };
        let sparks = Thrower {
            first: true,
            cell: 45,
            velocity: Vec3::new(0.0, 2.0, 0.0),
            sizes: (0..256).map(|_| rand.next() * 0.075).collect(),
            // The spark's own velocity, ±5 ft/s each way.
            scales: (0..8)
                .map(|_| Vec3::new(rand.next() * 10.0 - 5.0, rand.next() * 10.0 - 5.0, rand.next() * 10.0 - 5.0))
                .collect(),
            ..Default::default()
        };
        let steam = Steam {
            first: true,
            cell: 14,
            growths: (0..64)
                .map(|_| {
                    let r = rand.next();
                    r + r + 1.0
                })
                .collect(),
            ..Default::default()
        };
        Emitters {
            dust: Dust {
                first: true,
                ..Default::default()
            },
            chunks,
            sparks,
            steam,
            opening: false,
            gas: 0.0,
            slide: None,
            slide_cell: 0,
        }
    }
}

/// The particles in flight, their emitters, and how they are drawn.
pub struct Particles {
    list: Vec<Particle>,
    rand: Rand,
    bikes: Vec<Emitters>,
    /// The scene's light on the corners of a particle facing the camera,
    /// 0 to 1: top left, top right, bottom right, bottom left.
    light: [Vec3; 4],
    gpu: Option<Gpu>,
    /// Leaves the tyres' dust trail undrawn (still made, so the rest is
    /// as the game has it), for the enhanced drawing's dust.
    pub hide_trail: bool,
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    count: u32,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Particles {
    pub fn new() -> Particles {
        Particles {
            list: Vec::with_capacity(MOST),
            rand: Rand(1),
            bikes: Vec::new(),
            light: [Vec3::ONE; 4],
            gpu: None,
            hide_trail: false,
        }
    }

    /// Adds a particle while the pool has room (0x4bf1a0).
    fn add(&mut self, p: Particle) {
        if self.list.len() < MOST {
            self.list.push(p);
        }
    }

    /// Feeds bike `i`'s emitters for this frame and runs them (0x507340,
    /// then each emitter's update).
    pub fn bike(&mut self, i: usize, feed: &Feed, dt: f32) {
        while self.bikes.len() <= i {
            let e = Emitters::new(&mut self.rand);
            self.bikes.push(e);
        }
        if dt <= 0.0 {
            return;
        }
        let mut e = std::mem::replace(&mut self.bikes[i], Emitters::new(&mut Rand(1)));
        e.dust.active = false;
        e.chunks.active = false;
        // Put back on the course (or the start): no trail from where it was.
        let feed = &Feed {
            moved: feed.moved || feed.exhaust.distance(e.steam.at) > 30.0,
            ..*feed
        };
        if feed.running {
            feed_dust(&mut e.dust, feed);
            feed_chunks(&mut e.chunks, feed);
        }
        if feed.moved {
            for (at, last) in [
                (e.dust.at, &mut e.dust.last),
                (e.chunks.at, &mut e.chunks.last),
                (e.steam.at, &mut e.steam.last),
            ] {
                *last = at;
            }
        }
        // The exhaust follows the bike every frame.
        e.steam.last = e.steam.at;
        e.steam.at = feed.exhaust;
        if feed.moved {
            e.steam.last = e.steam.at;
        }
        // Opening the throttle puffs for a quarter second (0x4bea50); held
        // opening, five puffs a frame (0x4bea70).
        let opening = feed.gas_ok && e.gas < feed.gas;
        e.gas = feed.gas;
        if !opening || e.opening {
            if opening {
                e.steam.left = 1e-4;
                e.steam.on = true;
                e.steam.burst = true;
            }
        } else {
            e.steam.on = true;
            e.steam.left = 0.25;
        }
        e.opening = opening;
        if let Some(at) = feed.hit {
            e.sparks.last = e.sparks.at;
            e.sparks.at = at;
            e.sparks.velocity = Vec3::new(0.0, 12.0, 0.0);
            e.sparks.active = true;
        }
        self.dust(&mut e.dust, dt);
        self.chunks(&mut e.chunks, dt);
        self.sparks(&mut e.sparks, dt);
        self.steam(&mut e.steam, dt);
        self.slide(&mut e, feed.slide, dt);
        self.bikes[i] = e;
    }

    /// 0x4bd200: puffs spaced along the way the tyre went, sized by its
    /// speed; they start a quarter through their life and grow.
    fn dust(&mut self, d: &mut Dust, dt: f32) {
        if !d.active {
            d.started = false;
            d.last = d.at;
            return;
        }
        if !d.started {
            d.started = true;
            return;
        }
        if d.first {
            d.first = false;
            d.last = d.at;
            return;
        }
        let way = d.at - d.last;
        let run = way.length();
        let speed = run / dt * if d.factor == 0.0 { 1.0 } else { d.factor };
        d.size = (speed * 0.013_333).clamp(0.1, 2.0);
        let life = (d.size * 2000.0).clamp(500.0, 1000.0);
        let n = run / ((d.size + d.last_size) * 0.3);
        let mut i = 0;
        while (i as f32) < n {
            let k = i as f32;
            let size = k * ((d.size - d.last_size) / n) + d.size;
            self.add(Particle {
                position: d.last + way * (k / n),
                velocity: Vec3::ZERO,
                colour: Vec3::ONE,
                cell: d.cell,
                age: life * 0.25,
                life,
                size,
                growth: size + size,
                flags: GROWS | FADES | TRAIL,
            });
            d.cell = if d.cell >= 12 { 0 } else { d.cell + 1 };
            i += 1;
        }
        d.last_size = d.size;
    }

    /// 0x4bd690 and 0x4be130: `rate` a second along the way from the last
    /// place, each thrown with the base velocity scaled by the next of the
    /// emitter's random factors (chunks) or at its own random velocity
    /// (sparks), as if thrown through the frame.
    fn throw(&mut self, t: &mut Thrower, dt: f32, spec: Throw) {
        let n = dt * spec.rate;
        let way = t.at - t.last;
        let mut i = 0;
        while (i as f32) < n {
            let k = i as f32;
            let flown = dt * (n - k) / n;
            let size = t.sizes[t.size_at];
            let scale = t.scales[t.scale_at % t.scales.len()];
            let velocity = if spec.own_velocity {
                scale
            } else {
                (t.velocity + GRAVITY * flown) * scale
            };
            self.add(Particle {
                position: t.last + way * (k / n) + t.velocity * flown,
                velocity,
                colour: Vec3::ONE,
                cell: t.cell,
                age: spec.age,
                life: spec.life,
                size,
                growth: size + size,
                flags: MOVES | FALLS,
            });
            t.cell = if t.cell >= spec.cells.1 { spec.cells.0 } else { t.cell + 1 };
            t.scale_at = if t.scale_at >= 32 { 0 } else { t.scale_at + 1 };
            t.size_at = if t.size_at >= 255 { 0 } else { t.size_at + 1 };
            i += 1;
        }
    }

    fn chunks(&mut self, t: &mut Thrower, dt: f32) {
        if t.first {
            t.first = false;
            t.last = t.at;
            return;
        }
        if !t.active {
            return;
        }
        self.throw(
            t,
            dt,
            Throw {
                rate: 200.0,
                age: 0.0,
                life: 375.0,
                cells: (29, 44),
                own_velocity: false,
            },
        );
    }

    fn sparks(&mut self, t: &mut Thrower, dt: f32) {
        if t.first {
            t.first = false;
            t.last = t.at;
            return;
        }
        if !t.active {
            t.idle += dt;
            return;
        }
        // From a standstill they come from the one place.
        if t.idle > 0.0 {
            t.last = t.at;
        }
        self.throw(
            t,
            dt,
            Throw {
                rate: 2400.0,
                age: 0.0,
                life: 500.0,
                cells: (45, 60),
                own_velocity: true,
            },
        );
        t.active = false;
        t.idle = 0.0;
    }

    /// 0x4be7c0: puffs back along the way the exhaust went, 0.03 ft apart
    /// (five at once for a burst), white and fading.
    fn steam(&mut self, s: &mut Steam, dt: f32) {
        if !s.on {
            return;
        }
        if s.left <= 0.0 {
            s.left = 0.0;
            s.on = false;
            s.burst = false;
            return;
        }
        if s.first {
            s.first = false;
            s.last = s.at;
            return;
        }
        s.left -= dt;
        let way = s.at - s.last;
        let spacing = way.length() / (0.1 * 0.3);
        let step = 1.0 / spacing;
        let n = if s.burst { 5.0 } else { spacing };
        let grow = dt * 3.0;
        let mut i = 0;
        while (i as f32) < n {
            let k = i as f32;
            let at = if step.is_finite() { s.at - way * (step * k) } else { s.at };
            self.add(Particle {
                position: at,
                velocity: Vec3::ZERO,
                colour: Vec3::ONE,
                cell: s.cell,
                age: 0.0,
                life: 500.0,
                size: k * ((grow - 0.1) / n) + 0.1,
                growth: s.growths[s.growth_at],
                flags: COLOURED | GROWS | FADES,
            });
            s.growth_at = if s.growth_at >= 63 { 0 } else { s.growth_at + 1 };
            s.cell = if s.cell >= 17 { 14 } else { s.cell + 1 };
            i += 1;
        }
    }

    /// 0x537410: the thrown rider's body, touching the ground, leaves a
    /// trail of puffs from where it was, sized by its speed (at least a
    /// quarter foot).
    fn slide(&mut self, e: &mut Emitters, at: Option<Vec3>, dt: f32) {
        let (Some(to), Some(from)) = (at, e.slide) else {
            e.slide = at;
            return;
        };
        e.slide = Some(to);
        let way = to - from;
        let run = way.length();
        let size = (run / dt * 0.013_333).clamp(0.25, 2.0);
        let life = (size * 2000.0).clamp(500.0, 1000.0);
        let n = ((run / (size * 0.6)) as i32).max(1);
        for k in 0..n {
            self.add(Particle {
                position: from + way * (k as f32 / n as f32),
                velocity: Vec3::ZERO,
                colour: Vec3::ONE,
                cell: e.slide_cell,
                age: life * 0.25,
                life,
                size,
                growth: size + size,
                flags: GROWS | FADES,
            });
            e.slide_cell = if e.slide_cell >= 12 { 0 } else { e.slide_cell + 1 };
        }
    }

    /// Ages, grows and moves everything by `dt` (0x4bf240). A particle
    /// past its life is swapped for the last, which then waits a frame.
    pub fn update(&mut self, dt: f32) {
        let mut i = 0;
        while i < self.list.len() {
            let p = &mut self.list[i];
            p.age += dt * 1000.0;
            if p.age <= p.life {
                if p.flags & GROWS != 0 {
                    p.size += dt * p.growth;
                }
                if p.flags & MOVES != 0 {
                    p.position += dt * p.velocity;
                }
                if p.flags & FALLS != 0 {
                    p.velocity += dt * GRAVITY;
                }
            } else {
                self.list.swap_remove(i);
            }
            i += 1;
        }
    }

    /// Takes over what `other` has in flight (made before the drawing was).
    pub fn adopt(&mut self, other: Particles) {
        self.list = other.list;
        self.bikes = other.bikes;
        self.rand = other.rand;
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// The scene's light on the corners of a particle facing the camera
    /// (0x4bf3b0 lights the quad by the scene's lights), 0 to 1: top left,
    /// top right, bottom right, bottom left.
    pub fn set_light(&mut self, light: [Vec3; 4]) {
        self.light = light;
    }

    /// Readies the drawing, with the sheet as RGBA.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, sheet: &mm2_formats::tga::Image, format: wgpu::TextureFormat, samples: u32) {
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("dirt"),
                size: wgpu::Extent3d {
                    width: sheet.width,
                    height: sheet.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &sheet.rgba,
        );
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dirt"),
            size: (MOST * 6 * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dirt globals"),
            contents: bytemuck::bytes_of(&Globals::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("dirt"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("particles.wgsl"));
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dirt"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dirt"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        self.gpu = Some(Gpu {
            pipeline,
            vertices,
            count: 0,
            globals,
            bind_group,
        });
    }

    /// Builds the squares facing the camera (`view` in drawing space),
    /// farthest first (0x4bf3b0 sorts them), each drawn half a foot above
    /// its place. `fog` is the fog's colour (linear) and its start and end.
    pub fn prepare(&mut self, queue: &wgpu::Queue, proj: Mat4, view: Mat4, fog: ([f32; 3], (f32, f32))) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let inverse = view.inverse();
        let right = inverse.x_axis.truncate();
        let up = inverse.y_axis.truncate();
        let eye = inverse.w_axis.truncate();
        let back = inverse.z_axis.truncate();
        let mut order: Vec<(f32, &Particle)> = self
            .list
            .iter()
            .filter(|p| !(self.hide_trail && p.flags & TRAIL != 0))
            .map(|p| ((from_game(p.position) - eye).dot(back), p))
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0).reverse());
        let mut out = Vec::with_capacity(order.len() * 6);
        for (_, p) in order {
            let centre = from_game(p.position + Vec3::Y * 0.5);
            let (corner, side) = cell_uv(p.cell);
            let alpha = if p.flags & FADES != 0 {
                (((1.0 - p.age / p.life) * 127.5 - 0.5).round() / 255.0).max(0.0)
            } else {
                1.0
            };
            let at = |dx: f32, dy: f32, k: usize| {
                let rgb = if p.flags & COLOURED != 0 { p.colour } else { self.light[k] };
                Vertex {
                    position: (centre + right * dx * p.size + up * dy * p.size).into(),
                    uv: [corner[0] + (dx + 1.0) * 0.5 * side, corner[1] + (1.0 - dy) * 0.5 * side],
                    colour: [rgb.x, rgb.y, rgb.z, alpha],
                }
            };
            let quad = [at(-1.0, 1.0, 0), at(1.0, 1.0, 1), at(1.0, -1.0, 2), at(-1.0, -1.0, 3)];
            out.extend([0, 1, 2, 0, 2, 3].map(|i| quad[i]));
        }
        gpu.count = out.len() as u32;
        if !out.is_empty() {
            queue.write_buffer(&gpu.vertices, 0, bytemuck::cast_slice(&out));
        }
        let (colour, (start, end)) = fog;
        let globals = Globals {
            view_proj: (proj * view).to_cols_array(),
            camera: eye.extend(1.0).into(),
            fog: [colour[0], colour[1], colour[2], 1.0],
            fog_range: [start, end, 0.0, 0.0],
        };
        queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(gpu) = self.gpu.as_ref().filter(|g| g.count > 0) {
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &gpu.bind_group, &[]);
            pass.set_vertex_buffer(0, gpu.vertices.slice(..));
            pass.draw(0..gpu.count, 0..1);
        }
    }
}

struct Throw {
    rate: f32,
    age: f32,
    life: f32,
    cells: (usize, usize),
    own_velocity: bool,
}

/// 0x52f430: the driven tyre on dusty ground, its puffs scaled by its spin
/// (×20, at least 1); else the first other tyre there, scaled by nothing.
fn feed_dust(d: &mut Dust, feed: &Feed) {
    let rear = feed.tyres[1].filter(|t| t.dust).map(|t| (t.at, (feed.spin * 20.0).max(1.0)));
    let any = || feed.tyres.iter().flatten().find(|t| t.dust).map(|t| (t.at, 0.0));
    if let Some((at, factor)) = rear.or_else(any) {
        d.last = d.at;
        d.at = at;
        d.factor = factor;
        if feed.moved {
            d.last = d.at;
        }
        d.active = true;
    }
}

/// 0x52f650: the driven tyre on dirt throws chunks while it spins
/// (traction under 0.95): out to the side away from the bars, up with the
/// spin, all by how hard it throws, each way at most 15, 18 and 15 ft/s.
/// Gripping, it throws none; with the driven tyre off dirt, the other
/// tyre on dirt throws them on as before (its +0x288 is always 1).
fn feed_chunks(t: &mut Thrower, feed: &Feed) {
    let place = |t: &mut Thrower, at: Vec3| {
        t.last = t.at;
        t.at = at;
        if feed.moved {
            t.last = t.at;
        }
        t.active = true;
    };
    if let Some(rear) = feed.tyres[1].filter(|r| r.dirt) {
        if feed.traction >= 0.95 {
            return;
        }
        place(t, rear.at);
        let side = if feed.bars >= 0.0 { -1.0 } else { 1.0 };
        let mut v = feed.right * side;
        v.y = feed.spin * 3.0;
        v *= feed.throw;
        t.velocity = Vec3::new(v.x.min(15.0), v.y.min(18.0), v.z.min(15.0));
        return;
    }
    if let Some(front) = feed.tyres[0].filter(|f| f.dirt) {
        place(t, front.at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(at: Vec3, traction: f32) -> Feed {
        let tyre = Tyre {
            at,
            dust: true,
            dirt: true,
        };
        Feed {
            tyres: [Some(tyre), Some(tyre)],
            traction,
            spin: 1.0 - traction,
            throw: 20.0,
            right: Vec3::X,
            running: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_spinning_tyre_throws_chunks_that_fall() {
        let mut p = Particles::new();
        for k in 0..10 {
            p.bike(0, &feed(Vec3::new(0.0, 0.0, k as f32), 0.5), 0.02);
        }
        let chunks: Vec<_> = p.list.iter().filter(|q| q.flags & FALLS != 0).collect();
        assert!(!chunks.is_empty());
        // Thrown out to the left (bars straight) and up.
        assert!(chunks.iter().all(|q| q.velocity.x < 0.0));
        for _ in 0..30 {
            p.update(0.02);
        }
        assert!(p.list.iter().all(|q| q.flags & FALLS == 0));
    }

    #[test]
    fn a_gripping_tyre_only_dusts() {
        let mut p = Particles::new();
        for k in 0..10 {
            p.bike(0, &feed(Vec3::new(0.0, 0.0, k as f32), 1.0), 0.02);
        }
        assert!(!p.list.is_empty());
        assert!(p.list.iter().all(|q| q.flags == GROWS | FADES | TRAIL));
        // 50 ft/s: puffs 0.67 ft across, 0.4 ft apart.
        let q = p.list[0];
        assert!((q.size - 0.667).abs() < 0.01, "{}", q.size);
    }

    #[test]
    fn no_dust_off_dusty_ground() {
        let mut p = Particles::new();
        for k in 0..10 {
            let mut f = feed(Vec3::new(0.0, 0.0, k as f32), 0.5);
            for t in f.tyres.iter_mut().flatten() {
                t.dust = false;
                t.dirt = false;
            }
            p.bike(0, &f, 0.02);
        }
        assert!(p.list.is_empty());
    }
}
