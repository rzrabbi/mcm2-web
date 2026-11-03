//! The Bike & Rider page's turning bike and rider (`0x44fe50`): the menu
//! models (`uihonda.mcf` and the like, `UIRider.mcf`) in the player's
//! paint, outfit and number, drawn into the window at 91,108 to 543,328
//! of the 640×480 screen. The camera stands 15 ft out, 1 ft up, looking at
//! 3 ft above the models with a 50° view.

use crate::objects::{ObjectMesh, ObjectRenderer, PLATE};
use crate::renderer::{from_game, Globals, DEPTH_FORMAT};
use crate::roster::Ride;
use bytemuck::Zeroable;
use glam::{Mat4, Vec3};
use mm2_formats::{rs2, scn};
use wgpu::util::DeviceExt;

/// The window on the 640×480 screen.
pub const VIEWPORT: [f32; 4] = [91.0, 108.0, 452.0, 220.0];
const EYE: Vec3 = Vec3::new(0.0, 1.0, 15.0);
const TARGET: Vec3 = Vec3::new(0.0, 3.0, 0.0);
const FOV: f32 = 50.0;
/// How fast it turns, radians a second (our pick).
const TURN: f32 = 0.6;

pub struct Preview {
    ride: Ride,
    /// The window on the 640×480 screen.
    pub window: [f32; 4],
    /// Whether the screen it is on takes the wide layout in a wide window
    /// (`Sprites::spread`), which stretches the window with the panel.
    pub spread: bool,
    objects: ObjectRenderer,
    globals: wgpu::Buffer,
    depth: Option<(wgpu::TextureView, u32, u32)>,
    angle: f32,
    /// Waiting (WaitR), or playing one of the idles since a time.
    idle: Option<f32>,
    seed: u32,
}

/// The rider's idles on the menu pages (0x450910, 0x4dbac0, 0x4fd5f0):
/// waiting, each frame has a 1 in 200 chance of one of these, picked
/// evenly; when it has played, the bike and rider go back to waiting.
const IDLES: [&str; 3] = ["LookLeftR", "LookRightR", "StretchR"];

impl Preview {
    /// Builds the models for `ride` from `RES/BIKE.RES`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bike_res: &rs2::Archive,
        ride: &Ride,
        format: wgpu::TextureFormat,
    ) -> Option<Preview> {
        let place = |model: &str, motion: &str| scn::Animation {
            model: model.to_string(),
            offset: [0.0; 3],
            look: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
            scale: [1.0; 3],
            motion: Some(motion.to_string()),
            lighting: Default::default(),
        };
        let animations = [
            place(&ride.bike.ui_model, "WaitB"),
            place("UIRider.mcf", "WaitR"),
        ];
        let skins = [
            Some(format!("{PLATE}{}:{}", ride.plate, ride.bike.texture)),
            Some(ride.outfit.clone()),
        ];
        let archives = std::slice::from_ref(bike_res);
        let textures = [("BIKE.RES", bike_res.clone())];
        let mesh = ObjectMesh::build(
            &[],
            &[],
            &[],
            &animations,
            &skins,
            archives,
            &textures,
            crate::hires::pack(),
        );
        let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("preview globals"),
            contents: bytemuck::bytes_of(&Globals::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let objects = ObjectRenderer::new(device, queue, &mesh, &globals, format, DEPTH_FORMAT, 1, None)?;
        Some(Preview {
            window: VIEWPORT,
            spread: false,
            ride: ride.clone(),
            objects,
            globals,
            depth: None,
            angle: 0.0,
            idle: None,
            seed: 0x1234_5678,
        })
    }

    fn rand(&mut self) -> u32 {
        // The C library's rand(), as the game calls it.
        self.seed = self.seed.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.seed >> 16) & 0x7fff
    }

    fn idle_step(&mut self, time: f32) {
        match self.idle {
            None => {
                if self.rand() % 200 == 1 {
                    let pick = IDLES[(self.rand() % 3) as usize];
                    self.objects.set_motion(1, pick, time);
                    self.idle = Some(time);
                }
            }
            Some(_) if self.objects.motion_done(1, time) => {
                self.objects.set_motion(0, "WaitB", time);
                self.objects.set_motion(1, "WaitR", time);
                self.idle = None;
            }
            Some(_) => {}
        }
    }

    pub fn ride(&self) -> &Ride {
        &self.ride
    }

    /// Draws over `target` (`size` pixels, the menus letterboxed in it).
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        size: (u32, u32),
        time: f32,
        dt: f32,
        turning: bool,
    ) -> wgpu::CommandBuffer {
        let (width, height) = size;
        if self
            .depth
            .as_ref()
            .is_none_or(|d| (d.1, d.2) != (width, height))
        {
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("preview depth"),
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
        if turning {
            self.angle += TURN * dt;
        }
        self.idle_step(time);
        let turn = Mat4::from_rotation_y(self.angle);
        self.objects.set_place(0, turn);
        self.objects.set_place(1, turn);

        // The window in pixels, as the sprites letterbox the screen.
        let scale = (width as f32 / 640.0).min(height as f32 / 480.0);
        let (mut ox, mut oy) = (
            (width as f32 - 640.0 * scale) / 2.0,
            (height as f32 - 480.0 * scale) / 2.0,
        );
        let [mut x, mut y, mut w, mut h] = self.window.map(|v| v * scale);
        // The shape the window had before it was stretched, which sets how
        // big the bike looks.
        let shaped = w / h;
        if let (true, Some(lay)) = (self.spread, crate::sprites::Sprites::layout((width, height))) {
            let along = lay.length / [640.0, 480.0][lay.axis];
            let w0 = self.window;
            // Along the spread axis the window stretches; across it, it has
            // the layout's scale.
            (ox, oy) = (0.0, 0.0);
            if lay.axis == 0 {
                [x, y, w, h] = [lay.offset + w0[0] * along, w0[1] * lay.scale, w0[2] * along, w0[3] * lay.scale];
            } else {
                [x, y, w, h] = [w0[0] * lay.scale, lay.offset + w0[1] * along, w0[2] * lay.scale, w0[3] * along];
            }
        }
        let aspect = w / h;
        // The view's 50° spans the window's width, as it was shaped.
        let fovy = 2.0 * ((FOV.to_radians() / 2.0).tan() / shaped).atan();
        let proj = Mat4::perspective_rh(fovy, aspect, 0.5, 200.0);
        let (eye, target_at) = (from_game(EYE), from_game(TARGET));
        let view = Mat4::look_at_rh(eye, target_at, Vec3::Y);
        let globals = Globals {
            view_proj: (proj * view).to_cols_array_2d(),
            camera: eye.extend(1.0).into(),
            grid: [0.0; 4],
            // No fog in the menus' view.
            fog: [0.0; 4],
            fog_range: [1e6, 2e6, 0.0, 0.0],
            ambient: crate::renderer::SceneLights::default().globals().0,
            sun: crate::renderer::SceneLights::default().globals().1,
            to_sun: crate::renderer::SceneLights::default().globals().2,
            shadow_view: Mat4::IDENTITY.to_cols_array_2d(),
            shadow_on: [0.0; 4],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        self.objects.update(queue, time);

        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth.as_ref().unwrap().0,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
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
            pass.set_viewport(ox + x, oy + y, w, h, 0.0, 1.0);
            pass.set_scissor_rect(
                (ox + x) as u32,
                (oy + y) as u32,
                (w as u32).min(width),
                (h as u32).min(height),
            );
            self.objects.draw(&mut pass);
        }
        encoder.finish()
    }
}
