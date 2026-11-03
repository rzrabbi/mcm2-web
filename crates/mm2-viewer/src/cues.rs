//! Visual Cues (F4): the green arrow at the bottom left of the screen that
//! points where the player should head (`VisualCue`, 0x48bd00), and in a
//! lap race the five runway lights that run along the course where the
//! player left it (`RunwayLights`, 0x48b560).
//!
//! * The arrow is an `ArcadeObject` (0x401310) held in front of the camera:
//!   3 ft ahead, a tenth of the way across the screen and 85% of the way
//!   down, sized so that its largest side is 45 pixels of the 640×480
//!   screen, and turned to look from the camera at its target (0x48c0a0).
//!   The target is the gate the player rides for in a Baja or Enduro race;
//!   in a lap race the confirmed place on the course, shown only while the
//!   player is off the course; in Moto-Tag IT, or the ball. It hides once
//!   the player has finished.
//! * The lights (0x48b700) stand 6, 3, 0, −3 and −6 ft along the course from
//!   the confirmed place, each 4 ft above the ground and upright on it, and
//!   only while the player is off the course. One at a time is lit, from the
//!   back to the front, a quarter of a second each, round a 1.375 s cycle.

use glam::{Mat4, Vec3};

/// The arrow and the light, in `RES/MODELS.RES` (`viscue.slt`,
/// `pointer.slt`; the game loads one light model five times).
pub const MODELS: [&str; 6] = [
    "VisCue.slt",
    "Pointer.slt",
    "Pointer.slt",
    "Pointer.slt",
    "Pointer.slt",
    "Pointer.slt",
];

/// The arrow's distance in front of the camera, its place across and down
/// the screen, and its size in pixels (0x4e4e80's 3.0, 0.1, 0.85, 45).
const DEPTH: f32 = 3.0;
const ACROSS: f32 = 0.1;
const DOWN: f32 = 0.85;
const PIXELS: f32 = 45.0;

/// Where the lights stand along the course, ft (0x48b9e4: 6.0, 3.0), and
/// how high over the ground (4.0).
const LIGHTS: [f32; 5] = [6.0, 3.0, 0.0, -3.0, -6.0];
const LIGHT_HEIGHT: f32 = 4.0;
/// The lights' cycle (1.375 s at 0x5593d0) and when each lights:
/// 1.125, 0.875, 0.625, 0.375 and 0.125 s in (0x5593c0..0x5593cc).
const CYCLE: f32 = 1.375;
const LIT_FROM: [f32; 5] = [1.125, 0.875, 0.625, 0.375, 0.125];

/// The arrow's and the lights' objects, and the arrow model's largest side.
#[derive(Debug, Clone, Copy)]
pub struct Cues {
    pub arrow: usize,
    pub lights: [usize; 5],
    /// Twice the model's largest half extent (the object's +0x3c).
    pub size: f32,
}

/// What the camera sees with: where it is, where it looks, its view across
/// a 4:3 screen in degrees and the window's width over its height.
#[derive(Debug, Clone, Copy)]
pub struct Eye {
    pub at: Vec3,
    pub target: Vec3,
    pub fov: f32,
    pub aspect: f32,
}

impl Cues {
    /// The arrow, looking from the eye to `aim` (game coordinates).
    pub fn arrow(&self, eye: Eye, aim: Vec3) -> Mat4 {
        let forward = (eye.target - eye.at).normalize_or(Vec3::Z);
        let right = Vec3::Y.cross(forward).normalize_or(Vec3::X);
        let up = forward.cross(right);
        // The game's view is 4:3; a wider window keeps its height, so the
        // arrow keeps its place across the window as the race panel does.
        let half = (eye.fov.to_radians() / 2.0).tan() * 0.75;
        let x = 2.0 * (ACROSS - 0.5) * half * eye.aspect * DEPTH;
        let y = 2.0 * (0.5 - DOWN) * half * DEPTH;
        let at = eye.at + right * x + up * y + forward * DEPTH;
        // 45 pixels of the 480 high screen, at its depth.
        let focal = 240.0 / half;
        let scale = PIXELS * DEPTH / (self.size.max(1e-3) * focal);
        let look = aim - eye.at;
        crate::objects::orientation(look.into(), [0.0, 1.0, 0.0], Vec3::splat(scale), at.into())
    }

    /// The five lights round `at` along `way`, at race time `time`: each
    /// one's place, and whether it is the lit one.
    pub fn lights(
        &self,
        at: Vec3,
        way: Vec3,
        time: f32,
        ground: impl Fn(f32, f32) -> (f32, Vec3),
    ) -> [(Mat4, bool); 5] {
        let t = time.rem_euclid(CYCLE);
        let lit = LIT_FROM.iter().position(|&from| t > from);
        std::array::from_fn(|k| {
            let mut p = at + way * LIGHTS[k];
            let (y, normal) = ground(p.x, p.z);
            p.y = y + LIGHT_HEIGHT;
            // Upright on the ground, facing along the course (0x500bb0
            // keeping the up).
            let side = normal.cross(way);
            let look = side.cross(normal);
            let place = crate::objects::orientation(look.into(), normal.into(), Vec3::ONE, p.into());
            (place, lit == Some(k))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arrow_sits_low_on_the_left() {
        let cues = Cues { arrow: 0, lights: [0; 5], size: 2.0 };
        let eye = Eye {
            at: Vec3::ZERO,
            target: Vec3::Z,
            fov: 85.0,
            aspect: 4.0 / 3.0,
        };
        let m = cues.arrow(eye, Vec3::new(0.0, 0.0, 100.0));
        let p = m.w_axis.truncate();
        assert!((p.z - DEPTH).abs() < 1e-4);
        // Left is −x in the game's left-handed frame, and below the middle.
        assert!(p.x < 0.0 && p.y < 0.0);
        // A tenth across: x / (z tan(hfov / 2)) = 2 × 0.1 − 1.
        let across = p.x / (DEPTH * (42.5f32).to_radians().tan());
        assert!((across + 0.8).abs() < 1e-3, "{across}");
    }

    #[test]
    fn the_lights_run_forward() {
        let cues = Cues { arrow: 0, lights: [0; 5], size: 1.0 };
        let flat = |_: f32, _: f32| (10.0, Vec3::Y);
        let lit = |t: f32| {
            cues.lights(Vec3::ZERO, Vec3::Z, t, flat).iter().position(|&(_, on)| on)
        };
        assert_eq!(lit(0.1), None);
        assert_eq!(lit(0.2), Some(4));
        assert_eq!(lit(0.7), Some(2));
        assert_eq!(lit(1.2), Some(0));
        let l = cues.lights(Vec3::ZERO, Vec3::Z, 0.0, flat);
        assert_eq!(l[0].0.w_axis.truncate(), Vec3::new(0.0, 14.0, 6.0));
        assert_eq!(l[4].0.w_axis.truncate(), Vec3::new(0.0, 14.0, -6.0));
    }
}
