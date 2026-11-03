//! The computer riders' bike (`Bike_SimulateSimple` 0x414000 and its
//! two-wheel solver 0x40faa0).
//!
//! The bike is a dumbbell: a front and a rear point 3.5 ft either side of
//! the model's origin, each on a spring of its own. The rear drives and
//! brakes toward the line's speed limit, the front is steered straight at
//! the line (no bars: its level velocity is turned by at most a fifth of
//! its speed a step), and the rear's sideways velocity is taken out
//! whole. The bike's up leans with the sideways pull. Everything here is
//! the game's own numbers, including its slips: the rear's final ground
//! check uses the front's spring and last substep's rear point, and the
//! air lean is one value shared by every computer rider.

use std::sync::Arc;

use glam::{Mat3, Vec3};

use crate::ai::{self, Path};
use crate::Ground;

/// Half the dumbbell, ft.
pub const HALF_BASE: f32 = 3.5;
const GRAVITY: f32 = 32.0;
/// Spring per unit of compression, both ends.
const SPRING: f32 = 90.0;
/// A wheel point more than this above the ground is off it, ft.
const CLEAR: f32 = 0.5;
/// The longest substep, s.
const LONGEST: f32 = 0.05;
/// cos 45°: the most the front is aimed off the bike's heading.
const REACH: f32 = 0.707_55;
/// The front's turn a step, as a share of its speed.
const TURN: f32 = 0.2;
/// The hardest braking, ft/s².
const BRAKE: f32 = 256.0;
/// The drive's split between the two ends.
const FRONT_SHARE: f32 = 0.35;
const REAR_SHARE: f32 = 0.65;
/// The up leans toward the sideways pull plus this much straight up.
const UPRIGHT: f32 = 512.0;
/// Crashes (0x414000): a speed change in one frame, a hit into the ground
/// (bike +0x778), or a solid touched for longer than this.
const CRASH_SPEED: f32 = 44.0;
const CRASH_IMPACT: f32 = 100.0;
const CRASH_STUCK: f32 = 0.5;
/// The player's crash rules (0x40a950) run on the computer riders too:
/// the one that can fire on them is a loss of speed in one frame, more
/// than 1000 × the frame (at least 0.02 s) over the level's tolerance
/// (0x492950).
const CRASH_DROP: f32 = 1000.0;
const SHORTEST_STEP: f32 = 0.02;

/// The tyres' radii, front and rear, ft (as the full model's).
const WHEEL_RADII: [f32; 2] = [1.177, 1.143];

/// The computer riders' speed-loss tolerance (0x492950) by level 1–3: in
/// Supercross 4, 3.25, 2.5; elsewhere 3.75, 3.25, 2.5. The harder riders
/// take more.
pub fn tolerance(level: usize, supercross: bool) -> f32 {
    let t = if supercross { [4.0, 3.25, 2.5] } else { [3.75, 3.25, 2.5] };
    t[level.min(2)]
}

/// The game's level power (bike +0x7c4, +0x7c8, +0x7cc) by AI level 1–3.
pub const POWER: [f32; 3] = [1.1, 1.3, 1.5];

/// How much a bike's engine class (bike +0x79c) pulls: by its cc, under
/// 126 the first, under 500 the second, else the third (0x490c30).
pub fn engine_class(cc: u32) -> usize {
    if cc < 126 {
        0
    } else if cc < 500 {
        1
    } else {
        2
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rider {
    /// The model's origin (+0xc), between the ends.
    pub position: Vec3,
    /// Along and up the bike (+0x88, +0x94).
    pub look: Vec3,
    pub up: Vec3,
    pub velocity: Vec3,
    /// Each end's own velocity (+0x7e0, +0x7ec).
    pub front: Vec3,
    pub rear: Vec3,
    /// The springs, 0 to 1 (+0x7f8, +0x7fc).
    pub springs: [f32; 2],
    /// Spin, rad/s (+0xcc), from the ends' velocities.
    pub spin: Vec3,
    /// Speed (+0xbc) and last frame's (+0xb8).
    pub speed: f32,
    pub last_speed: f32,
    /// In the air (+0x108): both ends off the ground.
    pub airborne: bool,
    /// Which ends touched the ground at the end of the step.
    pub down: [bool; 2],
    /// The rider's steer and lean as the riding motions read them
    /// (+0x504, +0x508), −1..1.
    pub steer: f32,
    pub lean: f32,
    /// The gas and brake as the engine sound hears them (+0x804, +0x808).
    pub gas: bool,
    pub brake: bool,
    /// Time touching a solid (+0x814).
    pub stuck: f32,
    /// How far to the side of the line this rider keeps (+0x824).
    pub lane: f32,
    /// The bike's engine class (+0x79c), an index into the thrust tables.
    pub class: usize,
    /// How much speed a frame may take before it throws the rider, see
    /// [`tolerance`].
    pub tolerance: f32,
    /// The air pose is on (`f_6fc`): in the air, unless a trick is
    /// running. The caller sets it.
    pub air_pose: bool,
    /// The bike's collision points (BIKE.RES `honda.col`) and how much
    /// the bike and rider weigh, lb, for [`Rider::hit_objects`].
    pub hull: Option<Arc<crate::bike::Hull>>,
    pub weight: f32,
    /// Where the model stood last frame: its origin and axes.
    previous: Option<(Vec3, Mat3)>,
    /// The axes the spin was last measured against. The game keeps the
    /// spin in the bike's own axes (+0xd8) and turns it back into the
    /// world's from the new axes at the start of each frame (0x414000),
    /// so it turns with the bike.
    spin_axes: Option<Mat3>,
}

impl Rider {
    /// A bike standing at `origin` facing `direction`.
    pub fn new(ground: &dyn Ground, origin: Vec3, direction: Vec3, lane: f32, class: usize) -> Self {
        let look = Vec3::new(direction.x, 0.0, direction.z).normalize_or(Vec3::Z);
        let (h, _) = ground.terrain(origin.x, origin.z);
        Rider {
            position: Vec3::new(origin.x, h, origin.z),
            look,
            up: Vec3::Y,
            velocity: Vec3::ZERO,
            front: Vec3::ZERO,
            rear: Vec3::ZERO,
            springs: [0.0; 2],
            spin: Vec3::ZERO,
            speed: 0.0,
            last_speed: 0.0,
            airborne: false,
            down: [true; 2],
            steer: 0.0,
            lean: 0.0,
            gas: false,
            brake: false,
            stuck: 0.0,
            lane,
            class: class.min(2),
            tolerance: tolerance(1, true),
            hull: None,
            weight: 220.0 + crate::bike::RIDER,
            previous: None,
            spin_axes: None,
            air_pose: true,
        }
    }

    /// Forgets the axes the spin was measured against, for a state set
    /// from outside (a tape) whose spin is in the world's axes.
    pub fn reset_spin_axes(&mut self) {
        self.spin_axes = None;
    }

    /// Steps the bike by a frame of `dt` along `path` (0x414000's computer
    /// branch). `air_lean` is the game's one smoothed air lean, shared by
    /// all computer riders (0x57dc10); `gates` is a Baja or Enduro race.
    /// Returns true when the rider crashes.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        ground: &dyn Ground,
        path: &mut Path,
        dt: f32,
        air_lean: &mut f32,
        gates: bool,
        movers: &[ai::Mover],
    ) -> bool {
        // The pitch the rider asks for in the air, and the lean it shows.
        let mut pitch = 0.0;
        let lean_to;
        if self.airborne {
            let (air, normal) = flight(ground, self.position, self.velocity);
            let off = normal.dot(self.look) - 0.087;
            pitch = if air > 0.0 { off * HALF_BASE / air } else { off };
            let target = if off > 0.0 { 1.0 } else { -1.0 };
            let k = dt.min(0.5) / 0.5;
            *air_lean += (target - *air_lean) * k;
            lean_to = air_lean.clamp(-1.0, 1.0);
        } else {
            let mut l = self.look.y;
            if l.abs() < 0.3 {
                l = 0.0;
            }
            l *= 4.0;
            lean_to = if l.abs() <= 0.1 {
                l
            } else if l <= 0.0 {
                -0.1
            } else {
                0.1
            };
        }
        let wanted = path.limit - self.speed;
        if self.airborne || wanted <= 0.0 {
            self.gas = false;
            self.brake = wanted < -20.0;
        } else {
            self.gas = true;
            self.brake = false;
        }

        // The spin from the bike's own axes, then the ends' velocities from
        // the body's.
        let axes = self.axes();
        if let Some(was) = self.spin_axes.replace(axes) {
            self.spin = axes * (was.transpose() * self.spin);
        }
        let arm = self.look * HALF_BASE;
        self.front = self.velocity + self.spin.cross(arm);
        self.rear = self.velocity - self.spin.cross(arm);
        let mut impact = 0.0;
        self.solve(ground, path, pitch, dt, &mut impact, gates, movers);
        let d = self.front - self.velocity;
        self.spin = -(d.cross(self.look)) * (1.0 / HALF_BASE);
        // The bike's air routine (Bk 0x40bcc0) runs on the computer
        // riders too: the air pose bends the path, and the spin is
        // damped ×0.9 a frame (the AI tape's pitch in the air matches only
        // with it; on the ground it doesn't run).
        if self.airborne {
            if self.air_pose {
                self.bend(dt);
            }
            self.spin *= 0.9;
        }
        self.speed = self.velocity.length();

        // The rider's steer follows the turn, the lean its target.
        let steer = self.spin.y * -1.2732;
        self.steer = toward(self.steer, steer, 0.125).clamp(-1.0, 1.0);
        self.lean = toward(self.lean, lean_to, 0.25).clamp(-1.0, 1.0);
        if self.airborne {
            self.steer = 0.0;
        }

        // The bike's own collision object against the solids (0x48ea00 →
        // 0x52fc60). Bikes (type 100), riders (101) and trees with a .col
        // (1000) don't count towards the time stuck; every solid the port
        // loads is of another type.
        // The speed-loss rule (0x40bc50) runs before the solids; the
        // solids set the speed afresh (0x52bb10), and the checks after
        // them and next frame's last speed (+0xb8) take that one.
        let dropped = (self.speed - self.last_speed) * self.tolerance < -CRASH_DROP * dt.max(SHORTEST_STEP);
        let touching = self.hit_objects(ground);
        if touching {
            self.stuck += dt;
            self.speed = self.velocity.length();
        } else {
            self.stuck = 0.0;
        }
        let crash = dropped
            || impact.abs() > CRASH_IMPACT
            || (self.speed - self.last_speed).abs() > CRASH_SPEED
            || self.up.y <= 0.0
            || self.stuck > CRASH_STUCK;
        self.last_speed = self.speed;
        crash
    }

    /// The air pose's path bend (Bk 0x40bcc0, the same as the player's):
    /// with the bike pointing 10° to 57° off its path, the path is pushed
    /// sideways, always to the same side (the steer pose's phase, 0 for
    /// the computer riders), and slowed by as much. On the AI tape it
    /// takes up to 5 ft/s off a long jump, nose up over the far side.
    fn bend(&mut self, dt: f32) {
        let speed = self.velocity.length();
        if speed <= 1e-3 {
            return;
        }
        let dir = self.velocity / speed;
        let dot = dir.dot(self.look);
        if dot > 0.55 && dot < 0.984 {
            let across = Vec3::new(dir.z, 0.0, -dir.x);
            let push = (1.0 - dot) * 3.412_97 * speed * 0.075 * dt;
            self.velocity += across * push;
            self.velocity *= speed / (push + speed);
        }
    }

    /// The model's axes: right, up, look.
    pub fn axes(&self) -> Mat3 {
        let right = self.up.cross(self.look).normalize_or(Vec3::X);
        Mat3::from_cols(right, self.look.cross(right), self.look)
    }

    /// The collision points against solid objects, as the player's bike
    /// (0x52fc60, 0x439e10): each point that passed through a face from
    /// its front since last frame is pushed back out, and the bike bounces
    /// off at their middle (0x405fb0 → 0x5050b0, restitution 0.1). Whether
    /// anything was touched.
    fn hit_objects(&mut self, ground: &dyn Ground) -> bool {
        let axes = self.axes();
        let was = self.previous.replace((self.position, axes));
        let (Some(hull), Some((was_at, was_axes))) = (self.hull.clone(), was) else {
            return false;
        };
        let mut out = Vec3::ZERO;
        let (mut middle, mut count) = (Vec3::ZERO, 0);
        let mut normals: Vec<Vec3> = Vec::new();
        let mut moving = Vec3::ZERO;
        let wheels = [0, 1].map(|i| (crate::bike::axle(i, 0.0), &hull.wheels[i]));
        for (offset, points) in [(Vec3::ZERO, &hull.body)].into_iter().chain(wheels) {
            for &p in points {
                let from = was_at + was_axes * (p + offset);
                let to = self.position + axes * (p + offset);
                for _ in 0..4 {
                    let Some(c) = ground.crossing(from, to + out) else {
                        break;
                    };
                    out += c.normal * c.depth;
                    middle += c.point;
                    moving = c.velocity;
                    count += 1;
                    if !normals.iter().any(|n| n.distance(c.normal) < 1e-4) {
                        normals.push(c.normal);
                    }
                }
            }
        }
        if count == 0 {
            return false;
        }
        let normal = normals.iter().sum::<Vec3>().normalize_or_zero();
        self.position += out + out.normalize_or_zero() * crate::bike::PUSH_MARGIN;
        self.previous = Some((self.position, axes));
        let com = self.position + axes * crate::bike::COM;
        let r = middle / count as f32 - com;
        let into = normal.dot(self.velocity + self.spin.cross(r) - moving);
        if into < 0.0 {
            let mass = self.weight * 0.031_055_9;
            let inertia = crate::bike::INERTIA * mass / 12.0;
            let twist = axes * (axes.transpose() * r.cross(normal) / inertia);
            let turn = twist.cross(r).dot(normal);
            let j = -(1.0 + crate::bike::RESTITUTION) * into / (1.0 / mass + turn);
            self.velocity += normal * j / mass;
            self.spin += twist * j;
        }
        true
    }

    /// The two-wheel solver 0x40faa0, as the computer riders call it: a
    /// frame of `dt` in substeps of at most 0.05 s.
    #[allow(clippy::too_many_arguments)]
    fn solve(
        &mut self,
        ground: &dyn Ground,
        path: &mut Path,
        pitch: f32,
        dt: f32,
        impact: &mut f32,
        gates: bool,
        movers: &[ai::Mover],
    ) {
        let mut h = dt.min(LONGEST);
        // Damping, the same both ends.
        let damp = ((h - 0.005) * 4.21053 + 0.6) * 0.6;
        let power = path.power;
        let thrust = &crate::thrust::THRUST[self.class];
        let mut half_h2 = h * h * 0.5;

        let mut look = self.look;
        let mut up = self.up;
        let (mut sf, mut sr) = (self.springs[0], self.springs[1]);
        let mut pf = self.position + look * HALF_BASE;
        let mut pr = self.position - look * HALF_BASE;
        let (mut vf, mut vr) = (self.front, self.rear);
        let mut side = level(Vec3::new(look.z, 0.0, -look.x));

        let lane = self.lane;
        let aim = |pf: Vec3, vf: Vec3, look: Vec3, side: Vec3, path: &mut Path| -> Vec3 {
            let mut d = path_aim(path, pf, lane, look);
            let jump = path.plan_jump(ground, pf, vf, lane);
            if let Some(s) = jump.speed {
                path.limit = s;
            }
            if let Some(a) = jump.aim {
                d = level(a);
            }
            if d.dot(look) < REACH {
                d = if d.dot(side) >= 0.0 {
                    (look + side).normalize_or_zero()
                } else {
                    (look - side).normalize_or_zero()
                };
            }
            d
        };
        // The aim is made here and after each substep.
        let mut dir = aim(pf, vf, look, side, path);

        let (mut front_down, mut rear_down) = (false, false);
        let mut airborne = true;
        let mut rear_query = pr;
        let mut elapsed = 0.0;
        if dt > 0.0 {
            while elapsed < dt {
                let mut af = Vec3::new(0.0, -GRAVITY, 0.0);
                let mut ar = Vec3::new(0.0, -GRAVITY, 0.0);
                let mut hit = 0.0;
                rear_query = pr;
                let (gf, nf) = surface(ground, pf, 4.0, 5.0);
                let (gr, nr) = surface(ground, pr, 4.0, 5.0);
                side = look.cross(nr).normalize_or(side);
                let speed_f = vf.length();
                let speed_r = vr.length();

                // Front.
                let force = sf * SPRING;
                let along_up = up.dot(nf);
                let gap = (pf + up * sf - Vec3::new(pf.x, gf, pf.z)).dot(nf);
                let mut front_spring = 0.0;
                front_down = gap <= CLEAR;
                if !front_down {
                    sf = (sf - force * half_h2 * 3.0).max(0.0);
                } else {
                    let into = vf.dot(nf);
                    let give = into * along_up * damp;
                    if into < 0.0 {
                        if sf >= 1.0 {
                            vf -= nf * into;
                        } else {
                            vf -= nf * into - up * give;
                        }
                        hit = into;
                        let now = vf.length();
                        if now < speed_f {
                            vf = vf * 0.9 + vf.normalize_or_zero() * speed_f * 0.1;
                        }
                    }
                    if gap < 0.0 {
                        pf -= nf * gap;
                    }
                    let pull = af.dot(nf);
                    front_spring = pull * along_up;
                    if pull < 0.0 {
                        af -= nf * pull;
                    }
                    af += up * (along_up * force + front_spring);
                    sf = (sf - (give * h + along_up * force * half_h2)).clamp(0.0, 1.0);
                }

                // Rear.
                let force = sr * SPRING;
                let along_up = up.dot(nr);
                let gap = (pr + up * sr - Vec3::new(pr.x, gr, pr.z)).dot(nr);
                let mut rear_spring = 0.0;
                rear_down = gap <= CLEAR;
                if !rear_down {
                    sr = (sr - force * half_h2 * 3.0).max(0.0);
                } else {
                    let into = vr.dot(nr);
                    let give = into * along_up * damp;
                    if into < 0.0 {
                        hit += into;
                        if sr >= 1.0 {
                            vr -= nr * into;
                        } else {
                            vr -= nr * into - up * give;
                        }
                        let now = vr.length();
                        if now < speed_r {
                            vr = vr * 0.9 + vr.normalize_or_zero() * speed_r * 0.1;
                        }
                    }
                    if gap < 0.0 {
                        pr -= nr * gap;
                    }
                    let pull = ar.dot(nr);
                    rear_spring = pull * along_up;
                    if pull < 0.0 {
                        ar -= nr * pull;
                    }
                    ar += up * (along_up * force + rear_spring);
                    sr = (sr - (give * h + along_up * force * half_h2)).clamp(0.0, 1.0);
                }
                airborne = !front_down && !rear_down;

                // Steering the front: at the line, out of the way of what
                // the scan finds, at most 45° off the heading.
                if rear_down || front_down {
                    // With the front off the ground it goes on the way
                    // it is going.
                    let mut d = if front_down { dir } else { level(vf) };
                    let from = pf + Vec3::Y * 3.0;
                    let to = pf + (vf + vr) * 0.25 + Vec3::Y * 3.0;
                    let scan = ai::scan(ground, from, to, 2.0, speed_f, movers, &[], false);
                    if let (true, Some(o)) = (scan.blocked, scan.toward) {
                        let o = level(o);
                        let k = vf.dot(o);
                        if k > 0.0 {
                            d = level(vf - o * k);
                        }
                    }
                    let heading = level(look);
                    if heading.dot(d) < REACH {
                        let s = if d.dot(side) <= 0.0 { look - side } else { look + side };
                        d = level(s);
                    }
                    if gates {
                        d = level(level(vf) * 5.0 + d);
                    }
                    let flat_speed = ai::rough_sqrt(vf.x * vf.x + vf.z * vf.z);
                    let want = d * flat_speed + scan.nudge;
                    let change = Vec3::new(want.x - vf.x, 0.0, want.z - vf.z);
                    let cap = speed_f * TURN;
                    let change = change.normalize_or_zero() * change.length().min(cap);
                    af += change / h;
                }

                // The rear grips sideways completely.
                let slide = ar.dot(side) + vr.dot(side) / h;
                if front_down || rear_down {
                    ar -= side * slide;
                    if rear_down {
                        // Drive or brake toward the limit, with the
                        // engine's pull at this speed times the level's
                        // power as the most.
                        let mut f = (path.limit - speed_r) / h;
                        let i = (speed_r as i32).clamp(0, 199) as usize;
                        let most = power * thrust[i];
                        if f > most {
                            f = most;
                        } else if f < -BRAKE {
                            f = -BRAKE;
                        }
                        if f >= 0.0 {
                            ar += look * f;
                        } else if -f < speed_r / h {
                            if speed_r > 0.0 {
                                ar += vr * (f / speed_r);
                            }
                        } else {
                            vr = Vec3::ZERO;
                            ar = up * rear_spring;
                            if front_down {
                                vf = Vec3::ZERO;
                                af = up * front_spring;
                            }
                        }
                    }
                }
                // The game compares the size of the hit with the signed
                // value it keeps, so after the first hit every substep's
                // hit replaces it.
                if hit.abs() > *impact {
                    *impact = hit;
                }

                // The ends share their pull and speed along the bike.
                let share = |a: Vec3, b: Vec3| {
                    let (x, y) = (a.dot(look), b.dot(look));
                    let both = x * FRONT_SHARE + y * REAR_SHARE;
                    (a + look * (both - x), b + look * (both - y))
                };
                (af, ar) = share(af, ar);
                (vf, vr) = share(vf, vr);

                pf += af * half_h2 + vf * h;
                pr += ar * half_h2 + vr * h;
                if airborne {
                    pf -= up * pitch * h;
                    pr += up * pitch * h;
                }
                vf += af * h;
                vr += ar * h;
                look = (pf - pr).normalize_or(look);
                side = level(Vec3::new(look.z, 0.0, -look.x));
                dir = aim(pf, vf, look, side, path);
                *impact *= 0.5;

                // Up leans into the sideways pull.
                let pull = af + ar;
                let lean = (side * pull.dot(side) + Vec3::Y * UPRIGHT).normalize_or(Vec3::Y);
                up = (lean * h + up * 4.0 * dt) * (1.0 / (4.0 * dt + h));
                let across = up.cross(look);
                up = look.cross(across).normalize_or(Vec3::Y);

                elapsed += h;
                let left = dt - elapsed;
                if left < h {
                    h = left;
                    half_h2 = h * h * 0.5;
                }
            }
        }

        // Last, out of the ground: the front by its own spring, the rear
        // by the front's, against where the rear stood a substep ago.
        let (gf, nf) = surface(ground, pf, 1.0, 2.0);
        let (gr, nr) = surface(ground, rear_query, 1.0, 2.0);
        let g = (pf + up * sf - Vec3::new(pf.x, gf, pf.z)).dot(nf);
        if g < 0.0 {
            pf -= nf * g;
        }
        let g = (pr + up * sf - Vec3::new(rear_query.x, gr, rear_query.z)).dot(nr);
        if g < 0.0 {
            pr -= nr * g;
        }
        self.position = (pf + pr) * 0.5;
        self.look = look;
        self.up = up;
        self.velocity = (vf + vr) * 0.5;
        self.front = vf;
        self.rear = vr;
        if airborne {
            self.front.x = self.velocity.x;
            self.front.z = self.velocity.z;
            self.rear.x = self.velocity.x;
            self.rear.z = self.velocity.z;
        }
        self.airborne = airborne;
        self.down = [front_down, rear_down];
        self.springs = [sf, sr];
    }

    /// Forward speed along the bike (+0x434).
    pub fn forward_speed(&self) -> f32 {
        self.velocity.dot(self.look)
    }

    /// How fast each drawn wheel turns, front then rear, rad/s. 0x414000
    /// turns both by +0x434 over the radius times the frame, and the tyre
    /// (0x518c40) divides by its radius again. None for a tyre in the air.
    pub fn wheel_rates(&self) -> [Option<f32>; 2] {
        let speed = self.forward_speed();
        [0, 1].map(|i| self.down[i].then(|| speed / (WHEEL_RADII[i] * WHEEL_RADII[i])))
    }
}

/// The line's aim from the front (0x40f310), along the heading when the
/// line gives none.
fn path_aim(path: &mut Path, at: Vec3, lane: f32, look: Vec3) -> Vec3 {
    if path.points.is_empty() {
        return level(look);
    }
    path.direction(at, lane)
}

/// The ground under a point: the terrain, or a solid's top face within
/// `above` ft over it and `below` under it, whichever is higher (the
/// bike's collision object, 0x4367f0).
fn surface(ground: &dyn Ground, p: Vec3, above: f32, span: f32) -> (f32, Vec3) {
    let (mut h, mut n) = ground.terrain(p.x, p.z);
    let top = p + Vec3::Y * above;
    if let Some(c) = ground.crossing(top, top - Vec3::Y * span) {
        if c.point.y > h {
            h = c.point.y;
            n = c.normal;
        }
    }
    (h, n.normalize_or(Vec3::Y))
}

/// The flight ahead (0x40f170): a ghost from `at` with `velocity` in
/// 0.1 s steps until the ground is 6 ft over it. The air time, and the
/// ground's normal where it comes down.
pub fn flight(ground: &dyn Ground, at: Vec3, velocity: Vec3) -> (f32, Vec3) {
    let (mut h, mut n) = ground.terrain(at.x, at.z);
    let mut p = at;
    let mut vy = velocity.y;
    let mut t = 0.0;
    let mut steps = 0;
    while h < p.y + 6.0 && steps < 600 {
        p.x += velocity.x * 0.1;
        p.z += velocity.z * 0.1;
        p.y += vy * 0.1 - 0.16;
        vy -= 3.2;
        (h, n) = ground.terrain(p.x, p.z);
        t += 0.1;
        steps += 1;
    }
    (t, n)
}

fn toward(from: f32, to: f32, rate: f32) -> f32 {
    from + (to - from).clamp(-rate, rate)
}

fn level(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z).normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Flat;

    /// Down a straight line on the flat at level 2 it pulls away like the
    /// game's riders (about 49 ft/s after a second, 0x56d05c's middle
    /// class at 1.3), keeps upright and on its line, and never crashes.
    #[test]
    fn pulls_away_down_a_straight() {
        let mut path = Path::new((0..40).map(|i| Vec3::new(0.0, 0.0, i as f32 * 50.0)).collect(), 20.0);
        path.power = POWER[1];
        let mut rider = Rider::new(&Flat, Vec3::ZERO, Vec3::Z, 0.0, engine_class(250));
        let mut lean = 0.0;
        let mut speeds = Vec::new();
        for frame in 0..180 {
            let crash = rider.step(&Flat, &mut path, 1.0 / 35.0, &mut lean, false, &[]);
            assert!(!crash, "crashed at frame {frame}");
            if frame % 35 == 34 {
                speeds.push(rider.speed);
            }
        }
        assert!((35.0..60.0).contains(&speeds[0]), "{speeds:?}");
        assert!(speeds.windows(2).all(|w| w[1] > w[0]), "{speeds:?}");
        assert!(speeds[4] < 100.0, "{speeds:?}");
        assert!(rider.position.x.abs() < 1.0 && rider.up.y > 0.95, "{:?}", rider.position);
    }
}
