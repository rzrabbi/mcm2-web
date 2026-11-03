//! Dust clouds behind the bikes for the enhanced drawing
//! ([`crate::graphics`]), none of them the game's: soft, lumpy puffs the
//! colour of the ground, thrown up behind each bike's rear tyre as it runs
//! on ground where the game raises dust. A spinning tyre or a bike pulling
//! hard throws up more, and a landing throws out a ring. They swell, spread
//! low, drift with the wind and thin away (`dust.wgsl`). They stand in for
//! the game's own dust trail; its dirt chunks still fly. On wet ground the
//! tyres throw up spray instead: drops of muddy water that fall back.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// Puffs in the air at most.
const MOST: usize = 2400;
/// The wind the dust drifts on, ft/s, drawing coordinates.
const WIND: Vec3 = Vec3::new(2.0, 0.0, 1.2);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    /// Where, drawing coordinates, and how big (half its side), ft.
    place: [f32; 4],
    /// How thick, its seed, how far it has churned and its size again; a
    /// drop's thickness and velocity.
    look: [f32; 4],
    /// Where it rose from (x, z), for the ground's colour, and 1 for a
    /// drop of spray.
    home: [f32; 4],
}

/// How a bike's tyres kick up dust this step.
#[derive(Clone, Copy, Debug, Default)]
pub struct Kick {
    /// The ground under a tyre raises dust (the surface's GenerateDust).
    pub dusty: bool,
    /// How much the rear tyre spins, 0 to 1 (1 less its traction).
    pub spin: f32,
}

/// What is kept of each bike from the step before.
#[derive(Clone, Copy, Default)]
struct Track {
    /// The rear tyre, drawing coordinates.
    last: Option<Vec3>,
    /// Its speed, ft/s.
    speed: f32,
    /// Seconds off the ground.
    air: f32,
    /// Puffs owed, a fraction carried over.
    owed: f32,
}

struct Puff {
    at: Vec3,
    home: Vec3,
    velocity: Vec3,
    age: f32,
    life: f32,
    seed: f32,
    thick: f32,
    /// A drop of spray, not a puff of dust: small, and falls.
    drop: bool,
    /// The size it is thrown at and how much it swells, ft.
    size: f32,
    swell: f32,
}

pub struct DustRenderer {
    pipeline: wgpu::RenderPipeline,
    instances: wgpu::Buffer,
    count: u32,
    puffs: Vec<Puff>,
    bikes: Vec<Track>,
    random: u32,
}

impl DustRenderer {
    /// With the terrain's own group's layout and the enhanced drawing's
    /// shared one.
    pub fn new(
        device: &wgpu::Device,
        terrain_layout: &wgpu::BindGroupLayout,
        shared_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        samples: u32,
        depth_format: wgpu::TextureFormat,
    ) -> DustRenderer {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("dust"),
            bind_group_layouts: &[Some(terrain_layout), Some(shared_layout)],
            immediate_size: 0,
        });
        let shader = crate::enhance::shader(device, "dust", include_str!("dust.wgsl"));
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("dust"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
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
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dust"),
            size: (MOST * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        DustRenderer {
            pipeline,
            instances,
            count: 0,
            puffs: Vec::new(),
            bikes: Vec::new(),
            random: 0x9e37_79b9,
        }
    }

    fn random(&mut self) -> f32 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        (self.random >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Each bike's tyres on the ground this step (game coordinates) and
    /// how they kick up dust, the step, s, and whether the ground is wet:
    /// the rear ones throw up dust (or spray) as they run, and what is in
    /// the air moves on.
    pub fn step(&mut self, feed: &[([Option<Vec3>; 2], Kick)], dt: f32, wet: bool) {
        if dt <= 0.0 {
            return;
        }
        self.bikes.resize(feed.len(), Track::default());
        for (i, (tyres, kick)) in feed.iter().enumerate() {
            let rear = tyres[1].map(crate::renderer::from_game);
            let mut track = self.bikes[i];
            match (track.last, rear) {
                (Some(a), Some(b)) => {
                    let velocity = (b - a) / dt;
                    let speed = velocity.length();
                    if speed < 200.0 {
                        if wet {
                            self.spray(&mut track, b, velocity, dt);
                        } else if kick.dusty {
                            self.dust(&mut track, b, velocity, *kick, dt);
                        }
                    }
                    track.speed = speed;
                    track.air = 0.0;
                }
                (None, Some(b)) => {
                    // Down again after a jump: a ring of dust thrown out
                    // round the tyre, by how long it flew and how fast.
                    if !wet && kick.dusty && track.air > 0.3 && track.speed > 12.0 {
                        let n = ((track.air * 10.0).min(12.0) + track.speed * 0.15) as usize;
                        for k in 0..n {
                            let (r1, r2, r3, seed) = (self.random(), self.random(), self.random(), self.random());
                            let angle = (k as f32 + r1) / n as f32 * std::f32::consts::TAU;
                            let out = Vec3::new(angle.cos(), 0.0, angle.sin());
                            self.add(Puff {
                                at: b + out * 0.8 + Vec3::Y * 0.2,
                                home: b,
                                velocity: out * (6.0 + 8.0 * r2) + Vec3::Y * (1.0 + 2.0 * r3),
                                age: 0.0,
                                life: 1.6 + 1.4 * r3,
                                seed,
                                thick: 0.32 + 0.2 * r2,
                                drop: false,
                                size: 0.5 + 0.4 * r1,
                                swell: 1.4 + 1.2 * r2,
                            });
                        }
                    }
                    track.air = 0.0;
                }
                (_, None) => track.air += dt,
            }
            track.last = rear;
            self.bikes[i] = track;
        }
        for p in &mut self.puffs {
            p.age += dt;
            if p.drop {
                // A drop falls, a little slowed by the air.
                p.velocity = p.velocity * (-dt * 0.6).exp() - Vec3::Y * 32.0 * dt;
                p.at += p.velocity * dt;
                continue;
            }
            // Slowed hard by the air toward the wind, so it hangs low
            // rather than rising as smoke does.
            let drag = (-dt * 2.2).exp();
            p.velocity = WIND + (p.velocity - WIND) * drag + Vec3::Y * 0.3 * dt;
            p.at += p.velocity * dt;
        }
        self.puffs.retain(|p| p.age < p.life);
    }

    fn add(&mut self, puff: Puff) {
        if self.puffs.len() >= MOST {
            self.puffs.remove(0);
        }
        self.puffs.push(puff);
    }

    /// A rear tyre running at `b` on dusty ground: a little dust always,
    /// more and thicker as it spins or the bike pulls away.
    fn dust(&mut self, track: &mut Track, b: Vec3, velocity: Vec3, kick: Kick, dt: f32) {
        let speed = velocity.length();
        if speed < 4.0 {
            return;
        }
        let pull = ((speed - track.speed) / dt / 20.0).clamp(0.0, 1.5);
        let spin = (kick.spin * 4.0).clamp(0.0, 1.5);
        let hard = pull.max(spin);
        track.owed += (3.0 + speed * 0.18) * (1.0 + 1.5 * hard) * dt;
        let back = -velocity / speed;
        let side = back.cross(Vec3::Y).normalize_or(Vec3::X);
        while track.owed >= 1.0 {
            track.owed -= 1.0;
            let (r1, r2, r3, r4, seed) = (self.random(), self.random(), self.random(), self.random(), self.random());
            // Kicked up and back off the tyre, fanning out to the sides; a
            // spinning tyre throws it further back.
            let thrown = velocity * (0.05 + 0.1 * r1) + back * speed * (0.06 + 0.12 * hard * r3) + side * (r2 - 0.5) * (6.0 + speed * 0.12) + Vec3::Y * (1.5 + 3.0 * r3 + 2.0 * hard);
            self.add(Puff {
                at: b + back * 0.8 + side * (r4 - 0.5) * 0.8 + Vec3::Y * 0.3,
                home: b,
                velocity: thrown,
                age: 0.0,
                life: 2.2 + 2.0 * r4,
                seed,
                thick: (0.5 + speed / 60.0 * 0.25 + hard * 0.2).min(0.9),
                drop: false,
                size: 0.4 + 0.3 * r1,
                swell: (1.2 + speed * 0.02 + 0.8 * hard) * (0.7 + 0.6 * r2),
            });
        }
    }

    /// A rear tyre running at `b` on wet ground: a rooster tail of muddy
    /// drops, and now and then a wisp of mist.
    fn spray(&mut self, track: &mut Track, b: Vec3, velocity: Vec3, dt: f32) {
        let speed = velocity.length();
        if speed < 4.0 {
            return;
        }
        track.owed += (40.0 + speed * 3.0) * dt;
        let back = -velocity / speed;
        let side = back.cross(Vec3::Y).normalize_or(Vec3::X);
        while track.owed >= 1.0 {
            track.owed -= 1.0;
            let (r1, r2, r3, r4, seed) = (self.random(), self.random(), self.random(), self.random(), self.random());
            // Flung up off the back of the tyre in a rooster tail, fanning
            // out to the sides.
            let thrown = velocity * (0.2 + 0.2 * r1) + back * speed * (0.1 + 0.15 * r3) + side * (r2 - 0.5) * (4.0 + speed * 0.2) + Vec3::Y * (5.0 + 9.0 * r4 + speed * 0.1);
            self.add(Puff {
                at: b + back * 0.6 + Vec3::Y * 0.4,
                home: b,
                velocity: thrown,
                age: 0.0,
                life: 0.5 + 0.5 * r1,
                seed,
                thick: 0.6 + 0.3 * r2,
                drop: true,
                size: 0.0,
                swell: 0.0,
            });
            if seed < 0.06 {
                self.add(Puff {
                    at: b + back * 0.8 + Vec3::Y * 0.3,
                    home: b,
                    velocity: velocity * 0.1 + back * speed * 0.05 + Vec3::Y * 2.0,
                    age: 0.0,
                    life: 1.0 + r3,
                    seed: r4,
                    thick: 0.22,
                    drop: false,
                    size: 1.0,
                    swell: 4.5,
                });
            }
        }
    }

    /// Puts the puffs in the buffer, furthest first, for the eye at `eye`.
    pub fn prepare(&mut self, queue: &wgpu::Queue, eye: Vec3) {
        let mut order: Vec<(f32, &Puff)> = self.puffs.iter().map(|p| (p.at.distance_squared(eye), p)).collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0));
        let instances: Vec<Instance> = order
            .iter()
            .map(|(_, p)| {
                let t = p.age / p.life;
                if p.drop {
                    let size = 0.04 + 0.07 * p.seed;
                    return Instance {
                        place: [p.at.x, p.at.y, p.at.z, size],
                        look: [p.thick * (1.0 - t * t), p.velocity.x, p.velocity.y, p.velocity.z],
                        home: [p.home.x, p.home.z, 1.0, 0.0],
                    };
                }
                // Swells fast then slowly; fades in quickly and out slowly,
                // thinning as it spreads.
                let size = p.size + p.swell * (1.0 - (-p.age * 1.3).exp()) + p.age * 0.25;
                let thick = p.thick * (t / 0.05).min(1.0) * (1.0 - t).powf(1.6);
                // Raised by about half its size, so it billows up off the
                // ground rather than being cut off flat by it.
                Instance {
                    place: [p.at.x, p.at.y + size * 0.45, p.at.z, size],
                    look: [thick, p.seed, p.age, size],
                    home: [p.home.x, p.home.z, 0.0, 0.0],
                }
            })
            .collect();
        self.count = instances.len() as u32;
        if !instances.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
        }
    }

    /// Draws the dust, with the terrain's own group and the shared one.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, terrain: &wgpu::BindGroup, shared: &wgpu::BindGroup) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, terrain, &[]);
        pass.set_bind_group(1, shared, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..6, 0..self.count);
    }
}
