//! A race: the riders on the start line, the computer riders following the
//! course from the track's `.tdf`, the gate drop, laps and positions.

use crate::ground::TrackGround;
use crate::ride::{Chase, Controls};
use glam::{Mat4, Vec3};
use mm2_formats::tdf::{Line, TrackDef};
use mm2_physics::race::{self, Timing};
use mm2_physics::spec::Spec;
use mm2_physics::stunt::{self, Stunt};

/// The player's bike's collision points, once `RES/BIKE.RES` is read.
static HULL: std::sync::OnceLock<std::sync::Arc<mm2_physics::bike::Hull>> =
    std::sync::OnceLock::new();

pub fn set_hull(hull: mm2_physics::bike::Hull) {
    let _ = HULL.set(std::sync::Arc::new(hull));
}

/// The player's bike's cross-up motion, frame by frame, once
/// `RES/BIKE.RES` is read.
static CROSS_UP: std::sync::OnceLock<std::sync::Arc<Vec<glam::Quat>>> = std::sync::OnceLock::new();

pub fn set_cross_up(frames: Vec<glam::Quat>) {
    let _ = CROSS_UP.set(std::sync::Arc::new(frames));
}

/// How far clear both tyres must be to start taunting or a trick, ft
/// (0x48f230's −2.5).
const TAUNT_CLEAR: f32 = -2.5;

/// A computer rider passes a gate anywhere within this many times its
/// width (our pick: their line is laid from the `.tdf`, not through the
/// gates, so they may run wide of an arch).
const COMPUTER_GATE: f32 = 2.0;

/// Course points in a computer rider's line at a time.
const LINE_POINTS: usize = 14;

/// Seconds from the start until the gate drops (0x421f90's 5.2 s; 8.2 s
/// when the 30-second man is out, [`crate::finish::MAN_LEAD`]).
pub const START_LEAD: f32 = 5.2;
/// Over this long off the course in a race, a bike is put back on it
/// (0x492ac0, 6.0 at 0x555c2c).
pub const OFF_COURSE_RESET: f32 = 6.0;

/// The computer riders' frame in live play, s: the original's AI tape
/// (`ai_race4.csv`) runs at 0.028 s a frame, and the port's riders were
/// matched to it there. Run each screen frame at 120 fps they crash a
/// third more often (whole races, 38 → 49 crashes; the tape has 31).
pub const AI_FRAME: f32 = 0.028;
pub const DEFAULT_LAPS: u32 = 3;
/// Practice: laps without end.
pub const ENDLESS: u32 = 99;

/// The course as one list of points to ride through, in order.
#[derive(Debug, Clone)]
pub struct Course {
    points: Vec<Vec3>,
    /// Half the course's width at each point, ft.
    widths: Vec<f32>,
    /// Each point's cross-section ends.
    edges: Vec<[Vec3; 2]>,
    /// The point after each one; the last of a node leads to the node it
    /// links to.
    next: Vec<usize>,
    /// Points in one lap.
    lap: usize,
    finish: Line,
    /// Where every rider's confirmed place starts: the start line's
    /// place, 0.1 on (0x4eb170).
    start: Option<Place>,
}

impl Course {
    /// Each stretch of the course's two sides, as the radar draws them.
    pub fn side_segments(&self) -> impl Iterator<Item = (Vec3, Vec3)> + '_ {
        self.next
            .iter()
            .enumerate()
            .flat_map(move |(i, &n)| (0..2).map(move |k| (self.edges[i][k], self.edges[n][k])))
    }

    pub fn new(def: &TrackDef) -> Option<Self> {
        let mut first = Vec::new();
        let mut points = Vec::new();
        let mut widths = Vec::new();
        let mut edges = Vec::new();
        for node in &def.nodes {
            first.push(points.len());
            edges.extend(node.sections.iter().map(|s| s.edges.map(Vec3::from)));
            points.extend(node.sections.iter().map(|s| Vec3::from(s.center)));
            widths.extend(
                node.sections
                    .iter()
                    .map(|s| flat(Vec3::from(s.edges[0]) - Vec3::from(s.edges[1])).length() / 2.0),
            );
        }
        if points.len() < 3 {
            return None;
        }
        let mut next = Vec::with_capacity(points.len());
        for (n, node) in def.nodes.iter().enumerate() {
            let count = node.sections.len();
            for i in 0..count {
                let here = first[n] + i;
                next.push(if i + 1 < count {
                    here + 1
                } else {
                    // On to the linked node, or round this one again.
                    let to = node.links.first().map_or(n, |&l| l as usize);
                    first
                        .get(to)
                        .copied()
                        .filter(|&f| f < points.len())
                        .unwrap_or(first[n])
                });
            }
        }
        // Skip a closing point that repeats the first one.
        for i in 0..points.len() {
            let j = next[i];
            if j != i && (points[i] - points[j]).length() < 0.5 {
                next[i] = next[j];
            }
        }
        // Walk well into the loop, then count its length.
        let mut at = 0;
        for _ in 0..points.len() {
            at = next[at];
        }
        let (mut lap, mut walk) = (1, next[at]);
        while walk != at && lap <= points.len() {
            walk = next[walk];
            lap += 1;
        }
        let finish = def.finish.or(def.start_line())?;
        let mut course = Course {
            points,
            widths,
            edges,
            next,
            lap,
            finish,
            start: None,
        };
        if let Some(line) = def.start_line() {
            let way = Vec3::from(line.direction);
            course.start = course
                .places(Vec3::from(line.position))
                .into_iter()
                .max_by(|a, b| course.way(*a).dot(way).total_cmp(&course.way(*b).dot(way)))
                .map(|p| Place {
                    leg: p.leg,
                    t: (p.t + 0.1).min(1.0),
                });
        }
        Some(course)
    }

    /// Where `at` stands on the leg into point `to`, and the way along it,
    /// when it is within the course's sides there (the game's confirmed
    /// course place, 0x51d700, 0x51d7b0).
    fn place(&self, at: Vec3, to: usize) -> Option<(Vec3, Vec3)> {
        let from = self.previous(to);
        let (a, b) = (flat(self.points[from]), flat(self.points[to]));
        let leg = b - a;
        let length = leg.length();
        if length < 1e-3 {
            return None;
        }
        let dir = leg / length;
        let t = ((flat(at) - a).dot(dir) / length).clamp(0.0, 1.0);
        let on = a + leg * t;
        let half = self.widths[from] + (self.widths[to] - self.widths[from]) * t;
        ((flat(at) - on).length() <= half).then_some((on, dir))
    }

    /// The point before `i` along the course.
    fn previous(&self, i: usize) -> usize {
        self.next.iter().position(|&n| n == i).unwrap_or(i)
    }

    /// The place on the course's centre line nearest `at`, on the level
    /// (0x51c320).
    fn nearest_place(&self, at: Vec3) -> Option<Place> {
        let p = flat(at);
        let mut best: Option<(Place, f32)> = None;
        for leg in 0..self.points.len() {
            let to = self.next[leg];
            if to == leg {
                continue;
            }
            let (a, b) = (flat(self.points[leg]), flat(self.points[to]));
            let d = b - a;
            let l2 = d.length_squared();
            let t = if l2 == 0.0 { 0.0 } else { ((p - a).dot(d) / l2).clamp(0.0, 1.0) };
            let gap = (a + d * t - p).length_squared();
            if best.is_none_or(|(_, g)| gap < g) {
                best = Some((Place { leg, t }, gap));
            }
        }
        best.map(|(p, _)| p)
    }

    /// The point nearest `at` among the first lap's points.
    fn nearest(&self, at: Vec3) -> usize {
        (0..self.points.len())
            .min_by(|&a, &b| {
                flat(self.points[a] - at)
                    .length()
                    .total_cmp(&flat(self.points[b] - at).length())
            })
            .unwrap_or(0)
    }

    /// The finish line as the game tests it (0x47c5b0): no height limit,
    /// as the port has no line height to go by.
    fn finish_line(&self) -> race::Line {
        let f = &self.finish;
        race::Line {
            centre: Vec3::from(f.position),
            forward: Vec3::from(f.direction),
            half_width: Vec3::from(f.across),
            height: f32::INFINITY,
        }
    }

    /// How far the finish line is ahead of `at` along the course, and how
    /// far `at` is past it (0x51d420 both ways), ft.
    fn to_finish(&self, at: Vec3) -> (f32, f32) {
        let finish = Vec3::from(self.finish.position);
        let (here, line) = (self.nearest(at), self.nearest(finish));
        let along = |from: usize, to: usize| {
            let (mut i, mut length) = (from, 0.0);
            for _ in 0..self.points.len() {
                if i == to {
                    break;
                }
                let j = self.next[i];
                length += flat(self.points[j] - self.points[i]).length();
                i = j;
            }
            length
        };
        (along(here, line), along(line, here))
    }

    /// The point `distance` along the course from `from`, going through
    /// point `to`.
    fn ahead(&self, from: Vec3, mut to: usize, mut distance: f32) -> Vec3 {
        let mut here = flat(from);
        for _ in 0..self.points.len() {
            let p = flat(self.points[to]);
            let leg = (p - here).length();
            if leg >= distance {
                return here + (p - here) * (distance / leg.max(1e-3));
            }
            distance -= leg;
            here = p;
            to = self.next[to];
        }
        here
    }
}

/// A place on the course graph: a leg (from a point to the next) and how
/// far along it, 0 to 1 (the game's node, segment and t).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    leg: usize,
    t: f32,
}

impl Course {
    fn leg_length(&self, leg: usize) -> f32 {
        (self.points[self.next[leg]] - self.points[leg]).length()
    }

    /// The point at `p` (0x51d700).
    fn point(&self, p: Place) -> Vec3 {
        self.points[p.leg].lerp(self.points[self.next[p.leg]], p.t.clamp(0.0, 1.0))
    }

    /// The level way along the leg at `p` (0x51d7b0).
    fn way(&self, p: Place) -> Vec3 {
        flat(self.points[self.next[p.leg]] - self.points[p.leg]).normalize_or(Vec3::Z)
    }

    /// The place `distance` ft on from `p` along the course (0x51d520).
    fn walk(&self, p: Place, mut distance: f32) -> Place {
        let (mut leg, mut t) = (p.leg, p.t);
        for _ in 0..=self.points.len() {
            let length = self.leg_length(leg);
            let left = (1.0 - t) * length;
            if distance <= left {
                let t = if length > 0.0 { t + distance / length } else { 1.0 };
                return Place { leg, t };
            }
            distance -= left;
            leg = self.next[leg];
            t = 0.0;
        }
        Place { leg, t }
    }

    /// How far on along the course `to` is from `from`, when it is no
    /// more than `most` ft on (0x51d420).
    fn along(&self, from: Place, to: Place, most: f32) -> Option<f32> {
        if from.leg == to.leg && from.t <= to.t {
            return Some((to.t - from.t) * self.leg_length(from.leg));
        }
        let mut d = (1.0 - from.t) * self.leg_length(from.leg);
        let mut leg = self.next[from.leg];
        for _ in 0..self.points.len() {
            if d > most {
                return None;
            }
            if leg == to.leg {
                let d = d + to.t * self.leg_length(leg);
                return (d <= most).then_some(d);
            }
            d += self.leg_length(leg);
            leg = self.next[leg];
        }
        None
    }

    /// The places whose stretch of course `at` stands on (0x51c9c0): each
    /// pair of cross-sections in a row bounds a stretch, its sides from
    /// edge to edge and its ends the sections' lines through their centres
    /// (0x51c570), so the outside of a bend has no gaps between stretches.
    /// The place is along the centre line, clamped to the stretch.
    fn places(&self, at: Vec3) -> Vec<Place> {
        let p = flat(at);
        let mut found = Vec::new();
        for leg in 0..self.points.len() {
            let to = self.next[leg];
            if to == leg {
                continue;
            }
            let (a, b) = (self.points[leg], self.points[to]);
            let ([a0, a1], [b0, b1]) = (self.edges[leg], self.edges[to]);
            if !inside(p, &[a0, b0, b, b1, a1, a]) {
                continue;
            }
            let d = flat(b - a);
            let l2 = d.length_squared();
            let t = if l2 == 0.0 { 0.0 } else { ((p - flat(a)).dot(d) / l2).clamp(0.0, 1.0) };
            found.push(Place { leg, t });
        }
        found
    }

    /// The place under `probe` where the course runs most the way of
    /// `heading`.
    fn facing(&self, probe: Vec3, heading: Vec3) -> Option<Place> {
        self.places(probe)
            .into_iter()
            .max_by(|a, b| self.way(*a).dot(heading).total_cmp(&self.way(*b).dot(heading)))
    }

    /// The race tracker's step for one rider (0x4eb170, 0x51c9c0 with
    /// 100 ft ahead and 50 ft back, 0x578ac8 and 0x578acc): where the probe is on the course near the
    /// confirmed place, and the confirmed place moved on to it when it is
    /// ahead by under 100 ft. None when the probe is off the course.
    fn follow(&self, probe: Vec3, heading: Vec3, confirmed: &mut Option<Place>) -> Option<Place> {
        let found = self.places(probe);
        // The game starts every rider from the start line's place, so a
        // rider on the gate behind it aims down the course from the line.
        // (A race picked up part way round starts from where it is.)
        if confirmed.is_none() {
            *confirmed = self.start.filter(|&s| {
                found.iter().any(|&p| {
                    self.along(s, p, 100.0).is_some() || self.along(p, s, 50.0).is_some()
                })
            });
        }
        let Some(c) = *confirmed else {
            // No start line on the course: where the course runs the way
            // the bike faces.
            let first = self.facing(probe, heading);
            *confirmed = first;
            return first;
        };
        let mut best: Option<(Place, f32)> = None;
        for p in found {
            let score = match (self.along(c, p, 100.0), self.along(p, c, 50.0)) {
                (Some(f), _) => f,
                (None, Some(b)) => -b,
                (None, None) => continue,
            };
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((p, score));
            }
        }
        let (p, score) = best?;
        if score > 0.0 && score < 100.0 {
            *confirmed = Some(p);
        }
        Some(p)
    }
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// Whether `p` is inside the polygon `corners` on the ground (x, z), by
/// counting the sides a line from it crosses (0x51c570).
fn inside(p: Vec3, corners: &[Vec3]) -> bool {
    let mut odd = false;
    for (i, a) in corners.iter().enumerate() {
        let b = corners[(i + 1) % corners.len()];
        if (a.z <= p.z) != (b.z <= p.z) {
            let x = a.x + (p.z - a.z) * (b.x - a.x) / (b.z - a.z);
            if p.x < x {
                odd = !odd;
            }
        }
    }
    odd
}

/// A bike: the player's rides the game's full model, the computer riders'
/// the simpler one the game gives them.
#[derive(Debug, Clone)]
pub enum Bike {
    Player(Box<mm2_physics::Bike>),
    Computer(Box<Computer>),
    /// Played from a tape.
    Taped(Box<crate::replay::Taped>),
}

impl Bike {
    /// The model's origin, on the ground between the tyres.
    pub fn position(&self) -> Vec3 {
        match self {
            Bike::Player(b) => b.drawn_place().w_axis.truncate(),
            Bike::Computer(b) => match &b.wreck {
                Some(w) => w.drawn_place().w_axis.truncate(),
                None => b.rider.position,
            },
            Bike::Taped(b) => b.place().w_axis.truncate(),
        }
    }

    /// Along the bike, level.
    pub fn forward(&self) -> Vec3 {
        let f = match self {
            Bike::Player(b) => b.forward(),
            Bike::Computer(b) => match &b.wreck {
                Some(w) => w.forward(),
                None => b.rider.look,
            },
            Bike::Taped(b) => b.forward(),
        };
        Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::Z)
    }

    /// Speed along the bike, ft/s.
    pub fn speed(&self) -> f32 {
        match self {
            Bike::Player(b) => b.speed(),
            Bike::Computer(b) => match &b.wreck {
                Some(w) => w.speed(),
                None => b.rider.forward_speed(),
            },
            Bike::Taped(b) => b.velocity.dot(b.forward()),
        }
    }

    /// How fast each drawn wheel turns, front then rear, rad/s; None for
    /// a wheel off the ground, as a crashed bike's are.
    pub fn wheel_rates(&self) -> [Option<f32>; 2] {
        match self {
            _ if self.crashed() => [None; 2],
            Bike::Player(b) => b.wheel_rates(),
            Bike::Computer(b) => b.rider.wheel_rates(),
            Bike::Taped(b) => {
                let rate = b.velocity.dot(b.forward()) / 1.16;
                [(!b.airborne).then_some(rate); 2]
            }
        }
    }

    /// Where the rear tyre meets the ground and how much it spins, 0 to 1.
    /// The computer's bikes have no tyre model: theirs spin below the
    /// wheelspin speed (45 ft/s).
    pub fn rear_contact(&self) -> Option<(Vec3, f32)> {
        match self {
            Bike::Player(b) => b.rear_contact(),
            _ if self.airborne() || self.crashed() => None,
            _ => {
                let spin = if self.speed().abs() < 45.0 { 0.4 } else { 0.0 };
                Some((self.position() - self.forward() * 2.5, spin))
            }
        }
    }

    pub fn airborne(&self) -> bool {
        match self {
            Bike::Player(b) => b.airborne,
            Bike::Computer(b) => match &b.wreck {
                Some(w) => w.airborne,
                None => b.rider.airborne,
            },
            Bike::Taped(b) => b.airborne,
        }
    }

    /// Whether the crash is a throw back off the world's edge (0xc).
    pub fn launched(&self) -> bool {
        matches!(self, Bike::Player(b) if matches!(b.crash, Some((mm2_physics::Crash::Edge, _))))
    }

    pub fn crashed(&self) -> bool {
        match self {
            Bike::Player(b) => b.crash.is_some(),
            Bike::Computer(b) => b.wreck.is_some() || b.thrown,
            Bike::Taped(b) => b.crashed,
        }
    }

    /// Whether a crashed bike's thrown rider touches something this step
    /// (the Wrecker's +0xb0), for the rider's grunts. The computer's
    /// riders have no thrown body; theirs touch while the bike is down.
    pub fn rider_touching(&self) -> bool {
        match self {
            Bike::Player(b) => b.wreck.as_ref().is_some_and(|w| w.touching),
            _ => self.crashed() && !self.airborne(),
        }
    }

    /// The thrown rider in a crash: where to draw them and their motion.
    pub fn wreck(&self) -> Option<(Mat4, &'static str)> {
        match self {
            Bike::Player(b) => b.wreck.as_ref().map(|w| (w.place(), w.motion)),
            Bike::Computer(b) => b.wreck.as_ref()?.wreck.as_ref().map(|w| (w.place(), w.motion)),
            Bike::Taped(_) => None,
        }
    }

    /// Where the thrown rider's motion puts the pelvis (model space) and
    /// the body parts' corners (the pelvis's space) now.
    pub fn set_wreck_pose(&mut self, pelvis: Mat4, points: Vec<glam::Vec3>) {
        let w = match self {
            Bike::Player(b) => b.wreck.as_mut(),
            Bike::Computer(b) => b.wreck.as_mut().and_then(|w| w.wreck.as_mut()),
            Bike::Taped(_) => None,
        };
        if let Some(w) = w {
            w.set_pose(pelvis, points);
        }
    }

    pub fn place(&self) -> Mat4 {
        match self {
            Bike::Player(b) => b.drawn_place(),
            Bike::Computer(b) => match &b.wreck {
                Some(w) => w.drawn_place(),
                None => b.drawn_place(),
            },
            Bike::Taped(b) => b.place(),
        }
    }

    /// How the model is turned from `place`: the player's bike crossed
    /// up in the air.
    pub fn drawn_turn(&self) -> glam::Quat {
        match self {
            Bike::Player(b) => b.drawn_turn(),
            Bike::Computer(_) | Bike::Taped(_) => glam::Quat::IDENTITY,
        }
    }

    /// What the rider's riding motions follow (0x48f390).
    pub fn rider_inputs(&self) -> crate::riding::Inputs {
        use mm2_physics::bike::PoseInputs;
        let level = |v: Vec3, f: Vec3| PoseInputs {
            rear_down: true,
            rear_speed: v.dot(f),
            ..Default::default()
        };
        // The game's bars (f_504) read positive to the left.
        match self {
            Bike::Player(b) => crate::riding::Inputs {
                steer: -b.rider.0,
                lean: b.rider.1,
                computer: false,
                bike: b.pose_inputs(),
            },
            Bike::Computer(b) => match &b.wreck {
                Some(w) => crate::riding::Inputs {
                    computer: true,
                    bike: w.pose_inputs(),
                    ..Default::default()
                },
                // In the air the computer's riders hold the cross-up
                // motions (CrosOv1–2) at their straight frame from the
                // first frame off the ground (the AI tape has rows 11–12
                // at phase 0 in 84% of its air frames; it drops them a few
                // frames before touching down, here they last). Their
                // tyres are never probed, so 0x48f390's clearance test
                // passes; it is held clear here.
                None => crate::riding::Inputs {
                    steer: -b.rider.steer,
                    lean: b.rider.lean,
                    computer: true,
                    bike: PoseInputs {
                        airborne: b.rider.airborne,
                        rear_down: b.rider.down[1],
                        air_pose: b.rider.airborne.then_some(0.0),
                        depth: if b.rider.airborne { [-10.0; 2] } else { [0.0; 2] },
                        ..level(b.rider.velocity, b.rider.look)
                    },
                },
            },
            Bike::Taped(b) => crate::riding::Inputs {
                computer: true,
                bike: PoseInputs {
                    airborne: b.airborne,
                    ..level(b.velocity, b.forward())
                },
                ..Default::default()
            },
        }
    }

    /// What the race camera follows (`ride::Chase::follow`).
    /// The Clutch and Back Up keys, on the player's bike.
    pub fn hands(&mut self, clutch: bool, back_up: bool) {
        if let Bike::Player(b) = self {
            b.clutch = clutch;
            b.reverse = back_up;
        }
    }

    pub fn followed(&self) -> crate::ride::Followed {
        let place = self.place();
        let airborne = self.airborne();
        crate::ride::Followed {
            position: place.w_axis.truncate(),
            forward: self.forward(),
            up: place.y_axis.truncate().normalize_or(Vec3::Y),
            velocity: self.velocity(),
            speed: self.speed(),
            crashed: self.crashed(),
            airborne,
            high: match self {
                Bike::Player(b) => b.rear_depth() < -2.0,
                _ => airborne,
            },
        }
    }

    pub fn velocity(&self) -> Vec3 {
        match self {
            Bike::Player(b) => b.velocity,
            Bike::Computer(b) => match &b.wreck {
                Some(w) => w.velocity,
                None => b.rider.velocity,
            },
            Bike::Taped(b) => b.velocity,
        }
    }

    /// The bike as the bike-to-bike bounce sees it, standing where it was
    /// last step at `was`; `None` while it is down or played from a tape.
    fn bump_state(&self, was: (Vec3, glam::Mat3)) -> Option<mm2_physics::bump::State> {
        use mm2_physics::bike::{COM, INERTIA, RIDER};
        let state = |origin: Vec3, axes: glam::Mat3, velocity, spin, weight: f32| {
            let mass = weight * 0.031_055_9;
            mm2_physics::bump::State {
                origin,
                axes,
                was,
                com: origin + axes * COM,
                velocity,
                spin,
                mass,
                inertia: INERTIA * mass / 12.0,
            }
        };
        match self {
            Bike::Player(b) if b.crash.is_none() => {
                let axes = b.rotation();
                let weight = b.spec.weight + RIDER;
                Some(state(
                    b.position - axes * COM,
                    axes,
                    b.velocity,
                    axes * b.spin,
                    weight,
                ))
            }
            Bike::Computer(b) if b.wreck.is_none() && !b.thrown => {
                let r = &b.rider;
                Some(state(r.position, r.axes(), r.velocity, r.spin, r.weight))
            }
            _ => None,
        }
    }

    /// Where the bump puts this bike's origin and axes now.
    fn stance(&self) -> Option<(Vec3, glam::Mat3)> {
        match self {
            Bike::Player(b) if b.crash.is_none() => {
                let axes = b.rotation();
                Some((b.position - axes * mm2_physics::bike::COM, axes))
            }
            Bike::Computer(b) if b.wreck.is_none() && !b.thrown => {
                Some((b.rider.position, b.rider.axes()))
            }
            _ => None,
        }
    }

    /// Takes a bump from another bike. The speed it leaves is the one
    /// the next frame's speed-loss check starts from (0x52bc20 sets
    /// `f_bc`, which becomes `f_b8`).
    fn bumped(&mut self, bump: &mm2_physics::bump::Bump) {
        match self {
            Bike::Player(b) => {
                let axes = b.rotation();
                b.spin += axes.transpose() * bump.spin;
            }
            Bike::Computer(b) => {
                b.rider.spin += bump.spin;
            }
            Bike::Taped(_) => {}
        }
        self.knock(bump.shift, bump.velocity, bump.landed_on);
        if let Bike::Computer(b) = self {
            b.rider.speed = b.rider.velocity.length();
        }
    }

    fn knock(&mut self, shift: Vec3, push: Vec3, crash: bool) {
        match self {
            Bike::Player(b) => b.knock(shift, push, crash),
            Bike::Computer(b) => match &mut b.wreck {
                Some(w) => w.knock(shift, push, crash),
                None => {
                    let r = &mut b.rider;
                    r.position += shift;
                    r.velocity += push;
                    r.front += push;
                    r.rear += push;
                    // Thrown off; the full model takes the bike on the
                    // next frame.
                    b.thrown |= crash;
                }
            },
            Bike::Taped(_) => {}
        }
    }

    /// Moves on by `dt`, part of a frame `frame` long that starts here
    /// when `first`: the player's bike steps once a frame, as the game's
    /// does, since above 50 fps its step is the frame's length.
    fn step(&mut self, ground: &TrackGround, controls: Controls, frame: f32, first: bool, fixed: bool) {
        match self {
            Bike::Player(b) => {
                if first {
                    b.fixed_step = fixed;
                    b.step(ground, controls, frame)
                }
            }
            Bike::Computer(_) => {}
            Bike::Taped(_) => {}
        }
    }
}

/// A computer rider's bike: the game's two-wheel model, and after a crash
/// the full model until the reset (0x414000 runs only uncrashed computer
/// riders; a crashed bike goes through Vehicle::Simulate like the
/// player's).
#[derive(Debug, Clone)]
pub struct Computer {
    pub rider: mm2_physics::computer::Rider,
    pub wreck: Option<Box<mm2_physics::Bike>>,
    /// Knocked off by another bike or a landing mid-trick, to crash on the
    /// next frame.
    thrown: bool,
    /// Where the model stood before its last frame, and how far through
    /// the next frame the screen is, 0 to 1: computer riders move in
    /// frames of [`AI_FRAME`] and are drawn between them.
    was: Option<(Vec3, glam::Quat)>,
    blend: f32,
}

impl Computer {
    fn place(&self) -> Mat4 {
        let r = &self.rider;
        crate::objects::orientation(r.look.into(), r.up.into(), Vec3::ONE, r.position.into())
    }

    /// Where to draw the model between its frames.
    fn drawn_place(&self) -> Mat4 {
        let now = self.place();
        let Some((position, from)) = self.was else {
            return now;
        };
        if position.distance(self.rider.position) > 20.0 {
            return now;
        }
        let (_, to, at) = now.to_scale_rotation_translation();
        Mat4::from_rotation_translation(from.slerp(to, self.blend), position.lerp(at, self.blend))
    }

    /// Off the two-wheel model onto the full one, crashed (0x414000:
    /// crash reason 4, the Endo bail).
    fn crash(&mut self, ground: &TrackGround, spec: &Spec, reset_to: Option<(Vec3, Vec3)>) {
        let r = &self.rider;
        let mut bike = mm2_physics::Bike::with_spec(ground, r.position, r.look, spec.clone());
        let right = r.up.cross(r.look).normalize_or(Vec3::X);
        let up = r.look.cross(right);
        let axes = glam::Mat3::from_cols(right, up, r.look);
        bike.set_state(
            r.position + axes * mm2_physics::bike::COM,
            r.velocity,
            axes,
            r.spin,
            [0.0; 2],
        );
        bike.hull = HULL.get().cloned();
        bike.reset_to = reset_to;
        // The game's crash clock, run on the thrown rider's touches.
        bike.crash_time = mm2_physics::bike::GAME_CRASH_TIME;
        bike.touch_clock = true;
        bike.fall(mm2_physics::Crash::Impact);
        self.wreck = Some(Box::new(bike));
    }
}

/// One bike in the race.
#[derive(Debug, Clone)]
pub struct Racer {
    pub bike: Bike,
    /// The bike's animated object; its rider is the next one.
    pub object: usize,
    start: (Vec3, Vec3),
    /// The course point being ridden towards.
    target: usize,
    /// Course points passed this lap.
    passed: usize,
    pub laps: u32,
    pub finished: Option<f32>,
    pub lap_times: Vec<f32>,
    /// What the rider asked of the bike last step (the engine sound
    /// follows the gas).
    pub controls: Controls,
    lap_start: f32,
    /// Where the finish line probe was last step (race entry +0x28),
    /// none yet at the origin.
    probe: Vec3,
    /// How hard a computer rider pushes, around 1.
    skill: f32,
    /// The line a computer rider follows (the game's BikeAI path), and how
    /// far to the side of it the autopilot keeps.
    line: Option<mm2_physics::ai::Path>,
    lane: f32,
    /// The confirmed place on the course (race entry +0x44), whether the
    /// tracker found the bike on the course this frame (+0x78c), and how
    /// long it has been off it (+0x76c).
    place: Option<Place>,
    /// A computer rider's place on a Baja or Enduro course's line, which
    /// it follows toward the next gate (+0x44 in 0x415170).
    line_place: Option<Place>,
    on_course: bool,
    off_course: f32,
    /// Where a computer rider with no course is heading (the quarries).
    wander: mm2_physics::ai::Wander,
    /// The game's race clock for this rider: crossings, best lap, splits
    /// and time behind the leader.
    pub timing: Timing,
    /// Where the bike was last step, for the gate crossings.
    last: Vec3,
    /// A computer rider's time since its last trick (bike +0x7dc).
    trick_wait: f32,
    /// Where the bike stood last step, its origin and axes, for the
    /// bikes' points to sweep against each other.
    stood: Option<(Vec3, glam::Mat3)>,
    /// The trick under way and the points this jump, and how long the
    /// trick's motion has played.
    pub stunt: Stunt,
    pub trick_clock: f32,
    /// A computer rider's hold left in its trick (0x415170's hold time),
    /// which keeps the motion at its hold frame as a held button would.
    ai_hold: f32,
    /// The rider looking round at another (action 9), and the phase
    /// through LookBack.
    pub taunt: Option<(crate::riding::Taunt, f32)>,
    /// In the air last step, and the trick buttons held last step.
    was_airborne: bool,
    held: [bool; 2],
    /// Stunt points scored this race (base × multiplier), and the most
    /// for one jump (`LargestSingleStuntPoints`).
    pub stunt_points: f32,
    pub best_stunt: f32,
    /// Repair, medical and stunt money units run up this race (bike
    /// +0x7ac, +0x7b0, +0x7b4), for the Pro Circuit payout.
    pub repair: f32,
    pub medical: f32,
    pub stunt_units: f32,
    /// Crashed last step, so each crash counts once.
    was_crashed: bool,
    /// The riding pose, and the rider's and the bike's motions it mixes.
    pub pose: crate::riding::Pose,
    pub mixes: (Vec<crate::riding::Mix>, Vec<crate::riding::Mix>),
    /// How far each drawn wheel has turned, front and rear, rad, and how
    /// fast it turns, rad/s (tyre +0x278 over the step).
    pub wheels: [(f32, f32); 2],
}

impl Racer {
    fn new(
        ground: &TrackGround,
        object: usize,
        (position, direction): (Vec3, Vec3),
        skill: f32,
        player: Option<&Spec>,
    ) -> Self {
        Racer {
            bike: if let Some(spec) = player {
                let mut bike =
                    mm2_physics::Bike::with_spec(ground, position, direction, spec.clone());
                // The game's gyro defaults (the profile's options replace
                // them): the brake tips the nose down in the air, the gas
                // does nothing.
                bike.gas_gyro = false;
                bike.brake_gyro = true;
                // Held at the gate until it drops.
                bike.racing = false;
                bike.hull = HULL.get().cloned();
                bike.cross_up = CROSS_UP.get().cloned();
                Bike::Player(Box::new(bike))
            } else {
                // A lane of ±7 ft from the line (±3 in the Baja and
                // Enduro, set by the race), from the game's rand().
                let mut seed = (object as u32).wrapping_mul(2_654_435_761) ^ 0x5bd1_e995;
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let lane = ((seed >> 16) & 0x7fff) as f32 / 32768.0 * 14.0 - 7.0;
                let class = mm2_physics::computer::engine_class(250);
                Bike::Computer(Box::new(Computer {
                    rider: mm2_physics::computer::Rider::new(ground, position, direction, lane, class),
                    wreck: None,
                    thrown: false,
                    was: None,
                    blend: 1.0,
                }))
            },
            object,
            start: (position, direction),
            target: 0,
            passed: 0,
            laps: 0,
            finished: None,
            lap_times: Vec::new(),
            controls: Controls::default(),
            lap_start: 0.0,
            probe: Vec3::ZERO,
            skill,
            line: None,
            // Riders spread across the track by their start positions.
            lane: ((object as f32 * 0.618).fract() - 0.5) * 4.0,
            wander: mm2_physics::ai::Wander::new(),
            place: None,
            line_place: None,
            on_course: true,
            off_course: 0.0,
            timing: Timing::new(false),
            last: position,
            trick_wait: 0.0,
            stood: None,
            wheels: [(0.0, 0.0); 2],
            stunt: Stunt::default(),
            trick_clock: 0.0,
            ai_hold: 0.0,
            taunt: None,
            was_airborne: false,
            held: [false; 2],
            stunt_points: 0.0,
            best_stunt: 0.0,
            repair: 0.0,
            medical: 0.0,
            stunt_units: 0.0,
            was_crashed: false,
            pose: Default::default(),
            mixes: Default::default(),
        }
    }

    /// Tricks for the player's rider (KrustyBike 0x48fa00, 0x4967d0,
    /// 0x496fa0): a trick button with the
    /// stick off centre starts one in the air, its motion runs its course,
    /// and a clean landing scores the jump. Returns the jump's base points
    /// and multiplier on a scoring landing.
    fn tricks(
        &mut self,
        controls: Controls,
        buttons: [bool; 2],
        motions: &[Option<TrickMotion>],
        dt: f32,
        ai: Option<(usize, f32)>,
    ) -> Option<(f32, f32)> {
        let air = self.bike.airborne();
        let landed = self.was_airborne && !air;
        self.was_airborne = air;
        let pressed = [buttons[0] && !self.held[0], buttons[1] && !self.held[1]];
        self.held = buttons;
        if self.bike.crashed() {
            // A crash loses the jump.
            self.stunt = Stunt::default();
            self.taunt = None;
            return None;
        }
        // The stick: the lean, or the gas and brake standing in for it.
        let lean = if controls.lean != 0.0 {
            controls.lean
        } else {
            controls.throttle - controls.brake
        };
        let dir = stunt::direction(lean, -controls.steer);
        if let Some((trick, hold)) = ai.filter(|_| air && !self.stunt.busy()) {
            // A computer rider's pick, with its hold time standing in for
            // a held button (0x415170).
            self.stunt.start(trick);
            self.stunt.held = hold;
            self.ai_hold = hold;
            self.trick_clock = 0.0;
        } else if air && !self.stunt.busy() && self.taunt.is_none() && (pressed[0] || pressed[1]) {
            if let Some(d) = dir {
                let trick = stunt::trick_for(buttons[0], d);
                if motions.get(trick).is_some_and(Option::is_some) {
                    self.stunt.start(trick);
                    self.trick_clock = 0.0;
                }
            }
        }
        if let Some(t) = self.stunt.trick {
            self.trick_clock += dt;
            let motion = motions.get(t).and_then(Option::as_ref);
            let frame = motion.map_or(0, |m| (self.trick_clock * m.fps) as i32);
            self.stunt.tick(dt, frame, buttons[0] || buttons[1], dir);
            let hold = match t {
                0..=15 => Some(stunt::HOLD_FRAME[t]),
                16 => Some(stunt::BAR_KNEEL_ALT_HOLD_FRAME),
                _ => None,
            };
            let at_hold = hold.is_some_and(|f| f < frame && frame < f + 3);
            if self.stunt.holding || (self.ai_hold > 0.0 && at_hold) {
                // A held trick stays at its hold frame (0x48fa00 keeps
                // the motion's clock while the button is down).
                self.ai_hold = (self.ai_hold - dt).max(0.0);
                self.trick_clock -= dt;
            }
            if self.stunt.trick != Some(t) {
                // Into the Alt: its motion starts afresh.
                self.trick_clock = 0.0;
            } else if motion.is_none_or(|m| frame >= m.frames) {
                self.stunt.finish();
            }
        }
        if !landed {
            return None;
        }
        if self.stunt.busy() || self.taunt.is_some() {
            self.taunt = None;
            // Landing in the middle of a trick throws the rider and the
            // trick scores nothing (KrustyBike 0x492650). The game picks
            // the bail from the tyres and speed; ours is the bike's crash.
            self.stunt = Stunt::default();
            self.bike.knock(Vec3::ZERO, Vec3::ZERO, true);
            return None;
        }
        if self.stunt.tricks == 0 {
            self.stunt = Stunt::default();
            return None;
        }
        let (base, m) = self.stunt.land();
        let score = base * m;
        if self.finished.is_some() {
            // After the flag a jump no longer counts (0x496e00).
            return None;
        }
        self.stunt_points += score;
        self.best_stunt = self.best_stunt.max(score);
        (base > 0.0).then_some((base, m))
    }

    /// Taunting (0x48f230, 0x48fe50): with the key down in the air, well
    /// clear of the ground with the air pose on and no trick running, the
    /// rider looks round at the nearest rider, when it is not ahead.
    fn taunting(&mut self, places: &[Vec3], me: usize, key: bool, dt: f32) {
        let Bike::Player(b) = &self.bike else {
            return;
        };
        let at = self.bike.position();
        let forward = self.bike.forward();
        let angle = |t: usize| crate::riding::angle_to(at, forward, places[t]);
        if let Some((taunt, phase)) = &mut self.taunt {
            match taunt.update(dt, angle(taunt.target)) {
                Some(p) => *phase = p,
                None => self.taunt = None,
            }
            return;
        }
        let pose = b.pose_inputs();
        let clear = pose.depth.iter().all(|&d| d < TAUNT_CLEAR);
        if !key
            || !pose.airborne
            || !clear
            || pose.air_pose.is_none()
            || self.bike.crashed()
            || self.stunt.busy()
        {
            return;
        }
        let nearest = (0..places.len())
            .filter(|&t| t != me)
            .min_by(|&p, &q| at.distance(places[p]).total_cmp(&at.distance(places[q])));
        if let Some(t) = nearest {
            self.taunt = crate::riding::Taunt::start(t, angle(t)).map(|t| (t, 0.5));
        }
    }

    /// A computer rider's trick at this moment, if it tries one (the AI
    /// controller 0x415170): once its wait for the level is over, in the
    /// air and not in a trick, it judges the flight and, with the level's
    /// chance in the Stunt Quarry (every time elsewhere), fits a trick in.
    fn ai_trick(
        &mut self,
        ground: &TrackGround,
        level: usize,
        quarry: bool,
        seed: &mut u32,
        dt: f32,
    ) -> Option<(usize, f32)> {
        let level = level.min(2);
        // Only the quarry waits 25/15/5 s between tricks. The lap and gate
        // branches of 0x415170 compare the timer with the level's int
        // loaded as float bits (0x415199, 0x4162c2), a denormal, so there
        // the wait is a single frame.
        let wait = if quarry { stunt::AI_TRICK_WAIT[level] } else { f32::from_bits(level as u32 + 1) };
        if self.trick_wait < wait {
            self.trick_wait += dt;
            return None;
        }
        if !self.bike.airborne() || self.stunt.busy() || self.bike.crashed() {
            return None;
        }
        let air = stunt::flight_time(ground, self.bike.position(), self.bike.velocity());
        // In a race the chance is 2% a frame (bike +0x15dc, 0x490c30). The
        // quarry's level chance is set too, but its wander branch reloads
        // race +0xa4 every frame, so there it is always that one.
        let chance = if quarry {
            stunt::AI_TRICK_CHANCE[2]
        } else {
            stunt::AI_RACE_TRICK_CHANCE
        };
        let pick = stunt::ai_trick(air, chance, || {
            *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (*seed >> 8) as f32 / (1u32 << 24) as f32
        });
        if pick.is_some() {
            self.trick_wait = 0.0;
        }
        pick
    }

    /// The rider's and bike's motions for the trick under way, and how
    /// long they have played.
    pub fn trick_motion<'a>(
        &self,
        motions: &'a [Option<TrickMotion>],
    ) -> Option<(&'a TrickMotion, f32)> {
        let t = self.stunt.trick?;
        Some((motions.get(t)?.as_ref()?, self.trick_clock))
    }

    /// Puts a computer rider into a state read from a tape of the game:
    /// where it is, its velocity, along and up the bike and its spin. The
    /// tracker finds its place on the course afresh.
    pub fn set_computer(&mut self, position: Vec3, velocity: Vec3, look: Vec3, up: Vec3, spin: Vec3) {
        if let Bike::Computer(c) = &mut self.bike {
            let r = &mut c.rider;
            r.position = position;
            r.velocity = velocity;
            r.look = look;
            r.up = up;
            r.spin = spin;
            r.speed = velocity.length();
            r.last_speed = r.speed;
            r.reset_spin_axes();
            c.wreck = None;
            self.place = None;
        }
    }

    /// Whether the race tracker last found a computer rider on the course.
    pub fn on_course(&self) -> bool {
        self.on_course
    }

    /// Seconds off the course (+0x76c).
    pub fn off_course(&self) -> f32 {
        self.off_course
    }

    /// Off the course in a lap race: the confirmed place on it (race entry
    /// +0x44), where the Visual Cue points and the runway lights stand
    /// (0x48c0a0, 0x48b700), and the course's level way there.
    pub fn rejoin(&self, course: &Course) -> Option<(Vec3, Vec3)> {
        if self.on_course {
            return None;
        }
        let p = self.place?;
        Some((course.point(p), course.way(p)))
    }

    /// Seconds into the current lap, at race time `clock`.
    pub fn lap_time(&self, clock: f32) -> f32 {
        self.finished.unwrap_or(clock) - self.lap_start
    }

    pub fn best_lap(&self) -> Option<f32> {
        self.lap_times.iter().copied().reduce(f32::min)
    }

    /// How far round the race this rider is, for ranking: in a gate race,
    /// the gates passed and then the nearest to the next one.
    fn progress(&self, course: Option<&Course>, gates: &[race::Line]) -> f32 {
        if let Some(next) = gates.get(self.timing.gate) {
            let d = flat(next.centre - self.bike.position()).length();
            return self.timing.gates as f32 + 1.0 / (1.0 + d / 100.0);
        }
        let lap = course.map_or(1, |c| c.lap) as f32;
        self.laps as f32 + self.passed as f32 / lap
    }

    /// Passes the next gate when the bike goes through it (0x4ea750);
    /// past the last one a lap is done.
    fn pass_gates(&mut self, gates: &[race::Line], clock: f32, laps: u32, computer: bool) {
        let at = self.bike.position();
        let from = std::mem::replace(&mut self.last, at);
        if self.finished.is_some() {
            return;
        }
        let Some(&gate) = gates.get(self.timing.gate) else {
            return;
        };
        let gate = if computer {
            race::Line {
                half_width: gate.half_width * COMPUTER_GATE,
                ..gate
            }
        } else {
            gate
        };
        if !gate.crossed(from, at, true) {
            return;
        }
        let before = self.timing.crossings;
        self.timing
            .pass_gate(gates.len(), laps, laps >= ENDLESS, false);
        if self.timing.crossings > before {
            self.laps += 1;
            self.passed = 0;
            self.lap_times.push(clock - self.lap_start);
            self.lap_start = clock;
            if self.laps >= laps {
                self.finished = Some(clock);
            }
        }
    }

    /// Steering, throttle and brake to follow the course.
    fn drive(&mut self, course: &Course) -> Controls {
        let speed = self.bike.speed().max(0.0);
        let look = 15.0 + speed * 0.4;
        let at = self.bike.position();
        // The line is laid from just behind the rider over the next few
        // course points, and laid again as the rider moves on. The game
        // lays up to 200 points, but our course points come from the
        // `.tdf`, whose stadium laps fold back alongside themselves; a short
        // line keeps the nearest point from jumping to a later stretch.
        let stale = self.line.as_ref().is_none_or(|l| l.segment >= 4);
        if stale {
            let count = LINE_POINTS.min(course.lap);
            let mut points = Vec::with_capacity(count);
            let mut i = course.previous(course.previous(self.target));
            for _ in 0..count {
                points.push(course.points[i]);
                i = course.next[i];
            }
            self.line = Some(mm2_physics::ai::Path::new(points, look));
        }
        let line = self.line.as_mut().unwrap();
        line.lookahead = look;
        let aim = line.direction(at, self.lane);
        let b = &self.bike;
        let far = course.ahead(at, self.target, look + 60.0) - flat(at);
        let forward = b.forward();
        let right = Vec3::new(forward.z, 0.0, -forward.x);
        let steer = (aim.dot(right) * 2.5).clamp(-1.0, 1.0);
        // Slow for the bend ahead.
        let bend = forward.angle_between(far.normalize_or(forward));
        let wanted = (110.0 * self.skill * (1.0 - bend / 1.6)).clamp(25.0, 110.0);
        // Off the gas in the air, so the gyro leaves the bike level.
        let ground = !b.airborne();
        Controls {
            throttle: if ground && speed < wanted { 1.0 } else { 0.0 },
            brake: if ground && speed > wanted + 12.0 {
                1.0
            } else {
                0.0
            },
            steer,
            lean: 0.0,
        }
    }

    /// A computer rider's line, speed limit and power for this frame (the
    /// AI controller 0x415170).
    ///
    /// * In a lap race the line is 100 points along the course from the
    ///   confirmed place, a tenth of a second's riding apart at the
    ///   present speed, 2 ft above the bike; off the course, one point
    ///   100 ft on. Once finished, the rider pulls off toward the nearer
    ///   side of the course at 40 ft/s.
    /// * In the Baja and Enduro it rides at the next gate: from 300 ft off
    ///   straight at it, nearer through a point a second's riding before
    ///   it and on through one a second after, at 40 ft/s when it is more
    ///   than 45° off the way the bike is going.
    /// * With neither (the quarries), it wanders.
    ///
    /// The limit is otherwise 1000 ft/s, flat out, until the jump planner
    /// lowers it; the look-ahead is a quarter of a second's riding.
    fn plan(
        &mut self,
        course: Option<&Course>,
        gates: &[race::Line],
        level: usize,
        scale: f32,
        seed: &mut u32,
    ) {
        let Bike::Computer(c) = &self.bike else {
            return;
        };
        let r = &c.rider;
        let (at, speed) = (r.position, r.speed);
        let mut power = mm2_physics::computer::POWER[level.min(2)];
        let mut limit = 1000.0;
        let raise = |p: Vec3| Vec3::new(p.x, at.y + 2.0, p.z);
        let mut widen = false;
        let points: Vec<Vec3> = if let (Some(gate), Some(course)) = (gates.get(self.timing.gate), course) {
            // A Baja or Enduro course with its line: along the line from
            // the confirmed place toward the next gate's, ten seconds'
            // riding laid out a tenth at a time, while the place under the
            // bike is nearer the gate than that (and it becomes the
            // confirmed place); otherwise straight at the gate.
            let g = gate.centre;
            let far = |from: Place, to: Place| course.along(from, to, f32::MAX).unwrap_or(0.0);
            let target = course.nearest_place(g);
            let here = course.nearest_place(at);
            let mut kept = self.line_place.or(here);
            let (before, now) = match (kept, here, target) {
                (Some(k), Some(h), Some(t)) => (far(k, t), far(h, t)),
                _ => (0.0, 0.0),
            };
            if now > 0.0 && before > 0.0 && now < before {
                kept = here;
            }
            self.line_place = kept;
            let off = flat(g - at).length();
            if off < speed {
                let to = (g - at).normalize_or_zero();
                if to.dot(r.velocity.normalize_or_zero()) < 0.707_55 {
                    limit = 40.0;
                }
            }
            match kept {
                Some(k) if before > now => (0..100)
                    .map(|i| raise(course.point(course.walk(k, i as f32 * 0.1 * speed))))
                    .collect(),
                _ => vec![g],
            }
        } else if let Some(gate) = gates.get(self.timing.gate) {
            let g = gate.centre;
            let look = gate.forward;
            let off = flat(g - at).length();
            if speed > off {
                let to = (g - at).normalize_or_zero();
                let going = r.velocity.normalize_or_zero();
                if to.dot(going) < 0.707_55 {
                    limit = 40.0;
                }
            }
            if off >= 300.0 {
                vec![g]
            } else if (g - at).dot(look) >= 0.0 {
                vec![g - look * speed, g, g + look * speed]
            } else {
                vec![g + look * speed, g, g - look * speed]
            }
        } else if let Some(course) = course {
            widen = true;
            match self.place {
                None => vec![at + r.look * speed],
                Some(place) if self.finished.is_some() => {
                    // Off to the nearer side at 40 ft/s.
                    limit = 40.0;
                    let mut way = course.way(place);
                    if way.dot(r.look) < 0.0 {
                        way = -way;
                    }
                    let mid = course.point(place);
                    let right = Vec3::new(way.z, 0.0, -way.x);
                    let side = if (at - mid).dot(right) >= 0.0 { right } else { -right };
                    vec![at + side * 40.0]
                }
                Some(place) if self.on_course => (0..100)
                    .map(|i| raise(course.point(course.walk(place, i as f32 * 0.1 * speed))))
                    .collect(),
                Some(place) => vec![raise(course.point(course.walk(place, 100.0)))],
            }
        } else {
            power = 1.5;
            let target = self.wander.aim(at, scale, || {
                *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (*seed >> 8) as f32 / (1u32 << 24) as f32
            });
            vec![target]
        };
        let mut path = mm2_physics::ai::Path::new(points, speed * 0.25);
        path.limit = limit;
        path.power = power;
        path.widen = widen;
        self.line = Some(path);
    }

    /// One frame of a computer rider: the controller's line, then the
    /// two-wheel model along it, or the full model while crashed.
    #[allow(clippy::too_many_arguments)]
    fn ride(
        &mut self,
        ground: &TrackGround,
        course: Option<&Course>,
        gates: &[race::Line],
        level: usize,
        supercross: bool,
        seed: &mut u32,
        air_lean: &mut f32,
        movers: &[mm2_physics::ai::Mover],
        spec: &Spec,
        frame: f32,
    ) {
        // Where it is on the course (0x4eb170, from 3.5 ft ahead). The
        // tracker follows a crashed bike too, so the rider is put back
        // where the wreck came to lie: the original's computer riders get
        // up a few feet from their wrecks, often 90 ft on from the crash.
        if let (Some(course), Bike::Computer(_)) = (course, &self.bike) {
            let (at, look) = (self.bike.position(), self.bike.forward());
            self.on_course = course.follow(at + look * 3.5, look, &mut self.place).is_some();
            // Only the lap races' tracker counts the time off the course.
            self.off_course = if self.on_course || !gates.is_empty() { 0.0 } else { self.off_course + frame };
        }
        let reset_to = match (course, self.place) {
            (Some(course), Some(p)) => Some((course.point(p), course.way(p))),
            _ => None,
        };
        self.plan(course, gates, level, ground.terrain.scale, seed);
        let busy = self.stunt.busy();
        let Bike::Computer(c) = &mut self.bike else {
            return;
        };
        c.rider.air_pose = !busy;
        if let Some(w) = &mut c.wreck {
            w.reset_to = reset_to;
            w.step(ground, Controls::default(), frame);
            if w.crash.is_none() {
                // Put back on the course: on with the two-wheel model.
                let place = w.place();
                let (lane, class) = (c.rider.lane, c.rider.class);
                c.rider = mm2_physics::computer::Rider::new(
                    ground,
                    place.w_axis.truncate(),
                    place.z_axis.truncate(),
                    lane,
                    class,
                );
                c.wreck = None;
                c.was = None;
            }
            return;
        }
        // Off the course too long: back on it (0x492ac0, 6 s).
        if self.off_course > OFF_COURSE_RESET {
            if let Some((at, way)) = reset_to {
                let (lane, class) = (c.rider.lane, c.rider.class);
                c.rider = mm2_physics::computer::Rider::new(ground, at - way * 3.5, way, lane, class);
                c.was = None;
            }
            self.off_course = 0.0;
        }
        let path = self.line.as_mut().expect("planned");
        c.rider.tolerance = mm2_physics::computer::tolerance(level, supercross);
        if c.rider.hull.is_none() {
            c.rider.hull = HULL.get().cloned();
            c.rider.weight = spec.weight + mm2_physics::bike::RIDER;
        }
        let place = c.place();
        let (_, turn, at) = place.to_scale_rotation_translation();
        c.was = Some((at, turn));
        let crash = c.rider.step(ground, path, frame, air_lean, !gates.is_empty(), movers);
        if crash || std::mem::take(&mut c.thrown) {
            c.crash(ground, spec, reset_to);
        }
    }

    /// With no course to follow, as in the quarries, rides flat out at a
    /// point and picks another on reaching it (0x415170's last branch).
    fn roam(&mut self, scale: f32, seed: &mut u32) -> Controls {
        let at = self.bike.position();
        let aim = self.wander.aim(at, scale, || {
            *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (*seed >> 8) as f32 / (1u32 << 24) as f32
        });
        let forward = self.bike.forward();
        let right = Vec3::new(forward.z, 0.0, -forward.x);
        let steer = (flat(aim - at).normalize_or_zero().dot(right) * 2.5).clamp(-1.0, 1.0);
        let ground = !self.bike.airborne();
        Controls {
            throttle: if ground && self.bike.speed() < mm2_physics::ai::WANDER_SPEED {
                1.0
            } else {
                0.0
            },
            brake: 0.0,
            steer,
            lean: 0.0,
        }
    }

    /// Moves the course target past points the bike has reached, and counts
    /// laps over the finish line.
    fn track(&mut self, course: &Course, clock: f32, laps: u32, gates: bool) {
        let p = flat(self.bike.position());
        for _ in 0..course.points.len() {
            let here = course.points[self.target];
            let next = course.points[course.next[self.target]];
            let along = flat(next - here);
            // Past this point once ahead of it along the leg it starts.
            if (p - flat(here)).dot(along) < 0.0
                || (p - flat(next)).length() > along.length() + 40.0
            {
                break;
            }
            self.target = course.next[self.target];
            self.passed += 1;
        }

        if gates {
            return;
        }
        // The game's probe, 3.5 ft ahead of the bike, crossing the finish
        // line one way (0x4eb170, 0x47c5b0). A line with no direction, as
        // custom tracks that hide the arch have, counts either way.
        let probe = self.bike.position() + self.bike.forward() * 3.5;
        let crossed = course.finish_line().crossed(self.probe, probe, true);
        self.probe = probe;
        // A lap counts once most of the course has gone by.
        if crossed && self.passed * 10 >= course.lap * 6 && self.finished.is_none() {
            self.laps += 1;
            self.passed = 0;
            self.lap_times.push(clock - self.lap_start);
            self.lap_start = clock;
            self.timing.cross_line(laps, laps >= ENDLESS);
            if self.laps >= laps {
                self.finished = Some(clock);
            }
        }
    }
}

pub struct Race {
    pub racers: Vec<Racer>,
    /// Which racer the player rides.
    pub player: usize,
    /// The computer rides for the player too.
    pub autopilot: bool,
    /// The camera keys held this frame.
    pub cam_keys: crate::ride::CamKeys,
    /// Wreck Reset (Game page): a crash puts the player back on the
    /// course; off, where the bike came to rest (0x492cc0). Network races
    /// always reset.
    pub wreck_reset: bool,
    /// Visual Cues (F4, profile +0x6a8, on in a new profile): the green
    /// arrow to where the player should head and, in a lap race, the
    /// runway lights back to the course (0x4e54c0).
    pub visual_cues: bool,
    /// Player Indicator (Game page): the sign over the player's own bike
    /// when there is anyone else in the race (0x418cd0's `youind.slt`).
    pub player_indicator: bool,
    /// The rider the camera watches, [ and ] or Page Up and Down
    /// (0x41ffd0); none, the player.
    pub watched: Option<usize>,
    /// The player's bike steps in whole 0.02 s steps (live play), not
    /// once a frame as the game does (the scripted comparison runs).
    pub fixed_step: bool,
    /// The computer riders' frame, s: their controller and two-wheel
    /// model run once a frame, with rules per frame, so they ride as the
    /// original's did at its frame rate only at that rate. 0 runs them
    /// once each update (scripted runs, at a tape's own frames).
    pub ai_frame: f32,
    ai_clock: f32,
    pub course: Option<Course>,
    pub laps: u32,
    /// The checkpoint gates of a Baja or Enduro course; none in a lap race,
    /// where the finish line counts the laps.
    pub gates: Vec<race::Line>,
    /// Seconds since the start; the gate drops at `lead`.
    pub time: f32,
    /// Seconds from the start until the gate drops (race +0x15c).
    pub lead: f32,
    chase: Chase,
    /// The player's bike.
    spec: Spec,
    /// For the times given to computer riders the race finishes.
    seed: u32,
    /// The player's trick buttons (the game's buttons 6 and 7).
    pub buttons: [bool; 2],
    /// The player's Kahuna Dumpster and Taunting keys (actions 8, 9).
    pub actions: [bool; 2],
    /// The tricks' rider and bike motions, by trick (Alts from 16).
    pub tricks: Vec<Option<TrickMotion>>,
    /// The last jump the player scored: when (race time), its base points
    /// and multiplier, for the score that flies up from the bike.
    pub popup: Option<(f32, f32, f32)>,
    /// The Stunt Quarry: no laps, riders ranked by stunt points, and the
    /// session's length in minutes unless it has no end (practice).
    pub stunt_event: bool,
    /// The event's stunt prize money; none pays nothing.
    pub purse: Option<stunt::Purse>,
    /// Each racer's name from the menus; see `rider_name`.
    pub names: Vec<String>,
    pub minutes: Option<f32>,
    /// The computer riders' level, 0 to 2 (the game's 1 to 3).
    pub level: usize,
    /// A Supercross race: the computer riders take a little less (0x492950).
    pub supercross: bool,
    /// The racer that is a ghost race's ghost: played from a tape, nobody
    /// touches it and it takes no place.
    pub ghost: Option<usize>,
    /// Practice or a ghost race: no flag girl or 30-second man.
    pub practice: bool,
    /// The flag girl at the finish line.
    pub flag_girl: crate::finish::FlagGirl,
    /// The podium once the race is over, and the clock when it went up.
    pub podium: Option<(crate::podium::Podium, f32)>,
    /// Moto-Tag's rules, in a tag game.
    pub tag: Option<crate::tag::Tag>,
    /// The computer riders' smoothed air lean, one for all of them
    /// (0x57dc10).
    air_lean: f32,
}

/// One trick's motions: the rider's and the bike's, their rate and length.
#[derive(Debug, Clone, PartialEq)]
pub struct TrickMotion {
    pub rider: String,
    pub bike: String,
    pub fps: f32,
    pub frames: i32,
}

impl Race {
    /// The next camera view (Numpad 5), and its name.
    pub fn next_view(&mut self) -> &'static str {
        self.chase.next_view()
    }

    /// Numpad 0: the camera freezes where it is, or goes back to its view.
    pub fn freeze_camera(&mut self) -> bool {
        self.chase.freeze()
    }

    /// Home and End: the view narrows or widens 3°.
    pub fn zoom(&mut self, by: f32) {
        self.chase.zoom(by);
    }

    /// The view's width, degrees across.
    pub fn fov(&self) -> f32 {
        self.chase.fov()
    }

    /// Physics runs in fixed steps, whatever the frame rate.
    const STEP: f32 = 1.0 / 120.0;

    /// Riders on `grid` (each an object and where it stands), facing
    /// `direction`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ground: &TrackGround,
        grid: &[(usize, Vec3)],
        player: usize,
        direction: Vec3,
        def: Option<&TrackDef>,
        laps: u32,
        spec: Spec,
        gates: Vec<race::Line>,
    ) -> Option<Self> {
        if grid.is_empty() {
            return None;
        }
        let course = def.and_then(Course::new);
        let player = player.min(grid.len() - 1);
        let racers: Vec<Racer> = grid
            .iter()
            .enumerate()
            .map(|(i, &(object, at))| {
                let skill = 0.85 + 0.05 * i as f32;
                let me = (i == player).then_some(&spec);
                Racer::new(ground, object, (at, direction), skill, me)
            })
            .collect();
        let mut race = Race {
            chase: Chase::new(Vec3::ZERO, Vec3::Z),
            racers,
            player,
            autopilot: false,
            cam_keys: Default::default(),
            wreck_reset: true,
            visual_cues: true,
            player_indicator: true,
            watched: None,
            fixed_step: true,
            ai_frame: AI_FRAME,
            ai_clock: 0.0,
            course,
            laps,
            gates,
            time: 0.0,
            lead: START_LEAD,
            spec,
            seed: 0x1234_5678,
            buttons: [false; 2],
            tricks: Vec::new(),
            actions: [false; 2],
            popup: None,
            stunt_event: false,
            purse: None,
            names: Vec::new(),
            minutes: None,
            level: 1,
            supercross: false,
            ghost: None,
            practice: false,
            flag_girl: Default::default(),
            podium: None,
            tag: None,
            air_lean: 0.0,
        };
        race.restart(ground);
        Some(race)
    }

    pub fn restart(&mut self, ground: &TrackGround) {
        for (i, r) in self.racers.iter_mut().enumerate() {
            let me = (i == self.player).then_some(&self.spec);
            let old = std::mem::replace(r, Racer::new(ground, r.object, r.start, r.skill, me));
            // The rider's own settings stay.
            if let (Bike::Player(was), Bike::Player(b)) = (&old.bike, &mut r.bike) {
                b.gas_gyro = was.gas_gyro;
                b.brake_gyro = was.brake_gyro;
                b.sliders = was.sliders;
                b.drivetrain = was.drivetrain.fresh();
                b.fixed_step = was.fixed_step;
            }
            r.timing = Timing::new(!self.gates.is_empty());
            if let Some(c) = &self.course {
                r.target = c.nearest(r.bike.position());
                r.probe = Vec3::ZERO;
            }
        }
        self.time = 0.0;
        self.popup = None;
        let me = &self.racers[self.player].bike;
        self.chase = self.chase.restart(me.position(), me.forward());
    }

    pub fn gate_down(&self) -> bool {
        self.time >= self.lead
    }

    /// Seconds since the gate dropped.
    pub fn clock(&self) -> f32 {
        (self.time - self.lead).max(0.0)
    }

    /// Advances by `dt` seconds with the player's `controls`, and returns
    /// the chase camera's eye and target in game coordinates.
    pub fn update(&mut self, ground: &TrackGround, controls: Controls, dt: f32) -> (Vec3, Vec3) {
        // The computer riders' frame, when one is due.
        // A frame takes `ai_frame` and the rest is kept for the next, so
        // the frames come evenly and the drawing between them (by the time
        // since the last) moves the bikes on evenly: taking the whole of
        // the clock made the bikes jump on and stall in turn at 60 fps.
        // Under half the frame rate, one frame takes the lot.
        self.ai_clock += dt;
        let ai_dt = if self.ai_frame <= 0.0 || self.ai_clock >= self.ai_frame * 2.0 {
            std::mem::take(&mut self.ai_clock)
        } else if self.ai_clock >= self.ai_frame * 0.999 {
            self.ai_clock = (self.ai_clock - self.ai_frame).max(0.0);
            self.ai_frame
        } else {
            0.0
        };
        let mut left = dt;
        while left > 0.0 {
            let first = left == dt;
            let step = left.min(Self::STEP);
            self.time += step;
            let racing = self.gate_down();
            let clock = self.clock();
            let places: Vec<Vec3> = self.racers.iter().map(|r| r.bike.position()).collect();
            let going: Vec<Vec3> = self.racers.iter().map(|r| r.bike.velocity()).collect();
            for (i, r) in self.racers.iter_mut().enumerate() {
                let computer = i != self.player || self.autopilot;
                let ai = matches!(r.bike, Bike::Computer(_));
                let taped = matches!(r.bike, Bike::Taped(_));
                // A crashed computer rider is on the full model, which
                // steps in whole 0.02 s steps like the player's, each
                // frame.
                let wrecked = match &mut r.bike {
                    Bike::Computer(c) => c
                        .wreck
                        .as_mut()
                        .map(|w| w.fixed_step = self.fixed_step)
                        .is_some(),
                    _ => false,
                };
                // The controller tries its trick before the model steps
                // (0x414000 → 0x415170), on the frame's starting state.
                let pick = if ai && first && racing && ai_dt > 0.0 {
                    r.ai_trick(ground, self.level, self.stunt_event, &mut self.seed, ai_dt)
                } else {
                    None
                };
                if ai && first && racing && (ai_dt > 0.0 || wrecked) {
                    // The other bikes, where they are and will be in a
                    // quarter second, for the obstacle scan.
                    let movers: Vec<mm2_physics::ai::Mover> = (0..places.len())
                        .filter(|&j| j != i)
                        .map(|j| mm2_physics::ai::Mover {
                            at: places[j],
                            to: places[j] + going[j] * 0.25,
                        })
                        .collect();
                    r.ride(
                        ground,
                        self.course.as_ref(),
                        &self.gates,
                        self.level,
                        self.supercross,
                        &mut self.seed,
                        &mut self.air_lean,
                        &movers,
                        &self.spec,
                        if wrecked { dt } else { ai_dt },
                    );
                }
                let mut c = match (&self.course, computer) {
                    _ if taped => Controls::default(),
                    _ if ai => match &r.bike {
                        Bike::Computer(b) => Controls {
                            throttle: b.rider.gas as u8 as f32,
                            brake: b.rider.brake as u8 as f32,
                            steer: b.rider.steer,
                            lean: b.rider.lean,
                        },
                        _ => Controls::default(),
                    },
                    (Some(course), true) => r.drive(course),
                    (None, true) => r.roam(ground.terrain.scale, &mut self.seed),
                    (_, false) => controls,
                };
                if let Bike::Player(b) = &mut r.bike {
                    // The game holds the player's bike itself, engine running.
                    b.racing = racing;
                } else if !racing {
                    c = Controls::default();
                }
                if r.finished.is_some() && computer && !ai {
                    // Coasting after the flag.
                    c = Controls::default();
                }
                r.controls = c;
                let busy = r.stunt.busy() || r.taunt.is_some();
                if let Bike::Player(b) = &mut r.bike {
                    b.stunting = busy;
                }
                r.bike.step(ground, c, dt, first, self.fixed_step);
                let scored = if i == self.player {
                    if let Bike::Player(b) = &mut r.bike {
                        if self.actions[0] && !busy {
                            b.dump();
                        }
                    }
                    let s = r.tricks(c, self.buttons, &self.tricks, step, None);
                    if let Some((base, m)) = s {
                        self.popup = Some((self.time, base, m));
                    }
                    r.taunting(&places, i, self.actions[1], step);
                    s
                } else if racing && !taped {
                    // The controller runs once a frame.
                    r.tricks(c, [false; 2], &self.tricks, step, pick)
                } else {
                    None
                };
                if let (Some((base, _)), Some(purse)) = (scored, &self.purse) {
                    purse.pay(base, &mut r.stunt_units);
                }
                // Each crash runs up repairs and medical bills (0x48ea00).
                let crashed = r.bike.crashed();
                if crashed && !r.was_crashed && racing && r.finished.is_none() {
                    let seed = &mut self.seed;
                    let (repair, medical) =
                        mm2_physics::career::crash_units(i != self.player, || {
                            *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                            (*seed >> 8) as f32 / (1u32 << 24) as f32
                        });
                    r.repair += repair;
                    r.medical += medical;
                }
                let got_up = r.was_crashed && !crashed;
                r.was_crashed = crashed;
                if racing {
                    r.timing.tick(step, true);
                }
                let laps = !self.gates.is_empty() || self.stunt_event;
                if let Some(course) = &self.course {
                    if racing {
                        r.track(course, clock, self.laps, laps);
                    }
                    // The player's place on the course, as the computer
                    // riders' (0x4eb170), for the Visual Cues.
                    if let Bike::Player(b) = &r.bike {
                        let look = b.forward();
                        let probe = r.bike.position() + look * 3.5;
                        if got_up {
                            // Put back after a crash, maybe well behind
                            // where the wreck slid to: the place starts
                            // afresh where the bike now stands.
                            r.place = course.facing(probe, look).or(r.place);
                        }
                        r.on_course = course.follow(probe, look, &mut r.place).is_some();
                        r.off_course = if r.on_course { 0.0 } else { r.off_course + step };
                    }
                    // Over 6 s off the course in a lap race (not practice),
                    // the player too is put back where they left it,
                    // crashed or not (0x492ac0 → 0x492cc0); the 3D numbers
                    // count down to it. Only the lap races' tracker
                    // (0x4eb170) counts the time off the course.
                    if !self.practice && self.gates.is_empty() && r.off_course > OFF_COURSE_RESET {
                        if let (Some(p), Bike::Player(b)) = (r.place, &mut r.bike) {
                            let (at, way) = (course.point(p), course.way(p));
                            b.reset(ground, at - way * 3.5, way);
                            r.off_course = 0.0;
                        }
                    }
                    if !crashed && (self.wreck_reset || i != self.player) {
                        if let Some(place) = course.place(r.bike.position(), r.target) {
                            if let Bike::Player(b) = &mut r.bike {
                                b.reset_to = Some(place);
                            }
                        }
                    }
                }
                if racing && !self.gates.is_empty() && !self.stunt_event && self.tag.is_none() {
                    r.pass_gates(&self.gates, clock, self.laps, computer);
                }
            }
            if racing {
                self.contact();
                self.tag_step(ground, step);
                if self.stunt_event || self.tag.is_some() {
                    self.end_session();
                } else {
                    self.rank();
                }
            }
            left -= step;
        }
        let blend = if self.ai_frame > 0.0 {
            (self.ai_clock / self.ai_frame).min(1.0)
        } else {
            1.0
        };
        for r in &mut self.racers {
            if let Bike::Computer(c) = &mut r.bike {
                c.blend = blend;
            }
        }
        self.pose_riders(dt);
        if let Some(course) = &self.course {
            let me = &self.racers[self.player];
            let (ahead, behind) = course.to_finish(me.bike.position());
            self.flag_girl
                .update(self.time, me.laps, self.laps, me.finished.is_some(), ahead, behind);
        }
        let me = self.racers[self.watching()].bike.followed();
        self.chase.follow(&me, self.cam_keys, ground.terrain, dt)
    }

    /// The rider the camera follows.
    pub fn watching(&self) -> usize {
        self.watched.filter(|&i| i < self.racers.len()).unwrap_or(self.player)
    }

    /// The camera on to the next rider (or the one before), round to the
    /// player again; the camera starts afresh behind them.
    pub fn watch_next(&mut self, forward: bool) {
        let n = self.racers.len();
        let mut i = self.watching();
        loop {
            i = if forward { (i + 1) % n } else { (i + n - 1) % n };
            if Some(i) != self.ghost {
                break;
            }
        }
        self.watched = (i != self.player).then_some(i);
        let b = &self.racers[i].bike;
        self.chase = self.chase.restart(b.position(), b.forward());
    }

    /// Picks each rider's riding motions for a frame of `dt` seconds, and
    /// turns the wheels.
    fn pose_riders(&mut self, dt: f32) {
        for r in &mut self.racers {
            for ((angle, rate), now) in r.wheels.iter_mut().zip(r.bike.wheel_rates()) {
                // A wheel off the ground keeps turning as it was (0x519ab0
                // turns it by its last step's angle again).
                *rate = now.unwrap_or(*rate);
                *angle = (*angle + *rate * dt).rem_euclid(std::f32::consts::TAU);
            }
            let inputs = r.bike.rider_inputs();
            r.mixes = r.pose.update(&inputs, dt);
            if let Some((_, phase)) = r.taunt {
                r.mixes = crate::riding::taunting(phase);
            }
        }
    }

    /// A replay at tape time `time`: each bike where `bike` says (left
    /// where it was when the tape has nothing), its trick's motion running,
    /// and the chase camera on the rider followed. Returns the camera's
    /// eye and target.
    pub fn replay(
        &mut self,
        time: f32,
        bike: impl Fn(usize) -> Option<Bike>,
        terrain: &mm2_formats::trn::Terrain,
    ) -> (Vec3, Vec3) {
        let dt = time - self.time;
        self.time = time;
        for (i, r) in self.racers.iter_mut().enumerate() {
            if let Some(b) = bike(i) {
                r.bike = b;
            }
            let trick = match &r.bike {
                Bike::Taped(t) => t.trick,
                _ => None,
            };
            if trick != r.stunt.trick || dt < 0.0 {
                r.trick_clock = 0.0;
            } else {
                r.trick_clock += dt;
            }
            r.stunt.trick = trick;
        }
        // A taunt on the tape plays as the rider looked round.
        let places: Vec<Vec3> = self.racers.iter().map(|r| r.bike.position()).collect();
        for (i, r) in self.racers.iter_mut().enumerate() {
            let taunting = matches!(&r.bike, Bike::Taped(t) if t.taunting);
            if !taunting || dt < 0.0 {
                r.taunt = None;
                continue;
            }
            let at = r.bike.position();
            let forward = r.bike.forward();
            let angle = |t: usize| crate::riding::angle_to(at, forward, places[t]);
            match &mut r.taunt {
                Some((taunt, phase)) => {
                    if let Some(p) = taunt.update(dt, angle(taunt.target)) {
                        *phase = p;
                    }
                }
                None => {
                    let nearest = (0..places.len())
                        .filter(|&t| t != i)
                        .min_by(|&p, &q| at.distance(places[p]).total_cmp(&at.distance(places[q])));
                    r.taunt = nearest
                        .and_then(|t| crate::riding::Taunt::start(t, angle(t)))
                        .map(|t| (t, 0.5));
                }
            }
        }
        self.pose_riders(dt.clamp(0.0, 0.5));
        let me = &self.racers[self.player].bike;
        if !(0.0..0.5).contains(&dt) {
            // A jump along the tape: the camera starts afresh.
            self.chase = self.chase.restart(me.position(), me.forward());
        }
        let me = me.followed();
        self.chase.follow(&me, self.cam_keys, terrain, dt.clamp(0.0, 0.5))
    }

    /// Bikes that touch bounce off each other (0x405fb0 for type 100):
    /// each bike's collision points are swept against the other's hull,
    /// and a bike comes off only when the other is more than 1.5 ft above
    /// it, landed on (crash 2 of 0x40a950); side by side they just bump.
    /// See `mm2_physics::bump`.
    fn contact(&mut self) {
        let hull = HULL.get().cloned();
        for i in 0..self.racers.len() {
            for j in i + 1..self.racers.len() {
                let Some(hull) = &hull else { break };
                let (a, b) = self.racers.split_at_mut(j);
                let (a, b) = (&mut a[i], &mut b[0]);
                let (Some(was_a), Some(was_b)) = (a.stood, b.stood) else {
                    continue;
                };
                let (Some(sa), Some(sb)) = (a.bike.bump_state(was_a), b.bike.bump_state(was_b))
                else {
                    continue;
                };
                if let Some((ba, bb)) = mm2_physics::bump::bump(hull, &sa, &sb) {
                    a.bike.bumped(&ba);
                    b.bike.bumped(&bb);
                }
            }
        }
        for r in &mut self.racers {
            r.stood = r.bike.stance();
        }
    }

    /// Where the riders are, as Moto-Tag needs them.
    pub fn tag_riders(&self) -> Vec<crate::tag::Rider> {
        self.racers
            .iter()
            .map(|r| crate::tag::Rider {
                place: r.bike.place(),
                velocity: r.bike.velocity(),
                crashed: r.bike.crashed(),
                finished: r.finished.is_some(),
            })
            .collect()
    }

    /// Moto-Tag's step: who is IT, their time, the ball.
    fn tag_step(&mut self, ground: &TrackGround, step: f32) {
        let riders = self.tag_riders();
        let Some(tag) = &mut self.tag else { return };
        let mut points: Vec<f32> = self.racers.iter().map(|r| r.stunt_points).collect();
        let seed = &mut self.seed;
        let random = || {
            *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (*seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let terrain = ground.terrain;
        tag.tick(&riders, &mut points, self.time, step, random, |x, z| {
            crate::ride::ground(terrain, x, z)
        });
        for (r, p) in self.racers.iter_mut().zip(points) {
            r.stunt_points = p;
        }
    }

    /// Keeps each rider's time behind the leader, and once the player
    /// finishes, finishes the computer riders still out as the game does
    /// (`mm2_physics::race::finish_the_rest`).
    fn rank(&mut self) {
        let order = self.standings();
        let mut timings: Vec<Timing> = self.racers.iter().map(|r| r.timing.clone()).collect();
        race::behind_leader(&order, &mut timings, !self.gates.is_empty());
        let me = self.player;
        let player_done = self.racers[me].finished.is_some();
        if player_done && self.laps < ENDLESS {
            let computer: Vec<bool> = (0..self.racers.len()).map(|i| i != me).collect();
            let player_lap = timings[me].lap_time;
            let seed = &mut self.seed;
            race::finish_the_rest(
                &order,
                &mut timings,
                &computer,
                self.laps,
                player_lap,
                || {
                    *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (*seed >> 8) as f32 / (1u32 << 24) as f32
                },
            );
        }
        for (r, t) in self.racers.iter_mut().zip(timings) {
            if t.finished && r.finished.is_none() {
                r.finished = Some(t.race_time);
                r.laps = self.laps;
            }
            r.timing = t;
        }
    }

    /// The Stunt Quarry's clock (0x4eb060): a second before the session's
    /// minutes are up, every rider is finished.
    fn end_session(&mut self) {
        let Some(minutes) = self.minutes else {
            return;
        };
        let clock = self.clock();
        if clock >= minutes * 60.0 - 1.0 {
            if let Some(t) = &mut self.tag {
                if self.racers.iter().any(|r| r.finished.is_none()) {
                    t.end();
                }
            }
            for r in &mut self.racers {
                if r.finished.is_none() {
                    r.finished = Some(clock);
                    r.timing.finished = true;
                }
            }
        }
    }

    /// Seconds left in a timed Stunt Quarry session.
    pub fn time_left(&self) -> Option<f32> {
        let m = self
            .minutes
            .filter(|_| self.stunt_event || self.tag.is_some())?;
        Some((m * 60.0 - self.clock()).max(0.0))
    }

    /// Every racer's index in finishing order: those who finished by
    /// their time, then the rest by how far round they got.
    pub fn standings(&self) -> Vec<usize> {
        let (course, gates) = (self.course.as_ref(), &self.gates);
        let mut order: Vec<usize> = (0..self.racers.len())
            .filter(|&i| Some(i) != self.ghost)
            .collect();
        if let Some(t) = &self.tag {
            let points: Vec<f32> = self.racers.iter().map(|r| r.stunt_points).collect();
            return t.standings(order.into_iter(), &points);
        }
        if self.stunt_event {
            // Most stunt points first (0x45f8c0).
            order.sort_by(|&a, &b| {
                self.racers[b]
                    .stunt_points
                    .total_cmp(&self.racers[a].stunt_points)
            });
            return order;
        }
        order.sort_by(|&a, &b| {
            let (a, b) = (&self.racers[a], &self.racers[b]);
            match (a.finished, b.finished) {
                (Some(x), Some(y)) => x.total_cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => b
                    .progress(course, gates)
                    .total_cmp(&a.progress(course, gates)),
            }
        });
        order
    }

    /// The player's place, from 1.
    pub fn position(&self) -> usize {
        1 + self
            .standings()
            .iter()
            .position(|&i| i == self.player)
            .unwrap_or(0)
    }

    /// A line for the window title: lap, place and times.
    pub fn status(&self) -> String {
        let me = &self.racers[self.player];
        if !self.gate_down() {
            return format!("Get ready... {:.0}", (self.lead - self.time).ceil());
        }
        if let Some(t) = &self.tag {
            let it = match t.it {
                Some(i) if i == self.player => "You are IT".to_string(),
                Some(i) => format!(
                    "{} is IT",
                    self.names
                        .get(i)
                        .cloned()
                        .unwrap_or(format!("Rider {}", i + 1))
                ),
                None => "Nobody is IT".to_string(),
            };
            return format!(
                "{it}  {} of {}  IT {}  left {}",
                ordinal(self.position()),
                self.racers.len(),
                time(t.time_it(self.player)),
                time(self.time_left().unwrap_or(0.0))
            );
        }
        if self.course.is_none() {
            return format!("{}  free ride", time(self.clock()));
        }
        let best = me.lap_times.iter().copied().reduce(f32::min);
        let best = best.map_or(String::new(), |b| format!("  best lap {}", time(b)));
        match me.finished {
            Some(t) => format!(
                "Finished {} of {} in {}{best}",
                ordinal(self.position()),
                self.racers.len() - self.ghost.is_some() as usize,
                time(t)
            ),
            None => format!(
                "Lap {}/{}  {} of {}  {}{best}",
                (me.laps + 1).min(self.laps),
                self.laps,
                ordinal(self.position()),
                self.racers.len() - self.ghost.is_some() as usize,
                time(self.clock())
            ),
        }
    }
}

/// Minutes, seconds and tenths.
pub fn time(t: f32) -> String {
    let tenths = (t * 10.0) as u32;
    format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_formats::tdf::{CrossSection, Node};

    fn def() -> TrackDef {
        // A start straight into a square loop.
        let s = |x: f32, z: f32| CrossSection {
            center: [x, 0.0, z],
            edges: [[x + 10.0, 0.0, z], [x - 10.0, 0.0, z]],
        };
        let line = Line {
            position: [0.0, 0.0, 10.0],
            direction: [0.0, 0.0, 1.0],
            across: [10.0, 0.0, 0.0],
            width: 20.0,
        };
        TrackDef {
            nodes: vec![
                Node {
                    sections: vec![s(0.0, 0.0), s(0.0, 50.0)],
                    links: vec![1],
                    ..Default::default()
                },
                Node {
                    sections: vec![
                        s(0.0, 50.0),
                        s(0.0, 100.0),
                        s(100.0, 100.0),
                        s(100.0, 0.0),
                        s(0.0, 0.0),
                    ],
                    links: vec![1],
                    ..Default::default()
                },
            ],
            start: Some(line),
            finish: Some(line),
        }
    }

    #[test]
    fn course_loops() {
        let c = Course::new(&def()).unwrap();
        assert_eq!(c.points.len(), 7);
        // The straight's end repeats the loop's first point, so it is skipped.
        assert_eq!(c.next[1], 3);
        // The loop's closing point leads back round it.
        assert_eq!(c.lap, 5);
        let p = c.ahead(Vec3::ZERO, 1, 120.0);
        assert!((p - Vec3::new(20.0, 0.0, 100.0)).length() < 1e-3, "{p}");
    }

    /// A trick held to its end scores on landing; one still going when
    /// the bike comes down throws the rider.
    #[test]
    fn finish_line_with_no_direction_counts_either_way() {
        let mut d = def();
        d.finish = d.finish.map(|l| Line {
            direction: [0.0; 3],
            ..l
        });
        let line = Course::new(&d).unwrap().finish_line();
        let (south, north) = (Vec3::new(0.0, 1.0, 5.0), Vec3::new(0.0, 1.0, 15.0));
        assert!(line.crossed(south, north, true));
        assert!(line.crossed(north, south, true));
        // A real direction still only counts one way.
        let line = Course::new(&def()).unwrap().finish_line();
        assert!(line.crossed(south, north, true));
        assert!(!line.crossed(north, south, true));
    }

    #[test]
    fn tricks_score_on_landing() {
        let terrain = mm2_formats::trn::Terrain {
            size: 9,
            heights: vec![0.0; 81],
            normals: vec![[0, 127, 0]; 81],
            surfaces: vec![0; 81],
            scale: 30.0,
            textures: Vec::new(),
            tiles: Vec::new(),
        };
        let world = mm2_physics::collision::World::default();
        let zones = Default::default();
        let ground = TrackGround {
            terrain: &terrain,
            world: &world,
            zones: &zones,
            edge: true,
        };
        let motions: Vec<_> = (0..32)
            .map(|_| {
                Some(TrickMotion {
                    rider: "BarKneel".into(),
                    bike: "bBarKneel".into(),
                    fps: 18.0,
                    frames: 30,
                })
            })
            .collect();
        let mut r = Racer::new(
            &ground,
            0,
            (Vec3::new(100.0, 0.0, 100.0), Vec3::Z),
            1.0,
            None,
        );
        let fly = |r: &mut Racer, air: bool| {
            if let Bike::Computer(b) = &mut r.bike {
                b.rider.airborne = air;
            }
        };
        // Stick forward (the gas) and button 6: BarKneel's slot 4 is
        // HeelClicker, here every trick has the same motion.
        let gas = Controls {
            throttle: 1.0,
            ..Default::default()
        };
        let dt = 1.0 / 60.0;
        fly(&mut r, true);
        assert_eq!(r.tricks(gas, [true, false], &motions, dt, None), None);
        assert_eq!(r.stunt.trick, Some(4));
        for _ in 0..120 {
            r.tricks(gas, [false; 2], &motions, dt, None);
        }
        assert_eq!((r.stunt.trick, r.stunt.tricks), (None, 1));
        fly(&mut r, false);
        let (base, m) = r.tricks(gas, [false; 2], &motions, dt, None).unwrap();
        // HeelClicker's 1.667 s at 2500 a second, to the hundred below.
        assert_eq!((base, m), (4100.0, 1.0));
        assert_eq!(r.stunt_points, 4100.0);

        // Down mid-trick: thrown.
        fly(&mut r, true);
        r.tricks(gas, [false; 2], &motions, dt, None);
        r.tricks(gas, [true, false], &motions, dt, None);
        assert!(r.stunt.busy());
        fly(&mut r, false);
        assert_eq!(r.tricks(gas, [false; 2], &motions, dt, None), None);
        assert!(r.bike.crashed());
    }

    #[test]
    fn times_and_places() {
        assert_eq!(time(83.47), "1:23.4");
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(23), "23rd");
    }
}
