//! The 3D countdown numbers (`NumberObjectManager`, built in 0x48ccb0 and
//! run by 0x48cf70): five `ArcadeObject`s (0x401310), `one.slt` to
//! `five.slt`, held in front of the race camera. Only in a race (race mode
//! `0x2d70` not 0), and only while the race's own camera is the one in use.
//!
//! * Before the gate drops, when there is no 30-second man (he does the
//!   counting on his board when he is out): the number is the seconds left
//!   (race +0x15c), rounded down, plus one, so 5, 4, 3, 2, 1 over the last
//!   five of the 5.2 s.
//! * Otherwise, while the player has been off the course for over a second
//!   (bike +0x76c), they count down to the reset at 6 s: 5 from 1 s off, 1
//!   from 5 s.
//!
//! The number stands 6 ft ahead of the camera and 2.5 ft along its up
//! (0x555c2c, 0x556000), facing the way the camera looks, and is sized as
//! the arrow is, so that its largest side would be 30 pixels of the 640×480
//! screen 3 ft away (0x419bfd's 0x1e and 3.0).

use glam::{Mat4, Vec3};

/// The numbers 1 to 5, in `RES/MODELS.RES`.
pub const MODELS: [&str; 5] = ["one.slt", "two.slt", "three.slt", "four.slt", "five.slt"];

const AHEAD: f32 = 6.0;
const UP: f32 = 2.5;
const PIXELS: f32 = 30.0;
const DEPTH: f32 = 3.0;
/// Off the course this long before the numbers show (0x555748).
const OFF_FROM: f32 = 1.0;

#[derive(Debug, Clone, Copy)]
pub struct Countdown {
    /// The objects for 1 to 5.
    pub numbers: [usize; 5],
    /// Each model's largest side (the object's +0x3c).
    pub sizes: [f32; 5],
}

/// Which number shows (0 for 1 … 4 for 5): `left`, the seconds to the gate
/// when the countdown shows (no 30-second man); else the player's seconds
/// off the course.
pub fn number(left: Option<f32>, off_course: f32) -> Option<usize> {
    let k = match left {
        Some(t) if t > 0.0 => t as i32,
        _ if off_course > OFF_FROM => 5 - off_course as i32,
        _ => return None,
    };
    (0..5).contains(&k).then_some(k as usize)
}

impl Countdown {
    /// Number `k`'s place in front of the camera `eye`.
    pub fn place(&self, k: usize, eye: crate::cues::Eye) -> Mat4 {
        let forward = (eye.target - eye.at).normalize_or(Vec3::Z);
        let right = Vec3::Y.cross(forward).normalize_or(Vec3::X);
        let up = forward.cross(right);
        let at = eye.at + forward * AHEAD + up * UP;
        // 0x401310: scale = pixels · depth / (size · the camera's focal
        // length in pixels, +0x198).
        let half = (eye.fov.to_radians() / 2.0).tan() * 0.75;
        let focal = 240.0 / half;
        let scale = PIXELS * DEPTH / (self.sizes[k].max(1e-3) * focal);
        crate::objects::orientation(forward.into(), up.into(), Vec3::splat(scale), at.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_down_before_the_gate() {
        assert_eq!(number(Some(5.1), 0.0), None);
        assert_eq!(number(Some(4.9), 0.0), Some(4));
        assert_eq!(number(Some(0.5), 0.0), Some(0));
        assert_eq!(number(Some(0.0), 0.0), None);
    }

    #[test]
    fn counts_down_off_the_course() {
        assert_eq!(number(None, 0.5), None);
        assert_eq!(number(None, 1.5), Some(4));
        assert_eq!(number(None, 5.5), Some(0));
        assert_eq!(number(None, 6.5), None);
        // The countdown before the gate wins.
        assert_eq!(number(Some(2.5), 3.0), Some(2));
    }
}
