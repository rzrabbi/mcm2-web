//! The rider thrown off in a crash, after the game's Wrecker (wrecker.cpp,
//! update 0x535d50).
//!
//! The game does not use a jointed ragdoll. The rider is one rigid body
//! that plays a crash motion from Rider.mcf while it tumbles:
//!
//! * A crash on the ground starts a **bail** (0x537890): a motion chosen by
//!   the kind of crash (Endo, Kahuna, BackOver, LeftOver, RightOver, list
//!   +0x48) plays with the rider still on the bike, and once it is half
//!   done (0x535d50, state 1) the rider is let go (0x537b00) with the
//!   speed and spin the motion gave the pelvis over the last frame
//!   (0x537bf0), and one of Fall01 to Fall04 fades in over 0.5 s.
//! * A crash in the air **tumbles** at once (0x537980): one of Fall01 to
//!   Fall04 at random (list +0x50), blended in over 0.5 s, the body thrown
//!   with the bike's speed and spin.
//! * When the body first lands it picks one of LeftHit, RightHit,
//!   FeetHitL, FeetHitR or HeadHit at random (list +0x58, kept in +0xd4)
//!   and fades it in over 0.5 s once the fall's own fade is done (the
//!   motion player's +0x34), then slides to a stop; then the rider is put
//!   back on the bike.
//!
//! The body is the rider's pelvis (the Wrecker's +0xb8, the first frame
//! under the model's root, whose world placement +0x6c the 2026-10-24
//! recordings follow). The motion poses the rest of the rider round it,
//! so a motion change or fade moves the limbs but never jumps the body
//! (0x535d50 re-roots the model at the end of each fade to keep the
//! pelvis where it was).
//!
//! The body touches with Rider.col's six part shapes (loaded in
//! 0x535830), which move with the motion playing; the viewer hands their
//! posed corners over. The deepest corner in the ground is answered by the
//! rider's collision model (0x43cd50): no bounce (+0xb8 = 0), only 5 %
//! of the impulse that would stop it (1 − +0xf4, 0.95), at most 2000, and
//! the body's speed and spin cut by 0.99 × the step's time (+0xf0). The
//! body's inverse inertia is zeroed (0x4a2950 into +0x1d0), so contacts
//! never turn it; the fall motions do the tumbling. Until the corners are
//! set the body touches as one sphere. 0x538050
//! also keeps the parts out of the bike: a box in the bike's model space
//! (centre (0, 2, 0), half size (0.5, 1.5, 3) ft, set in 0x4924e6) pushes
//! any part inside it out at 5 × its depth per second. 0x537690 only
//! throws dirt where the parts slide.

use glam::{Mat3, Mat4, Quat, Vec3};

use crate::Ground;

/// The rider's centre in the shared bike and rider model space (Rider02
/// bounds).
pub const RIDER_CENTRE: Vec3 = Vec3::new(0.0, 3.0, -0.6);
const RADIUS: f32 = 0.9;
const GRAVITY: f32 = 32.2;
const BOUNCE: f32 = 0.3;
const FRICTION: f32 = 0.8;
/// How long a bail holds the rider on the bike, s: half its motion
/// (Rider.mcf: frames at 15 a second), or this for a motion not listed.
const BAIL_TIME: f32 = 0.5;
const BAILS: [(&str, f32); 5] = [("Endo", 12.0), ("LeftOver", 10.0), ("RightOver", 10.0), ("BackOver", 13.0), ("Kahuna", 25.0)];
const BAIL_FPS: f32 = 15.0;
/// How long a fall or a landing motion fades in, s (0x4aa1e0's span).
pub const FADE: f32 = 0.5;
/// The rider is at rest below this speed, ft/s, for this long, s.
const REST_SPEED: f32 = 2.0;
const REST_TIME: f32 = 0.75;
/// The longest a wreck lasts, s.
const LONGEST: f32 = 6.0;

/// The bike's box the thrown rider is kept out of (0x538050), and how hard.
const BIKE_BOX_CENTRE: Vec3 = Vec3::new(0.0, 2.0, 0.0);
const BIKE_BOX_HALF: Vec3 = Vec3::new(0.5, 1.5, 3.0);
const BIKE_BOX_PUSH: f32 = 5.0;

/// The rider's collision model (0x43c870, set up in 0x535830): the
/// share of a contact's stopping impulse it drops (+0xf4), the most one
/// impulse gives (+0xf8), the cut to speed and spin per second of each
/// contact (+0xf0), and the body's mass (its vtable +0x74).
const IMPULSE_DROPPED: f32 = 0.95;
const IMPULSE_MOST: f32 = 2000.0;
const DAMPING: f32 = 0.99;
const MASS: f32 = 200.0;
/// How far a solid object is felt round each corner, ft [guess].
const CORNER_RADIUS: f32 = 0.05;

/// Crash motions in Rider.mcf.
pub const FALLS: [&str; 4] = ["Fall01", "Fall02", "Fall03", "Fall04"];
pub const HITS: [&str; 5] = ["LeftHit", "RightHit", "FeetHitL", "FeetHitR", "HeadHit"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Still on the bike, playing the bail motion.
    Bail,
    /// Thrown, in the air.
    Tumble,
    /// Down on the ground, sliding to a stop.
    Down,
}

#[derive(Debug, Clone)]
pub struct Wreck {
    /// The Rider.mcf motion to play now.
    pub motion: &'static str,
    pub stage: Stage,
    /// The rider body's centre (the pelvis) and its velocity.
    pub position: Vec3,
    pub velocity: Vec3,
    orientation: Mat3,
    /// Spin, rad/s, world axes.
    spin: Vec3,
    time: f32,
    still: f32,
    seed: u32,
    /// The body touched the ground or an object this step (the Wrecker's
    /// +0xb0, from its collision model's +0x58), which the race sounds
    /// read for the rider's grunts.
    pub touching: bool,
    /// Rider.col's corners where the motion has them now, in the
    /// pelvis's own space; empty for one sphere.
    points: Vec<Vec3>,
    /// The pelvis in model space as the motion has it now; the model is
    /// drawn round it.
    pelvis: Mat4,
    /// Whether the viewer has said where the motion puts the pelvis.
    posed: bool,
    /// When the bail lets the rider go, s.
    bail_time: f32,
    /// When the motion fading in now is fully in, s.
    faded: f32,
    /// The landing motion waiting for that (+0xd4).
    queued: Option<&'static str>,
    /// The body at the last pose, while the bail plays: when, where and
    /// turned how, for the speed and spin it lets go with.
    last: Option<(f32, Vec3, Mat3)>,
}

impl Wreck {
    /// A wreck starting from a bike placed at `bike` (model to world)
    /// moving at `velocity` and spinning at `spin` (world).
    pub fn new(
        bike: Mat4,
        velocity: Vec3,
        spin: Vec3,
        motion: &'static str,
        bail: bool,
        seed: u32,
    ) -> Self {
        Wreck {
            motion,
            stage: if bail { Stage::Bail } else { Stage::Tumble },
            position: bike.transform_point3(RIDER_CENTRE),
            velocity,
            orientation: Mat3::from_mat4(bike),
            spin,
            time: 0.0,
            still: 0.0,
            seed,
            touching: false,
            points: Vec::new(),
            pelvis: Mat4::from_translation(RIDER_CENTRE),
            posed: false,
            bail_time: BAILS
                .iter()
                .find(|b| b.0 == motion)
                .map_or(BAIL_TIME, |b| b.1 / BAIL_FPS / 2.0),
            faded: if bail { 0.0 } else { FADE },
            queued: None,
            last: None,
        }
    }

    /// The body: the pelvis on the track.
    pub fn body(&self) -> Mat4 {
        let mut m = Mat4::from_mat3(self.orientation);
        m.w_axis = self.position.extend(1.0);
        m
    }

    /// Where to draw the rider: the model origin and axes it shares with
    /// the bike.
    pub fn place(&self) -> Mat4 {
        self.body() * self.pelvis.inverse()
    }

    /// Where the motion playing puts the pelvis (`pelvis`, model space)
    /// and Rider.col's corners (`points`, the pelvis's space), as the
    /// viewer poses them each frame. The first pose keeps the rider where
    /// they were drawn.
    pub fn set_pose(&mut self, pelvis: Mat4, points: Vec<Vec3>) {
        if !self.posed {
            let at = self.place() * pelvis;
            self.position = at.w_axis.truncate();
            self.orientation = orthonormal(Mat3::from_mat4(at));
            self.posed = true;
        } else if self.stage == Stage::Bail {
            self.last = Some((self.time, self.position, self.orientation));
        }
        self.pelvis = pelvis;
        self.points = points;
    }

    fn pick<const N: usize>(&mut self, from: [&'static str; N]) -> &'static str {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        from[self.seed as usize % N]
    }

    /// One step of `h` seconds. `bike` is where the bike is now, for a
    /// bail. Returns whether the wreck is over.
    pub fn step(&mut self, ground: &dyn Ground, h: f32, bike: Mat4, bike_velocity: Vec3) -> bool {
        self.time += h;
        if self.stage == Stage::Bail {
            let at = bike * self.pelvis;
            self.velocity = bike_velocity;
            self.position = at.w_axis.truncate();
            self.orientation = orthonormal(Mat3::from_mat4(at));
            if self.time >= self.bail_time {
                // Let go with the pelvis's own speed and spin since the
                // last pose, and fall.
                if let Some((t, p, o)) = self.last.filter(|l| self.time - l.0 > 1e-3) {
                    let dt = self.time - t;
                    self.velocity = (self.position - p) / dt;
                    let (axis, angle) = Quat::from_mat3(&(self.orientation * o.transpose())).to_axis_angle();
                    self.spin = axis * (angle / dt);
                }
                self.stage = Stage::Tumble;
                self.motion = self.pick(FALLS);
                self.faded = self.time + FADE;
            }
            return false;
        }
        let was = self.position;
        self.velocity.y -= GRAVITY * h;
        self.position += self.velocity * h;
        self.position += push_off_bike(bike, self.position) * h;
        let angle = self.spin.length() * h;
        if angle > 1e-6 {
            let q = Mat3::from_quat(Quat::from_axis_angle(self.spin.normalize(), angle));
            self.orientation = q * self.orientation;
        }
        if !self.points.is_empty() {
            self.touch_parts(ground, h);
        } else {
            self.touch_sphere(ground, h);
        }
        self.orientation = orthonormal(self.orientation);
        if self.time >= self.faded {
            if let Some(m) = self.queued.take() {
                self.motion = m;
                self.faded = self.time + FADE;
            }
        }
        // At rest by how far it moves: held on the ground, the body keeps
        // some speed into it.
        if self.stage == Stage::Down && (self.position - was).length() < REST_SPEED * h {
            self.still += h;
        } else {
            self.still = 0.0;
        }
        self.still >= REST_TIME || self.time >= LONGEST
    }

    /// The deepest posed corner in the ground, answered by 0x43cd50. The
    /// game's swept collision stops the body at the surface; here it is
    /// pushed out along the normal [likely]. One contact a step is what
    /// the original's slides show (2026-10-24 recordings on SX01: speed
    /// falls by about e each second down, the 0.99 per second of a single
    /// contact).
    fn touch_parts(&mut self, ground: &dyn Ground, h: f32) {
        let place = self.body();
        let mut deepest: Option<(f32, Vec3)> = None;
        for p in &self.points {
            let at = place.transform_point3(*p);
            let (height, normal) = ground.terrain(at.x, at.z);
            let normal = normal.normalize_or(Vec3::Y);
            let mut touch = ((height - at.y) * normal.y, normal);
            if let Some(hit) = ground.solid(at, CORNER_RADIUS) {
                if hit.depth > touch.0 {
                    touch = (hit.depth, hit.normal);
                }
            }
            if touch.0 > deepest.map_or(0.0, |d| d.0) {
                deepest = Some(touch);
            }
        }
        self.touching = deepest.is_some();
        let Some((depth, normal)) = deepest else {
            return;
        };
        self.position += normal * depth;
        let into = self.velocity.dot(normal);
        if into < 0.0 {
            let push = ((1.0 - IMPULSE_DROPPED) * -into * MASS).min(IMPULSE_MOST);
            self.velocity += normal * (push / MASS);
        }
        let keep = 1.0 - h * DAMPING;
        self.velocity *= keep;
        self.spin *= keep;
        if self.stage == Stage::Tumble {
            self.stage = Stage::Down;
            self.queued = Some(self.pick(HITS));
        }
    }

    /// One sphere round the body's centre, until the parts are known.
    fn touch_sphere(&mut self, ground: &dyn Ground, h: f32) {
        let (height, normal) = ground.terrain(self.position.x, self.position.z);
        let normal = normal.normalize_or(Vec3::Y);
        let mut touch = ((height + RADIUS - self.position.y) * normal.y, normal);
        if let Some(hit) = ground.solid(self.position, RADIUS) {
            if hit.depth > touch.0 {
                touch = (hit.depth, hit.normal);
            }
        }
        let (depth, normal) = touch;
        self.touching = depth > 0.0;
        if depth > 0.0 {
            self.position += normal * depth;
            let into = self.velocity.dot(normal);
            if into < 0.0 {
                self.velocity -= normal * into * (1.0 + BOUNCE);
            }
            let along = self.velocity - normal * self.velocity.dot(normal);
            let slow = (FRICTION * GRAVITY * h).min(along.length());
            self.velocity -= along.normalize_or(Vec3::ZERO) * slow;
            self.spin *= 0.9;
            if self.stage == Stage::Tumble {
                self.stage = Stage::Down;
                self.queued = Some(self.pick(HITS));
            }
        }
    }
}

/// How fast the bike's box pushes a rider part at `at` out of it, ft/s
/// (0x538050): the depth to the nearest face, times 5, out through that
/// face, in world space. Zero outside the box. (The game sums per-axis
/// depths over the parts inside and averages them; how it picks the side
/// on each axis is not clear from the code, so this takes the nearest
/// face, which a part just inside the box gives either way.)
pub fn push_off_bike(bike: Mat4, at: Vec3) -> Vec3 {
    let d = bike.inverse().transform_point3(at) - BIKE_BOX_CENTRE;
    if d.abs().cmpge(BIKE_BOX_HALF).any() {
        return Vec3::ZERO;
    }
    let depth = BIKE_BOX_HALF - d.abs();
    let axis = if depth.x <= depth.y && depth.x <= depth.z {
        Vec3::X
    } else if depth.y <= depth.z {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let out = axis * depth * d.signum();
    bike.transform_vector3(out * BIKE_BOX_PUSH)
}

/// A fall motion picked by `seed`.
pub fn fall(seed: u32) -> &'static str {
    FALLS[seed as usize % FALLS.len()]
}

fn orthonormal(m: Mat3) -> Mat3 {
    let look = m.z_axis.normalize();
    let right = m.y_axis.cross(look).normalize();
    Mat3::from_cols(right, look.cross(right), look)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Corners given in model space, in the pelvis's space at rest.
    fn model_points(points: &[Vec3]) -> Vec<Vec3> {
        points.iter().map(|p| *p - RIDER_CENTRE).collect()
    }

    #[test]
    fn pushes_the_rider_off_the_bike() {
        let bike = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        // Just right of the seat: pushed right by 5 × the 0.2 ft depth.
        let v = push_off_bike(bike, Vec3::new(10.3, 2.0, 0.0));
        assert!((v - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-4, "{v}");
        assert_eq!(push_off_bike(bike, Vec3::new(11.0, 2.0, 0.0)), Vec3::ZERO);
    }

    #[test]
    fn corners_stop_on_the_ground_without_bouncing() {
        let start = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0));
        let mut w = Wreck::new(start, Vec3::new(20.0, -10.0, 0.0), Vec3::ZERO, "Fall01", false, 1);
        w.set_pose(Mat4::from_translation(RIDER_CENTRE), model_points(&[Vec3::new(0.5, 0.5, 0.0), Vec3::new(-0.5, 0.5, 0.0)]));
        let mut rose = false;
        for _ in 0..300 {
            let was = w.velocity.y;
            if w.step(&crate::Flat, 0.02, start, Vec3::ZERO) {
                break;
            }
            rose |= w.touching && w.velocity.y > 0.0 && was <= 0.0;
            let low = w.points.iter().map(|p| w.body().transform_point3(*p).y).fold(f32::MAX, f32::min);
            assert!(low > -1e-3, "{low}");
        }
        assert!(!rose, "bounced");
        assert_eq!(w.spin, Vec3::ZERO);
        assert!(w.velocity.x.abs() < 2.0, "{}", w.velocity);
    }

    #[test]
    fn a_bail_lets_go_at_half_its_motion_with_the_pelvis_speed() {
        // LeftOver is 10 frames at 15 a second: let go at 1/3 s. The
        // motion moves the pelvis forward 0.5 ft a frame, 30 ft/s.
        let bike = Mat4::IDENTITY;
        let mut w = Wreck::new(bike, Vec3::ZERO, Vec3::ZERO, "LeftOver", true, 7);
        let h = 1.0 / 60.0;
        for k in 0..30 {
            w.set_pose(Mat4::from_translation(RIDER_CENTRE + Vec3::Z * (0.5 * k as f32)), Vec::new());
            w.step(&crate::Flat, h, bike, Vec3::ZERO);
            let t = (k + 1) as f32 * h;
            if t < 0.33 {
                assert_eq!(w.stage, Stage::Bail, "{t}");
            } else {
                assert_ne!(w.stage, Stage::Bail, "{t}");
                assert!(FALLS.contains(&w.motion), "{}", w.motion);
                break;
            }
        }
        assert!((w.velocity - Vec3::Z * 30.0).length() < 0.5, "{}", w.velocity);
    }

    #[test]
    fn the_landing_waits_for_the_fall_to_fade_in() {
        // Thrown in the air just above the ground: down at once, but the
        // landing motion only starts once the fall's 0.5 s fade is done.
        let start = Mat4::from_translation(Vec3::new(0.0, -2.0, 0.0));
        let mut w = Wreck::new(start, Vec3::new(10.0, 0.0, 0.0), Vec3::ZERO, "Fall02", false, 3);
        w.set_pose(Mat4::from_translation(RIDER_CENTRE), model_points(&[Vec3::new(0.0, 2.5, 0.0)]));
        let h = 1.0 / 60.0;
        let mut t = 0.0;
        while t < 0.7 {
            w.step(&crate::Flat, h, start, Vec3::ZERO);
            t += h;
            if t < FADE - h {
                assert_eq!(w.motion, "Fall02", "{t}");
            }
        }
        assert_eq!(w.stage, Stage::Down);
        assert!(HITS.contains(&w.motion), "{}", w.motion);
    }

    #[test]
    fn slides_as_far_as_the_original() {
        // The original's slides from 30-37 ft/s cover 23-27 ft before the
        // reset, 2.3-3 s after the crash (SX01 recordings).
        // Thrown along the ground, the body's centre 0.5 ft up.
        let start = Mat4::from_translation(Vec3::new(0.0, -2.5, 0.0));
        let mut w = Wreck::new(start, Vec3::new(33.0, 0.0, 0.0), Vec3::ZERO, "Fall01", false, 1);
        w.set_pose(Mat4::from_translation(RIDER_CENTRE), model_points(&[Vec3::new(0.0, 2.6, -2.0), Vec3::new(0.0, 2.6, 1.0)]));
        for _ in 0..(2.6f32 / 0.02) as usize {
            w.step(&crate::Flat, 0.02, start, Vec3::ZERO);
        }
        let went = w.position.x;
        assert!((20.0..34.0).contains(&went), "{went}");
    }
}
