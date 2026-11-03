//! The podium after a race (bikerace.cpp: 0x45e490 sets it up when the
//! race ends, 0x460190 runs it).
//!
//! With the Podiums option on, a rider in the top three gets the podium
//! with the first three riders on it, each playing a `Winner.mcf` motion
//! for their place in their own outfit, while the camera drifts past for
//! 7 s to the crowd's loop. Esc, Enter or Space ends it early.
//!
//! * Where: the scene's `PodiumPosition` and `PodiumDirection` when it has
//!   them (the Moto-Tag arenas), else a supercross track's start gate,
//!   else the middle of the terrain facing +z; on the ground.
//! * Nationals, supercross and Moto-Tag stand `PODIUM.slt` 1.5 ft up and
//!   play `Podium5_0N` (always for a lone rider, else half the time) or
//!   `Podium3_0N`, the motion placing each rider; the other events put up
//!   `TruckPodium.slt` (0x4e2d70) and play `Podium4_0N` with the riders
//!   set out on its steps.

use glam::{Mat4, Vec3};

use crate::objects;

/// The stadium podium and the outdoor one.
pub const MODELS: [&str; 2] = ["PODIUM.slt", "TruckPodium.slt"];
pub const WINNER: &str = "Winner.mcf";
/// How long it runs, s (0x55582c).
pub const LENGTH: f32 = 7.0;

/// How high a stadium podium stands, ft.
const RAISED: f32 = 1.5;
/// A stadium podium's riders all stand at its root; its motions place them.
const STAGE: Vec3 = Vec3::new(0.0, 0.383, 0.0);
/// The other podium's steps, 1st to 3rd, and the way each rider faces:
/// -z, then (sin a, 0, cos a) for 170° and -150°.
const STEPS: [Vec3; 3] = [
    Vec3::new(0.0, 3.4, -6.0),
    Vec3::new(-11.0, 3.4, -7.0),
    Vec3::new(10.0, 3.4, -9.0),
];
const LOOKS: [f32; 2] = [170.0, -150.0];

/// The podium of one race.
#[derive(Debug, Clone, Copy)]
pub struct Podium {
    at: Vec3,
    dir: Vec3,
    /// A Nationals, supercross or Moto-Tag podium.
    stage: bool,
    /// `rand() / RAND_MAX` when it went up, for the stage's motions.
    roll: f32,
}

impl Podium {
    /// Where it stands: `scene` the scene's own podium, `gate` a supercross
    /// track's start gate, `centre` the terrain's middle; `ground` the
    /// height at x, z.
    pub fn new(
        scene: Option<(Vec3, Vec3)>,
        gate: Option<(Vec3, Vec3)>,
        centre: Vec3,
        ground: impl Fn(f32, f32) -> f32,
        stage: bool,
        roll: f32,
    ) -> Podium {
        let (mut at, dir) = scene.or(gate).unwrap_or((centre, Vec3::Z));
        let dir = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::Z);
        at.y = ground(at.x, at.z) + if stage { RAISED } else { 0.0 };
        Podium {
            at,
            dir,
            stage,
            roll,
        }
    }

    /// Turns `v` from the podium's frame to the world's (by the heading
    /// atan2(dir.x, dir.z)).
    fn turn(&self, v: Vec3) -> Vec3 {
        let right = Vec3::Y.cross(self.dir);
        right * v.x + Vec3::Y * v.y + self.dir * v.z
    }

    /// Which of [`MODELS`] it is.
    pub fn model(&self) -> usize {
        usize::from(!self.stage)
    }

    /// The podium model's place.
    pub fn place(&self) -> Mat4 {
        objects::orientation(self.dir.into(), [0.0, 1.0, 0.0], Vec3::ONE, self.at.into())
    }

    /// The rider in `place` (from 0) of `count` on the podium: where they
    /// stand and their motion.
    pub fn rider(&self, place: usize, count: usize) -> (Mat4, String) {
        let n = place + 1;
        let (offset, look, motion) = if self.stage {
            let kind = if self.roll >= 0.5 || count == 1 { 5 } else { 3 };
            (STAGE, -Vec3::Z, format!("Podium{kind}_{n:02}"))
        } else {
            let look = match place.checked_sub(1) {
                Some(k) => {
                    let a = LOOKS[k].to_radians();
                    Vec3::new(a.sin(), 0.0, a.cos())
                }
                None => -Vec3::Z,
            };
            (STEPS[place], look, format!("Podium4_{n:02}"))
        };
        let at = self.at + self.turn(offset);
        let place = objects::orientation(
            self.turn(look).into(),
            [0.0, 1.0, 0.0],
            Vec3::ONE,
            at.into(),
        );
        (place, motion)
    }

    /// The camera `t` s in: its eye and what it looks at.
    pub fn camera(&self, t: f32) -> (Vec3, Vec3) {
        let (from, to, target) = if self.stage {
            (
                Vec3::new(-10.0, 15.0, -20.0),
                Vec3::new(5.0, 9.0, -10.0),
                Vec3::new(0.0, 6.0, 0.0),
            )
        } else {
            (
                Vec3::new(-25.0, 7.0, -15.0),
                Vec3::new(20.0, 13.0, -28.0),
                Vec3::new(0.0, 6.0, -6.0),
            )
        };
        let k = (t / LENGTH).clamp(0.0, 1.0);
        (self.at + self.turn(from.lerp(to, k)), self.at + target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winner_on_the_top_step() {
        let p = Podium::new(
            None,
            None,
            Vec3::new(100.0, 0.0, 200.0),
            |_, _| 10.0,
            false,
            0.0,
        );
        let (place, motion) = p.rider(0, 3);
        assert_eq!(motion, "Podium4_01");
        assert!((place.w_axis.truncate() - Vec3::new(100.0, 13.4, 194.0)).length() < 1e-4);
        let p = Podium::new(
            Some((Vec3::ZERO, Vec3::X)),
            None,
            Vec3::ZERO,
            |_, _| 0.0,
            true,
            0.2,
        );
        assert_eq!(p.rider(1, 3).1, "Podium3_02");
        assert_eq!(p.rider(0, 1).1, "Podium5_01");
        // The stage camera ends 10 ft in front of the podium (dir +x).
        let (eye, _) = p.camera(LENGTH);
        assert!((eye - Vec3::new(-10.0, 10.5, -5.0)).length() < 1e-4);
    }
}
