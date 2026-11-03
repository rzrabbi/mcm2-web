//! How the game's computer riders follow a line (BikeAI: the path follower
//! 0x40f310).
//!
//! A rider's line is a polyline of up to 200 points with a look-ahead
//! distance (+0x974) and a speed limit (+0x96c). Each frame the rider
//! finds where it is along the line and aims at the point the look-ahead
//! further on, moved sideways by its own lane offset. Before a jump the
//! trajectory planner 0x40e000 ([`Path::plan_jump`]) sets the speed the
//! rider needs to clear it, and points the rider at the landing.

use glam::Vec3;

use crate::Ground;

/// The most points a line holds (the path's array, 0x974 bytes).
pub const MOST_POINTS: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub points: Vec<Vec3>,
    /// How far ahead along the line to aim, ft.
    pub lookahead: f32,
    /// The segment the rider is on and how far along it, 0 to 1.
    pub segment: usize,
    pub t: f32,
    /// The speed limit (+0x96c): the controller sets 1000 each frame and
    /// the jump planner lowers it before a jump that must be cut short.
    pub limit: f32,
    /// The level's power (+0x970), times the engine's pull.
    pub power: f32,
    /// Nationals and Supercross (event 2 or 3): the look-ahead grows by
    /// how far the rider is off the line, so a rider well off it comes
    /// back gently instead of turning square to it (0x40f310).
    pub widen: bool,
}

impl Path {
    pub fn new(points: Vec<Vec3>, lookahead: f32) -> Self {
        let mut points = points;
        points.truncate(MOST_POINTS);
        Path {
            points,
            lookahead,
            segment: 0,
            t: 0.0,
            limit: 1000.0,
            power: 1.5,
            widen: false,
        }
    }

    /// The level direction to steer for from `at`, aiming `offset` ft to
    /// the side of the line (0x40f310). Updates the segment and t.
    pub fn direction(&mut self, at: Vec3, offset: f32) -> Vec3 {
        let p = &self.points;
        let n = p.len();
        if n == 0 {
            return Vec3::Z;
        }
        if n < 2 {
            self.segment = 0;
            self.t = 0.0;
            return flat(p[0] - at);
        }
        // The nearest point (the last one is not counted).
        let (mut nearest, mut best) = (0, f32::MAX);
        for (i, q) in p[..n - 1].iter().enumerate() {
            let d = q.distance(at);
            if d < best {
                best = d;
                nearest = i;
            }
        }
        // On the segment before it or after it, if the rider is beside
        // one. The game compares the signed distance across the segment.
        let (mut segment, mut t) = (nearest, 0.0);
        if nearest > 0 {
            if let Some((ts, s)) = project(p[nearest - 1], p[nearest], at) {
                if (0.0..=1.0).contains(&ts) && s < best {
                    best = s;
                    segment = nearest - 1;
                    t = ts;
                }
            }
        }
        // The game widens by the last distance across it measured, the
        // segment after the nearest point's.
        let mut across = 0.0;
        if nearest < n - 1 {
            if let Some((ts, s)) = project(p[nearest], p[nearest + 1], at) {
                across = s;
                if (0.0..=1.0).contains(&ts) && s < best {
                    segment = nearest;
                    t = ts;
                }
            }
        }
        // Walk the look-ahead along the line.
        let mut ahead = self.lookahead;
        if self.widen {
            ahead += across.abs();
        }
        let len = |i: usize| {
            if i + 1 < n {
                p[i + 1].distance(p[i])
            } else {
                0.0
            }
        };
        let mut left = (1.0 - t) * len(segment);
        while ahead > 0.0 && ahead > left && segment < n - 1 {
            segment += 1;
            t = 0.0;
            ahead -= left;
            left = len(segment);
        }
        let dir = if segment + 1 < n {
            p[segment + 1] - p[segment]
        } else {
            p[segment] - p[segment - 1]
        };
        let side = flat(Vec3::new(dir.z, 0.0, -dir.x));
        let target = if segment < n - 1 {
            if ahead < left {
                t += ahead / left;
            }
            p[segment] + dir * t
        } else {
            t = 0.0;
            p[segment]
        };
        self.segment = segment;
        self.t = t;
        flat(target + side * offset - at)
    }
}

/// What the jump planner found ahead (0x40e000).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jump {
    /// Seconds until the take-off, or 10 when there is none ahead.
    pub takeoff_in: f32,
    /// Where to aim, level, when the take-off is under 0.5 s away: at the
    /// landing.
    pub aim: Option<Vec3>,
    /// The speed to take the jump at, ft/s, when the jump is longer than
    /// 20 ft (the path's speed limit, +0x96c).
    pub speed: Option<f32>,
    /// The velocity to leave the lip with, at that speed.
    pub launch: Option<Vec3>,
}

/// How far ahead the planner looks on the ground, s, and its step (the
/// AI's call in 0x40faa0).
const PLAN_AHEAD: f32 = 0.5;
const PLAN_STEP: f32 = 0.05;
/// A ghost more than this above the ground is in the air, ft.
const PLAN_LIFT: f32 = 0.5;
/// cos 45°: the turn the ghost may make in the air, and the hardest
/// landing it keeps its speed through (0.70755).
const PLAN_COS: f32 = 0.707_55;
/// Gravity the planner uses, ft/s².
const PLAN_GRAVITY: f32 = 32.0;
/// Jumps shorter than this need no speed, ft.
const PLAN_SHORTEST: f32 = 20.0;
/// The slowest speed the planner asks for, and how much it slows the
/// rider at a time, ft/s.
const PLAN_SLOWEST: f32 = 40.0;
const PLAN_EASE: f32 = 20.0;

impl Path {
    /// Rides a ghost of the rider along the line from `at` with
    /// `velocity` for half a second, steering for the look-ahead point that
    /// [`Path::direction`] last found, and on through any jump it reaches,
    /// to find the take-off and the landing (0x40e000).
    ///
    /// The ghost keeps its level speed and steers for the line (moved
    /// `offset` ft across, as in [`Path::direction`]). It follows the
    /// ground until it is more than 0.5 ft above it; in the air it falls at
    /// 32 ft/s² until it meets the ground again. Only a flight that is cut
    /// short, because the line turns more than 45° from the take-off's
    /// heading while the ghost is still in the air or the line ends, sets a
    /// speed, so the rider does not fly past a corner. The speed is
    /// `√(d² · 32 / (2 · (d · sin θ + Δy))) / cos θ`, with `d` the level
    /// distance from take-off to where the flight was cut, `θ` the
    /// take-off's climb and `Δy` that point's height over the take-off (the
    /// game's formula), and at least 40 ft/s. A rider more than 20 ft/s
    /// faster than that is slowed by 20 at a time. Jumps under 20 ft set
    /// no speed.
    ///
    /// The game writes the ghost's place on the line back into the path;
    /// this leaves the path as it is.
    pub fn plan_jump(&self, ground: &dyn Ground, at: Vec3, velocity: Vec3, offset: f32) -> Jump {
        let mut jump = Jump {
            takeoff_in: 10.0,
            aim: None,
            speed: None,
            launch: None,
        };
        let p = &self.points;
        let n = p.len();
        if n == 0 || self.segment >= n {
            return jump;
        }
        // The segment's direction (or the way to the only point).
        let leg = |i: usize, from: Vec3| -> Vec3 {
            let d = if n == 1 {
                p[0] - from
            } else if i + 1 < n {
                p[i + 1] - p[i]
            } else {
                p[i] - p[i - 1]
            };
            Vec3::new(d.x, 0.0, d.z)
        };
        let target = |i: usize, t: f32, d: Vec3| {
            let side = flat_or_zero(Vec3::new(d.z, 0.0, -d.x));
            p[i] + d * t + side * offset
        };
        let steer = |from: Vec3, to: Vec3, speed: f32| flat_or_zero(to - from) * speed;

        let (mut segment, mut t) = (self.segment, self.t);
        let mut d = leg(segment, at);
        let per_ft = |d: Vec3| {
            if d.length() > 0.0 {
                1.0 / d.length()
            } else {
                0.0
            }
        };
        let mut along = per_ft(d);
        let mut ghost = at;
        let level = Vec3::new(velocity.x, 0.0, velocity.z).length();
        let mut v = steer(ghost, target(segment, t, d), level);
        v.y = velocity.y;
        let mut time = 0.0;
        let mut flying = false;
        let mut turned = 1.0;
        // Take-off: where, when, its velocity and level heading.
        let mut takeoff: Option<(Vec3, Vec3, Vec3)> = None;
        let mut landing: Option<Vec3> = None;
        let mut landed = false;
        for _ in 0..2000 {
            if !(time < PLAN_AHEAD || flying) {
                break;
            }
            let speed = v.length();
            let (height, normal) = ground.terrain(ghost.x, ghost.z);
            let normal = normal.normalize_or(Vec3::Y);
            let g;
            if height + PLAN_LIFT < ghost.y {
                if flying {
                    if turned < PLAN_COS {
                        landing = Some(ghost);
                        landed = true;
                        break;
                    }
                } else {
                    let heading = flat_or_zero(Vec3::new(v.x, 0.0, v.z));
                    takeoff = Some((ghost, v, heading));
                    jump.takeoff_in = time;
                }
                flying = true;
                g = -PLAN_GRAVITY;
            } else {
                if flying && jump.takeoff_in < PLAN_AHEAD {
                    landing = Some(ghost);
                }
                ghost.y = height;
                let into = v.dot(normal);
                if into < 0.0 {
                    v -= normal * into;
                    if into >= -PLAN_COS * speed {
                        v = v.normalize_or_zero() * speed;
                    }
                }
                flying = false;
                g = 0.0;
            }
            // A step of the ghost.
            ghost += Vec3::new(v.x, 0.0, v.z) * PLAN_STEP;
            ghost.y += g * PLAN_STEP * PLAN_STEP * 0.5 + v.y * PLAN_STEP;
            v.y += g * PLAN_STEP;
            let level = Vec3::new(v.x, 0.0, v.z).length();
            t += along * level * PLAN_STEP;
            // The game takes a whole 1 off t per segment passed, whatever
            // the segments' lengths.
            while t > 1.0 && segment + 1 < n {
                t -= 1.0;
                segment += 1;
                d = leg(segment, ghost);
                along = per_ft(d);
            }
            if flying && segment + 1 == n {
                landing = Some(ghost);
                landed = true;
                break;
            }
            let aim = steer(ghost, target(segment, t, d), level);
            v.x = aim.x;
            v.z = aim.z;
            turned = match takeoff {
                Some((_, _, heading)) if flying => d.normalize_or_zero().dot(heading),
                _ => 1.0,
            };
            time += PLAN_STEP;
        }

        if let (true, Some((from, launch, _)), Some(to)) = (landed, takeoff, landing) {
            let distance = rough_sqrt((to.x - from.x).powi(2) + (to.z - from.z).powi(2));
            let level = rough_sqrt(launch.x * launch.x + launch.z * launch.z);
            if distance > PLAN_SHORTEST && level > 0.0 {
                let climb = launch.y / launch.length();
                let slant = launch.length() / level;
                let need = rough_sqrt(
                    distance * distance * PLAN_GRAVITY / (2.0 * (climb * distance + (to.y - from.y))),
                ) * slant;
                let have = launch.length();
                let speed = if need >= PLAN_SLOWEST {
                    if have - need > PLAN_EASE {
                        have - PLAN_EASE
                    } else {
                        need
                    }
                } else {
                    // Also when the formula has no answer.
                    PLAN_SLOWEST
                };
                jump.speed = Some(speed);
                jump.launch = Some(launch.normalize_or_zero() * speed);
            }
        }
        if jump.takeoff_in < PLAN_AHEAD {
            if let Some(to) = landing {
                jump.aim = Some(flat_or_zero(to - at));
            }
        }
        jump
    }
}

/// The AI turns at most 45° from its heading towards an aim (0x40faa0):
/// beyond that it steers along the heading turned 45° to the aim's side.
pub fn within_reach(heading: Vec3, aim: Vec3) -> Vec3 {
    let heading = flat(heading);
    let aim = flat(aim);
    if aim.dot(heading) >= PLAN_COS {
        return aim;
    }
    // Right is (z, 0, −x) in the game's left-handed axes.
    let right = Vec3::new(heading.z, 0.0, -heading.x);
    if aim.dot(right) >= 0.0 {
        flat(heading + right)
    } else {
        flat(heading - right)
    }
}

fn flat_or_zero(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z).normalize_or_zero()
}

/// Another rider or a moving object, for [`scan`]: where it is and
/// where it is heading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mover {
    pub at: Vec3,
    pub to: Vec3,
}

/// What the AI's look ahead found in its way (0x416440).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scan {
    /// Something is in the corridor.
    pub blocked: bool,
    /// The nearest wall hit along the corridor, ft (1000 when none).
    pub nearest: f32,
    /// The level-ish direction the obstacles lie in, when any did.
    pub toward: Option<Vec3>,
    /// A change to the rider's velocity to dodge movers near it.
    pub nudge: Vec3,
}

/// The corridor's sideways and vertical offsets: the line itself, 3 ft
/// up, 1.5 ft down and 3 ft to each side (0x416440).
const SCAN_UP: f32 = 3.0;
const SCAN_DOWN: f32 = 1.5;
const SCAN_SIDE: f32 = 3.0;
/// Walls are steeper than 45° (normal y under 0.70755). Hits further
/// than this, ft, are ignored when `near_only` is set.
const SCAN_NEAR: f32 = 10.0;
/// A tree counts a quarter of its direction.
const SCAN_TREE: f32 = 0.25;

/// Looks along the corridor from `from` to `to` (the AI uses its place
/// 3 ft up and where it will be in 0.25 s) for solids, trees and movers
/// within `radius` ft (0x416440; the AI passes 2). `speed` is the
/// rider's speed.
///
/// * A solid near the corridor is probed by five rays (the line, 3 ft up,
///   1.5 ft down, 3 ft left and right). Hits on walls steeper than 45°
///   count, and on the 3-ft-up ray any hit; the nearest gives the
///   direction to the obstacle. Here the rays are sampled against
///   [`Ground::solid`] every foot.
/// * Trees count a quarter of the way to their centres.
/// * A mover whose path crosses the corridor blocks it; one within 10 ft
///   ahead that does not cross pushes the rider's velocity away from it
///   by up to 10 ft/s.
#[allow(clippy::too_many_arguments)]
pub fn scan(
    ground: &dyn Ground,
    from: Vec3,
    to: Vec3,
    radius: f32,
    speed: f32,
    movers: &[Mover],
    trees: &[Vec3],
    near_only: bool,
) -> Scan {
    let along = to - from;
    let right = Vec3::new(along.z, 0.0, -along.x).normalize_or_zero();
    let ahead = along.normalize_or_zero();
    let mut steer = ahead * speed;
    let start = steer;
    let mut blocked = false;
    let mut nearest = 1000.0f32;
    let mut hit_at = None;
    let mut toward = Vec3::ZERO;

    // Solids, along the five rays.
    let rays = [
        (Vec3::Y * SCAN_UP, true),
        (Vec3::ZERO, false),
        (-Vec3::Y * SCAN_DOWN, false),
        (-right * SCAN_SIDE, false),
        (right * SCAN_SIDE, false),
    ];
    let length = along.length();
    let near_line = (0..=length.ceil() as usize).any(|i| {
        let t = (i as f32 / length.max(1.0)).min(1.0);
        ground.solid(from + along * t, radius).is_some()
    });
    if near_line {
        for (offset, any) in rays {
            for i in 0..=length.ceil() as usize {
                let t = (i as f32 / length.max(1.0)).min(1.0);
                let p = from + offset + along * t;
                let Some(hit) = ground.solid(p, 0.5) else {
                    continue;
                };
                if !any && hit.normal.y >= PLAN_COS {
                    continue;
                }
                let d = p.distance(from);
                if !(near_only && d > SCAN_NEAR) && d < nearest {
                    nearest = d;
                    hit_at = Some(p);
                }
                break;
            }
        }
        blocked = true;
        if let Some(p) = hit_at {
            if nearest < SCAN_NEAR || !near_only {
                toward += p - from;
            }
        }
    }

    // Trees.
    for &tree in trees {
        if distance_to_segment(tree, from, to) < radius {
            blocked = true;
            let d = match hit_at {
                Some(p) if nearest < SCAN_NEAR => p - from,
                _ => tree - from,
            };
            toward += d * SCAN_TREE;
        }
    }

    // Movers.
    for m in movers {
        let rel = m.at - from;
        let dir = rel.normalize_or_zero();
        match crossing(from, to, m.at, m.to) {
            Some(_) => {
                blocked = true;
                // The game measures the mover's heading point against the
                // dodge vector itself, as here.
                let k = (m.to - steer).dot(dir);
                if k < 0.0 {
                    steer += dir * k;
                }
            }
            None => {
                if rel.length_squared() < 100.0 && steer.dot(dir) > 0.0 {
                    steer -= dir * speed.min(10.0);
                }
            }
        }
    }

    Scan {
        blocked,
        nearest,
        toward: (toward.x != 0.0 || toward.z != 0.0).then(|| toward.normalize()),
        nudge: steer - start,
    }
}

/// The AI's steering after a [`scan`] (0x40faa0): the part of `direction`
/// heading into the obstacles is taken out, level.
pub fn avoid(direction: Vec3, scan: &Scan) -> Vec3 {
    let d = flat(direction);
    match scan.toward {
        Some(o) if scan.blocked => {
            let o = flat(o);
            let k = d.dot(o);
            if k > 0.0 {
                flat(d - o * k)
            } else {
                d
            }
        }
        _ => d,
    }
}

/// How much the AI changes the front wheel's velocity this step to ride
/// along `direction` (0x40faa0, after [`avoid`] and [`within_reach`]):
///
/// * in the Baja and Enduro (`gates`) the direction first leans back
///   toward the current heading, `direction + 5 × heading`;
/// * the wanted velocity keeps the level speed along that direction, plus
///   the scan's nudge away from movers;
/// * the level change to it is capped at 0.2 × the speed.
///
/// The game applies the change as an acceleration of `change / dt`, so it
/// lands in full within the step.
pub fn steer(velocity: Vec3, direction: Vec3, nudge: Vec3, gates: bool) -> Vec3 {
    let mut d = flat_or_zero(direction);
    if gates {
        d = flat_or_zero(d + flat_or_zero(velocity) * 5.0);
    }
    let level = Vec3::new(velocity.x, 0.0, velocity.z).length();
    let want = d * level + nudge;
    let change = Vec3::new(want.x - velocity.x, 0.0, want.z - velocity.z);
    let cap = 0.2 * velocity.length();
    if change.length() > cap {
        change.normalize_or_zero() * cap
    } else {
        change
    }
}

/// Where two level segments cross, as the fractions along each (0x47c740).
fn crossing(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> Option<(f32, f32)> {
    let r = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
    let s = Vec3::new(d.x - c.x, 0.0, d.z - c.z);
    let det = r.x * s.z - r.z * s.x;
    if det == 0.0 {
        return None;
    }
    let q = Vec3::new(c.x - a.x, 0.0, c.z - a.z);
    let t = (q.x * s.z - q.z * s.x) / det;
    let u = (q.x * r.z - q.z * r.x) / det;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

fn distance_to_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// Where `at` lies against the segment `a` to `b`, in the level plane:
/// how far along it (0 to 1 is on it) and how far across (signed, along
/// (dz, −dx)) (0x47c740).
fn project(a: Vec3, b: Vec3, at: Vec3) -> Option<(f32, f32)> {
    let d = b - a;
    let n = Vec3::new(d.z, 0.0, -d.x).normalize_or_zero();
    let det = d.z * n.x - d.x * n.z;
    if det == 0.0 {
        return None;
    }
    let t = ((a.x - at.x) * n.z + (at.z - a.z) * n.x) / det;
    let s = if n.x != 0.0 {
        (t * d.x + a.x - at.x) / n.x
    } else {
        (t * d.z + a.z - at.z) / n.z
    };
    Some((t, s))
}

fn flat(v: Vec3) -> Vec3 {
    let v = Vec3::new(v.x, 0.0, v.z);
    if v == Vec3::ZERO {
        Vec3::Z
    } else {
        v.normalize()
    }
}

/// An Enduro or Baja checkpoint race's gates, from a `.wpt` file
/// (`[Waypoints]`, `NumWaypoints`, `PointNX`, `PointNZ`, `PointNLookX`,
/// `PointNLookZ`): each gate's position and facing, level.
pub fn waypoints(text: &str) -> Vec<(Vec3, Vec3)> {
    let mut values = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.split(';').next().unwrap_or("").trim();
        if let Some((k, v)) = line.split_once('=') {
            if let Ok(v) = v.trim().parse::<f32>() {
                values.insert(k.trim().to_ascii_lowercase(), v);
            }
        }
    }
    let count = values.get("numwaypoints").copied().unwrap_or(0.0) as usize;
    (1..=count)
        .map_while(|i| {
            let get = |k: &str| values.get(&format!("point{i}{k}")).copied();
            Some((
                Vec3::new(get("x")?, 0.0, get("z")?),
                Vec3::new(get("lookx")?, 0.0, get("lookz")?),
            ))
        })
        .collect()
}

/// A computer rider in the Stunt Quarry (events 0 and 4), where there is
/// no course to follow (0x415170's last branch). The rider rides flat out
/// toward a point, and once it is within 50 ft (level) of it picks another.
/// It doesn't look for jumps; it tricks off whatever it happens to launch
/// from (see [`crate::stunt::ai_trick`], with the chance always race+0xa4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wander {
    /// Where the rider is heading (bike +0x818), at height 0.
    pub target: Vec3,
}

/// A new target is picked within this level distance of the old one, ft.
pub const WANDER_REACHED: f32 = 50.0;

/// The wander targets' centre and radius, in terrain units (times the
/// terrain's horizontal scale, Terrain +0x40).
pub const WANDER_CENTRE: f32 = 666.66;
pub const WANDER_RADIUS: f32 = 444.0;

/// The speed the rider aims for while wandering (+0x11a4): with 1000 ft/s
/// the simple model never lets off the gas.
pub const WANDER_SPEED: f32 = 1000.0;

impl Wander {
    /// Starts at the origin, as the game's zeroed bike does, so the first
    /// call picks a real target.
    pub fn new() -> Self {
        Wander { target: Vec3::ZERO }
    }

    /// The point to steer for this frame (the solver's single target,
    /// +0x844). `scale` is the terrain's horizontal scale and `rand`
    /// returns 0..1 (the game's `rand() / 32768`).
    ///
    /// The game takes the direction from two such numbers, `(u, 0, w)`
    /// normalised, and never makes them negative, so every target lies on
    /// the quarter circle on the +x, +z side of the centre. Kept as the
    /// game has it.
    pub fn aim(&mut self, at: Vec3, scale: f32, mut rand: impl FnMut() -> f32) -> Vec3 {
        let d = self.target - at;
        if d.x * d.x + d.z * d.z < WANDER_REACHED * WANDER_REACHED {
            let (u, w) = (rand(), rand());
            let len = (u * u + w * w).sqrt();
            let dir = if len > 0.0 {
                Vec3::new(u / len, 0.0, w / len)
            } else {
                Vec3::ZERO
            };
            self.target = Vec3::new(
                scale * WANDER_CENTRE + dir.x * WANDER_RADIUS * scale,
                0.0,
                scale * WANDER_CENTRE + dir.z * WANDER_RADIUS * scale,
            );
        }
        self.target
    }
}

impl Default for Wander {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aims_ahead_along_the_line() {
        let mut path = Path::new(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 100.0),
                Vec3::new(100.0, 0.0, 100.0),
            ],
            30.0,
        );
        // On the first leg, aiming straight on.
        let d = path.direction(Vec3::new(0.0, 0.0, 10.0), 0.0);
        assert!(d.abs_diff_eq(Vec3::Z, 1e-4), "{d:?}");
        assert_eq!(path.segment, 0);
        // Near the corner the aim swings onto the second leg.
        let d = path.direction(Vec3::new(0.0, 0.0, 90.0), 0.0);
        assert!(d.x > 0.5, "{d:?}");
        assert_eq!(path.segment, 1);
        // An offset moves the aim across the line.
        let d = path.direction(Vec3::new(0.0, 0.0, 10.0), 10.0);
        assert!(d.x.abs() > 0.1, "{d:?}");
    }

    /// Flat, a kicker from z 20 to 30 up to 5 ft, then flat again.
    struct Kicker;
    impl Ground for Kicker {
        fn terrain(&self, _x: f32, z: f32) -> (f32, Vec3) {
            if (20.0..30.0).contains(&z) {
                ((z - 20.0) * 0.5, Vec3::new(0.0, 1.0, -0.5).normalize())
            } else {
                (0.0, Vec3::Y)
            }
        }
    }

    #[test]
    fn plans_the_jump_ahead() {
        // A kicker, then a right-angle turn the rider would fly past.
        let corner = vec![
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 90.0),
            Vec3::new(100.0, 0.0, 90.0),
        ];
        let mut path = Path::new(corner, 30.0);
        let at = Vec3::new(0.0, 0.0, 16.0);
        let fast = Vec3::new(0.0, 0.0, 40.0);
        // The planner starts from the look-ahead point the follower found.
        path.direction(at, 0.0);
        let jump = path.plan_jump(&Kicker, at, fast, 0.0);
        assert!(jump.takeoff_in < 0.5, "{jump:?}");
        let aim = jump.aim.expect("an aim at the landing");
        assert!(aim.z > 0.9, "{aim:?}");
        let speed = jump.speed.expect("a speed for the jump");
        assert!(speed >= 40.0, "{speed}");
        assert!(jump.launch.unwrap().y > 0.0);
        // Down a straight the landing needs no speed, only the aim.
        let mut straight = Path::new(vec![Vec3::ZERO, Vec3::new(0.0, 0.0, 400.0)], 30.0);
        straight.direction(at, 0.0);
        let jump = straight.plan_jump(&Kicker, at, fast, 0.0);
        assert!(jump.aim.is_some() && jump.speed.is_none(), "{jump:?}");
        // Nothing to jump on the flat.
        let none = straight.plan_jump(&crate::Flat, Vec3::ZERO, fast, 0.0);
        assert_eq!(none.takeoff_in, 10.0);
        assert_eq!(none.speed, None);
    }

    #[test]
    fn widens_the_look_ahead_by_the_distance_off_the_line() {
        let line = || Path::new((0..10).map(|i| Vec3::new(0.0, 0.0, i as f32 * 20.0)).collect(), 2.0);
        let off = Vec3::new(-12.0, 0.0, 10.0);
        let sharp = line().direction(off, 0.0);
        let mut wide = line();
        wide.widen = true;
        let gentle = wide.direction(off, 0.0);
        assert!(sharp.x > 0.9, "{sharp}");
        // 12 ft off with 2 + 12 ft of look-ahead: about 40° to the line.
        assert!((gentle.x - 0.65).abs() < 0.02, "{gentle}");
    }

    #[test]
    fn steers_a_fifth_of_the_speed_at_most() {
        let v = Vec3::new(0.0, 0.0, 50.0);
        // A right angle turn: capped at 10 ft/s this step.
        let c = steer(v, Vec3::X, Vec3::ZERO, false);
        assert!((c.length() - 10.0).abs() < 1e-4);
        // A small correction lands in full.
        let d = Vec3::new(0.05, 0.0, 1.0).normalize();
        let c = steer(v, d, Vec3::ZERO, false);
        assert!(((v + c) - d * 50.0).length() < 1e-3);
        // In a gate race the heading damps the turn.
        let g = steer(v, d, Vec3::ZERO, true);
        assert!(g.length() < c.length());
    }

    #[test]
    fn turns_at_most_45_degrees() {
        let d = within_reach(Vec3::Z, Vec3::X);
        assert!(
            (d.dot(Vec3::Z) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4,
            "{d:?}"
        );
        assert!(d.x > 0.0);
        assert_eq!(within_reach(Vec3::Z, Vec3::Z), Vec3::Z);
    }

    /// A wall across z = 20.
    struct Wall;
    impl Ground for Wall {
        fn terrain(&self, _x: f32, _z: f32) -> (f32, Vec3) {
            (0.0, Vec3::Y)
        }
        fn solid(&self, centre: Vec3, radius: f32) -> Option<crate::collision::Hit> {
            let depth = radius - (20.0 - centre.z);
            (depth > 0.0).then(|| crate::collision::Hit {
                normal: -Vec3::Z,
                depth,
                point: Vec3::new(centre.x, centre.y, 20.0),
            })
        }
    }

    #[test]
    fn sees_a_wall_and_a_rider_ahead() {
        let from = Vec3::new(0.0, 3.0, 13.0);
        let to = Vec3::new(5.0, 3.0, 28.0);
        let s = scan(&Wall, from, to, 2.0, 40.0, &[], &[], true);
        assert!(s.blocked);
        assert!(s.nearest < 15.0, "{s:?}");
        let o = s.toward.unwrap();
        assert!(o.z > 0.5, "{o:?}");
        // Steering into the wall is turned along it.
        let d = avoid(Vec3::new(1.0, 0.0, 3.0), &s);
        assert!(d.z.abs() < 0.5 && d.x > 0.5, "{d:?}");
        // A rider crossing the corridor blocks it.
        let rider = Mover {
            at: Vec3::new(-5.0, 3.0, 15.0),
            to: Vec3::new(10.0, 3.0, 15.0),
        };
        let s = scan(&crate::Flat, from, to, 2.0, 40.0, &[rider], &[], true);
        assert!(s.blocked && s.toward.is_none());
        let clear = scan(&crate::Flat, from, to, 2.0, 40.0, &[], &[], true);
        assert!(!clear.blocked);
        assert_eq!(clear.nudge, Vec3::ZERO);
    }

    #[test]
    fn reads_waypoints() {
        let text = "[Waypoints]\r\nNumWaypoints=2\r\nPoint1X=3324.61\r\nPoint1Z=4418.56\r\n\
            Point1LookX=-0.30\r\nPoint1LookZ=-0.95\r\nPoint2X=1\r\nPoint2Z=2\r\nPoint2LookX=0\r\n\
            Point2LookZ=1\r\n";
        let w = waypoints(text);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].0, Vec3::new(3324.61, 0.0, 4418.56));
        assert_eq!(w[1].1, Vec3::Z);
    }

    #[test]
    fn wanders_the_quarry() {
        let mut w = Wander::new();
        let mut r = [0.6_f32, 0.8].into_iter().cycle();
        let t = w.aim(Vec3::ZERO, 1.0, || r.next().unwrap());
        assert!((t - Vec3::new(666.66 + 266.4, 0.0, 666.66 + 355.2)).length() < 0.01);
        // Far from it, the rider keeps the target.
        assert_eq!(w.aim(Vec3::new(500.0, 30.0, 500.0), 1.0, || 0.0), t);
        // Within 50 ft it picks again.
        let again = w.aim(t + Vec3::new(10.0, 0.0, 10.0), 1.0, || 1.0);
        assert!((again.x - (666.66 + 444.0 * std::f32::consts::FRAC_1_SQRT_2)).abs() < 0.1);
    }
}

/// The game's quick square root (0x461ac0): the root's mantissa from a
/// table of 256, so only its top 7 bits are kept and it comes out up to
/// 0.8% short. The computer riders steer with it, which costs them speed.
pub fn rough_sqrt(x: f32) -> f32 {
    if x == 0.0 {
        return 0.0;
    }
    let bits = x.to_bits();
    let e = (bits >> 23) as i32 - 127;
    let mut m = bits & 0x7f_ffff;
    if e & 1 != 0 {
        m |= 0x80_0000;
    }
    let i = m >> 16;
    // The table (0x461a50): the roots of 1 + i/128 and 2 + i/64.
    let root = if i < 128 {
        f32::from_bits((0x3f80 | i) << 16).sqrt()
    } else {
        f32::from_bits((0x4000 | (i - 128)) << 16).sqrt()
    };
    let exponent = (((e as i16) >> 1) + 127) as u32;
    f32::from_bits((root.to_bits() & 0x7f_0000) | (exponent << 23))
}

#[cfg(test)]
mod rough_sqrt_tests {
    use super::rough_sqrt;

    #[test]
    fn short_by_at_most_a_128th() {
        for x in [0.5f32, 1.0, 2.0, 3.0, 100.0, 6400.0, 8100.0, 12345.6] {
            let (r, s) = (rough_sqrt(x), x.sqrt());
            assert!(r <= s && r > s * (1.0 - 1.0 / 128.0), "{x}: {r} vs {s}");
        }
        assert_eq!(rough_sqrt(4.0), 2.0);
        assert_eq!(rough_sqrt(0.0), 0.0);
    }
}
