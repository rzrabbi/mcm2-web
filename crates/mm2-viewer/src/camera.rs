//! A free-flying camera.

use glam::{Mat4, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub position: Vec3,
    /// Radians around +y; 0 looks along +x.
    pub yaw: f32,
    /// Radians up from the horizon.
    pub pitch: f32,
    /// The field of view across a 4:3 screen, degrees (the game's own
    /// measure; wider windows keep the height).
    pub fov: f32,
}

/// The game's usual field of view, degrees.
pub const FOV: f32 = 85.0;

impl Camera {
    /// A camera at `eye` looking at `target`.
    pub fn looking_at(eye: Vec3, target: Vec3) -> Self {
        let d = (target - eye).normalize_or(Vec3::X);
        Camera {
            position: eye,
            yaw: d.z.atan2(d.x),
            pitch: d.y.clamp(-1.0, 1.0).asin(),
            fov: FOV,
        }
    }

    /// The field of view up the screen, radians.
    pub fn fovy(&self) -> f32 {
        2.0 * ((self.fov.to_radians() / 2.0).tan() * 0.75).atan()
    }

    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        )
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Y).normalize()
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_to_rh(self.position, self.forward(), Vec3::Y)
    }

    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw += dx;
        self.pitch = (self.pitch - dy).clamp(-1.55, 1.55);
    }
}
