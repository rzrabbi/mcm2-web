//! The chase camera and the terrain lookups the riders share. The
//! computer riders' bike is `mm2_physics::computer` (the game's two-wheel
//! model, 0x414000 and 0x40faa0); the player's is `mm2_physics::Bike`.

use glam::Vec3;
use mm2_formats::trn::Terrain;
use mm2_physics::camera::{Preset, View};

/// What the rider is asking for this frame; shared with the player's bike.
pub use mm2_physics::Controls;

/// The terrain's surface normal at `at` (0x50d3b0): the stored normals
/// of the corners of the triangle under it, each weighted by one over its
/// squared distance. On an even cell's lower triangle the original takes
/// the far corner's normal from (x, z+1) instead of (x+1, z+1); kept.
pub fn normal(terrain: &Terrain, at: Vec3) -> Vec3 {
    let (x, z, fx, fz) = cell(terrain, at.x, at.z);
    let d = [
        fx * fx + fz * fz,
        (1.0 - fx).powi(2) + fz * fz,
        fx * fx + (1.0 - fz).powi(2),
        (1.0 - fx).powi(2) + (1.0 - fz).powi(2),
    ]
    .map(|d: f32| if d < 0.001 { 0.001 } else { d });
    let corners = if (x ^ z) & 1 == 0 {
        if d[2] <= d[1] { [0, 2, 3] } else { [0, 1, 2] }
    } else if d[3] <= d[0] {
        [1, 2, 3]
    } else {
        [0, 1, 2]
    };
    let mut n = Vec3::ZERO;
    for c in corners {
        let s = terrain.normals[(z + c / 2) * terrain.size + x + c % 2];
        n += Vec3::new(s[0] as f32, s[1] as f32, s[2] as f32) / 127.0 / d[c];
    }
    n.normalize_or(Vec3::Y)
}

/// The Chase Cam (view 0). Its follow
/// point is 11.72 ft from the bike's centre of mass at 60° elevation
/// (10.15 ft up, 5.9 ft behind), looking at a point 12.7 ft ahead, which
/// puts the seat at the bottom of the frame at rest, as in the original.
/// The FollowCam eases its eye and its look point towards those through
/// filters (0x466b30), so at speed the eye trails well behind: in the
/// original's frames at 65 ft/s the bike is about 40 ft off, just below
/// the middle of the screen. The yaw trails the heading. In a crash it
/// stops turning, backs out to 24.5 ft at 19° and watches; it swings back
/// in after.
#[derive(Debug, Clone, Copy)]
pub struct Chase {
    eye: Vec3,
    target: Vec3,
    /// The way the camera faces, on the level.
    yaw: Vec3,
    /// 0 riding to 1 watching a crash.
    crash: f32,
    /// The view the camera button picked (section 20), kept between
    /// races like the game's global +0x2930.
    pub view: View,
    /// The field of view, degrees, easing toward `want_fov`.
    fov: f32,
    want_fov: f32,
    /// Where and how fast the bike left the ground last (bike +0x488,
    /// +0x494), for the Thrill Cam.
    takeoff: (Vec3, Vec3),
    was_airborne: bool,
    /// The Thrill Cam's spot while it is held.
    thrill: Option<Vec3>,
    /// The frozen camera's spot (mode 5, Numpad 0, 0x4679e0): the eye
    /// stays where it was and turns to watch the bike.
    frozen: Option<Vec3>,
}

/// What the player holds for the camera this frame: the Thrill Cam and
/// the Reverse Cam (actions 10 and 11).
#[derive(Debug, Clone, Copy, Default)]
pub struct CamKeys {
    pub thrill: bool,
    pub reverse: bool,
}

/// The bike the camera follows this frame.
#[derive(Debug, Clone, Copy)]
pub struct Followed {
    pub position: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
    pub velocity: Vec3,
    pub speed: f32,
    pub crashed: bool,
    pub airborne: bool,
    /// The rear tyre is more than 2 ft off the ground (0x499050's
    /// rear depth below −2).
    pub high: bool,
}

impl Chase {
    const HEIGHT: f32 = 10.15;
    const BEHIND: f32 = 5.86;
    const AHEAD: f32 = 12.7;
    /// How long the eye takes to catch up on the level, s. The original's
    /// listener trails 5.9 ft at rest and 31 ft at 91 ft/s, always 10.15 up.
    const EYE_LAG: f32 = 0.3;
    const TARGET_LAG: f32 = 0.05;
    /// The look point runs this far ahead per ft/s, s: the pitch eases from
    /// 28° down at rest to 9.6° at 91 ft/s.
    const LOOK_AHEAD: f32 = 0.2;
    /// How fast the yaw catches the heading, s.
    const YAW_LAG: f32 = 0.02;
    const CRASH_DISTANCE: f32 = 24.5;
    const CRASH_PITCH: f32 = 0.332;
    /// How fast the camera backs out of a crash and swings back, 1/s.
    const CRASH_EASE: f32 = 3.0;
    /// Kept this far above the ground, ft.
    const CLEARANCE: f32 = 3.0;
    /// The Reverse Cam (0x531610): 17 ft away at 0.75 rad, turned round.
    const REVERSE: Preset = Preset {
        distance: 17.0,
        elevation: 0.75,
        yaw: std::f32::consts::PI,
        fov: Some(85.0),
    };
    /// The rider's head above the bike's centre, ft.
    const HEAD: f32 = 3.0;
    /// How fast the field of view follows, 1/s.
    const FOV_EASE: f32 = 4.0;

    /// Behind a bike at `position` heading `forward`.
    pub fn new(position: Vec3, forward: Vec3) -> Self {
        let yaw = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::Z);
        Chase {
            eye: position - yaw * Self::BEHIND + Vec3::Y * Self::HEIGHT,
            target: position + yaw * Self::AHEAD,
            yaw,
            crash: 0.0,
            view: View::Chase,
            fov: 85.0,
            want_fov: 85.0,
            takeoff: (position, Vec3::ZERO),
            was_airborne: false,
            thrill: None,
            frozen: None,
        }
    }

    /// The same camera behind a new spot, keeping the view and zoom.
    pub fn restart(&self, position: Vec3, forward: Vec3) -> Self {
        Chase {
            view: self.view,
            fov: self.want_fov,
            want_fov: self.want_fov,
            frozen: self.frozen,
            ..Chase::new(position, forward)
        }
    }

    /// The next view on a camera press; the view's name, to flash. A
    /// frozen camera goes back to the view it froze in instead (0x4679e0,
    /// and the replay's camera button, 0x489c20).
    pub fn next_view(&mut self) -> &'static str {
        if self.frozen.take().is_some() {
            return self.view.name();
        }
        self.view = self.view.next();
        if let Some(fov) = self.view.preset().and_then(|p| p.fov) {
            self.want_fov = fov;
        }
        self.view.name()
    }

    /// Numpad 0 (0x4679e0's key 0x52): freezes the camera where it is, or
    /// lets it go back to its view. Whether it is frozen now.
    pub fn freeze(&mut self) -> bool {
        self.frozen = match self.frozen {
            Some(_) => None,
            None => Some(self.eye),
        };
        self.frozen.is_some()
    }

    /// Home and End: the field of view narrows or widens by 3° (10° to
    /// 120°).
    pub fn zoom(&mut self, by: f32) {
        self.want_fov = (self.want_fov + by * mm2_physics::camera::FOV_STEP).clamp(10.0, 120.0);
    }

    /// The field of view, degrees.
    pub fn fov(&self) -> f32 {
        self.fov
    }

    /// Moves towards the spot behind the bike and returns the view, in
    /// game coordinates: the eye and the point it looks at.
    pub fn follow(&mut self, bike: &Followed, keys: CamKeys, terrain: &Terrain, dt: f32) -> (Vec3, Vec3) {
        let Followed {
            position,
            forward,
            speed,
            crashed,
            ..
        } = *bike;
        if bike.airborne && !self.was_airborne {
            self.takeoff = (position, bike.velocity);
        }
        self.was_airborne = bike.airborne;
        if !crashed {
            let flat = Vec3::new(forward.x, 0.0, forward.z).normalize_or(self.yaw);
            let k = (dt / Self::YAW_LAG).min(1.0);
            self.yaw = (self.yaw + (flat - self.yaw) * k).normalize_or(flat);
        }
        let want = if crashed { 1.0 } else { 0.0 };
        self.crash += (want - self.crash) * (Self::CRASH_EASE * dt).min(1.0);

        // The Thrill Cam (0x499050, 0x531360) while held in the air: a spot
        // ahead of the jump, out to the side, picked from where and how
        // fast the bike took off, watching it fly past.
        if keys.thrill && bike.airborne && bike.high && !crashed {
            let (p, v) = self.takeoff;
            let spot = *self.thrill.get_or_insert_with(|| {
                let t = v.y * (1.0 / 32.2) * 1.5;
                let mut base = p.y;
                if v.y > 10.0 {
                    base += v.y * t * 0.19;
                }
                let mut at = Vec3::new(
                    p.x + v.x * t - 0.2 * v.x + 0.45 * v.z,
                    p.y + v.y * t - 0.2 * v.y,
                    p.z + v.z * t - 0.2 * v.z - 0.45 * v.x,
                );
                at.y = at.y.max(ground(terrain, at.x, at.z));
                at.y = (at.y + 7.0).max(base - 1.0);
                at
            });
            let target = position + Vec3::Y * 3.0;
            let d = Vec3::new(spot.x - target.x, 0.0, spot.z - target.z).length();
            self.fov = ((200.0 - d) / 180.0 * 60.0 + 10.0).clamp(10.0, 70.0);
            self.eye = spot;
            self.target = target;
            return (spot, target);
        }
        self.thrill = None;

        // The frozen camera (mode 5): the eye stays put and looks 3 ft
        // over the bike, led along the way it goes (0x4992f0), and the
        // view narrows with distance, 70° at 20 ft to 10° at 280 ft
        // (0x466b30, limits from 0x45e7c0's +0x2f4 to +0x300).
        if let Some(eye) = self.frozen {
            let d = (eye - position).length();
            self.fov = (10.0 + 60.0 * (280.0 - d) / 260.0).clamp(10.0, 70.0);
            // The lead: the bike's speed along its heading, scaled up
            // with distance, over its 180 ft/s top (+0x438), and by how
            // narrow the view is.
            let k = d * 0.009 + 1.0;
            let lead = k * k * bike.velocity.dot(self.yaw) / 180.0
                * (1.0 - d * 0.000526316)
                * (self.fov / self.want_fov)
                * 35.0;
            let target = position + self.yaw * lead + Vec3::Y * 3.0;
            self.eye = eye;
            self.target = target;
            return (eye, target);
        }
        self.fov += (self.want_fov - self.fov) * (Self::FOV_EASE * dt).min(1.0);

        // The view's spot: the Chase Cam's tuned follow, or another
        // distance, elevation and turn from behind (0x531b80).
        let preset = if keys.reverse && !crashed {
            Some(Self::REVERSE)
        } else {
            self.view.preset()
        };
        let (ride_eye, ride_target) = match (self.view, preset) {
            (View::Rider, _) if !keys.reverse => {
                let head = position + bike.up * Self::HEAD;
                let k = 1.0 - (-dt / 0.02).exp();
                self.eye += (head - self.eye) * k;
                self.target = head + forward * 30.0;
                let floor = ground(terrain, self.eye.x, self.eye.z) + 0.5;
                self.eye.y = self.eye.y.max(floor);
                return (self.eye, self.target);
            }
            (View::Chase, _) if !keys.reverse => {
                let ahead = Self::AHEAD + speed.max(0.0) * Self::LOOK_AHEAD * (1.0 - self.crash);
                (position - self.yaw * Self::BEHIND + Vec3::Y * Self::HEIGHT, position + self.yaw * ahead)
            }
            (_, p) => {
                let p = p.unwrap_or(Preset {
                    distance: 11.72,
                    elevation: std::f32::consts::FRAC_PI_3,
                    yaw: 0.0,
                    fov: None,
                });
                let (s, c) = p.yaw.sin_cos();
                let back = Vec3::new(self.yaw.x * c - self.yaw.z * s, 0.0, self.yaw.x * s + self.yaw.z * c);
                let eye = position - back * (p.distance * p.elevation.cos())
                    + Vec3::Y * (p.distance * p.elevation.sin());
                (eye, position + self.yaw * (speed.max(0.0) * Self::LOOK_AHEAD * 0.5))
            }
        };
        let (sin, cos) = Self::CRASH_PITCH.sin_cos();
        let crash_eye = position - self.yaw * (Self::CRASH_DISTANCE * cos)
            + Vec3::Y * (Self::CRASH_DISTANCE * sin);
        let c = self.crash;
        let eye = ride_eye.lerp(crash_eye, c);
        let target = ride_target.lerp(position, c);
        let k = 1.0 - (-dt / Self::EYE_LAG).exp();
        self.eye.x += (eye.x - self.eye.x) * k;
        self.eye.z += (eye.z - self.eye.z) * k;
        // The height doesn't lag while riding.
        self.eye.y = eye.y + (self.eye.y + (eye.y - self.eye.y) * k - eye.y) * c;
        self.target += (target - self.target) * (1.0 - (-dt / Self::TARGET_LAG).exp());
        let floor = ground(terrain, self.eye.x, self.eye.z) + Self::CLEARANCE;
        self.eye.y = self.eye.y.max(floor);
        (self.eye, self.target)
    }
}

/// The terrain cell under game `x`, `z` and the offset into it, in
/// terrain units.
fn cell(terrain: &Terrain, x: f32, z: f32) -> (usize, usize, f32, f32) {
    let last = terrain.size.saturating_sub(2) as f32;
    let (tx, tz) = ((x / terrain.scale).max(0.0), (z / terrain.scale).max(0.0));
    let (cx, cz) = (tx.floor().min(last), tz.floor().min(last));
    (cx as usize, cz as usize, tx - cx, tz - cz)
}

/// Terrain height in game units at game `x`, `z` (0x50d3b0). Each cell is
/// two triangles, split from (x, z) to (x+1, z+1) on cells where x + z is
/// even and from (x+1, z) to (x, z+1) where it is odd.
pub fn ground(terrain: &Terrain, x: f32, z: f32) -> f32 {
    let (cx, cz, fx, fz) = cell(terrain, x, z);
    let h = |dx: usize, dz: usize| terrain.height(cx + dx, cz + dz);
    let (h00, h10, h01, h11) = (h(0, 0), h(1, 0), h(0, 1), h(1, 1));
    let y = if (cx ^ cz) & 1 == 0 {
        if fx <= fz {
            (h11 - h01) * fx + (h00 - h01) * (1.0 - fz) + h01
        } else {
            (h10 - h00) * fx + (h11 - h10) * fz + h00
        }
    } else if fx + fz >= 1.0 {
        (h11 - h01) * fx + (h10 - h11) * (1.0 - fz) + h01
    } else {
        (h10 - h00) * fx + (h01 - h00) * fz + h00
    };
    y * terrain.scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_camera_stays_and_narrows_with_distance() {
        let terrain = Terrain {
            size: 9,
            heights: vec![0.0; 81],
            normals: vec![[0, 127, 0]; 81],
            surfaces: vec![0; 81],
            scale: 30.0,
            textures: Vec::new(),
            tiles: Vec::new(),
        };
        let mut c = Chase::new(Vec3::ZERO, Vec3::Z);
        let mut bike = Followed {
            position: Vec3::ZERO,
            forward: Vec3::Z,
            up: Vec3::Y,
            velocity: Vec3::ZERO,
            speed: 0.0,
            crashed: false,
            airborne: false,
            high: false,
        };
        let (eye, _) = c.follow(&bike, CamKeys::default(), &terrain, 0.1);
        assert!(c.freeze());
        bike.position = Vec3::new(0.0, 0.0, 270.0);
        let (still, target) = c.follow(&bike, CamKeys::default(), &terrain, 0.1);
        assert_eq!(still, eye);
        assert_eq!(target, bike.position + Vec3::Y * 3.0);
        assert!(c.fov() < 15.0, "{}", c.fov());
        // The camera button lets it go, keeping the view.
        assert_eq!(c.next_view(), View::Chase.name());
        let (moved, _) = c.follow(&bike, CamKeys::default(), &terrain, 0.1);
        assert_ne!(moved, eye);
    }
}
