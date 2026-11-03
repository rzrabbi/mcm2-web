//! Moto-Tag (`MotoTag.cpp`): one rider is IT, and touching the IT rider
//! takes it from them; whoever is IT longest when the minutes run out
//! wins. With the ball (`ChkTagBall`) IT carries a beach ball, drops it
//! in a crash, and anyone who touches the loose ball is IT. With stunt
//! mode (`ChkStuntMode`) the riders are ranked by stunt points instead,
//! and being IT earns them.
//!
//! What the game does:
//! * at the start (0x4ade20) IT is picked at random among the riders; with
//!   the ball nobody is, and the ball waits to be picked up (0x4ad900);
//! * each frame (0x4adf80) the five-second hold-off after a tag runs down,
//!   the IT rider's stint grows and, in stunt mode, IT scores 50 points a
//!   second; the IT sign (`Itind.slt`) stands 4.25 above the IT bike, and
//!   the ball rides 4.2 up and 2.5 behind its rider;
//! * a rider touching the IT bike, or the loose ball (0x48ea00), asks the
//!   host for IT (packet 0x87 + 0x8c), who agrees unless the hold-off is
//!   still running, and nobody holds a loose ball off (0x4ae210);
//! * IT changing hands (0x4ae360) adds the last IT's stint to their total
//!   and keeps their longest, starts the hold-off, gives the new IT 2500
//!   stunt points when they took it from someone, and the host tells
//!   everyone (packet 0x87);
//! * when the minutes are up (0x45feb0) the last stint is added in, and
//!   the riders are ranked by time as IT, most first (0x45f990).

use glam::{Mat4, Vec3};

/// The hold-off after IT changes hands, in seconds (+0xac, 5.0).
pub const HOLD_OFF: f32 = 5.0;
/// Stunt points for taking IT from someone (2500.0 at 0x4ae360), and IT's
/// points each second (50.0 at 0x5597d4, every 1.0 s at 0x555748).
pub const TAKEN_POINTS: f32 = 2500.0;
pub const POINTS_EACH_SECOND: f32 = 50.0;
/// The IT sign's height above the bike (0x555fe0).
pub const SIGN_HEIGHT: f32 = 4.25;
/// Where the carried ball rides, in the bike's frame (0x4ae192).
pub const CARRY: Vec3 = Vec3::new(0.0, 4.2, -2.5);
/// The signs turn about the upright, in degrees a second (0x41a01c's
/// 0x42960000), at the model's size (`0x401310`'s 2.0).
const SIGN_SPIN: f32 = 75.0;
const SIGN_SIZE: f32 = 2.0;
/// The ball's radius: `BBHighLod.slb` is one unit across each way; the
/// game's ball is bigger than a helmet, so our pick is this size.
pub const BALL_RADIUS: f32 = 1.5;
/// A loose ball falls, bounces back with this much of its speed (0.75 in
/// the ball's setup call) and rolls to a stop on the ground.
const GRAVITY: f32 = 32.2;
const BOUNCE: f32 = 0.75;
const ROLL_DRAG: f32 = 0.8;
/// How close two bikes, or a bike and the ball, are to touch. The game
/// tests its collision shapes; ours are the race's contact sizes.
const TOUCH_GAP: f32 = 3.0;
const TOUCH_HEIGHT: f32 = 3.0;
/// A claim waits this long before it is asked again.
const CLAIM_WAIT: f32 = 0.5;

/// The beach ball.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ball {
    pub position: Vec3,
    pub velocity: Vec3,
}

/// Where one rider stands, as the tag rules need it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rider {
    pub place: Mat4,
    pub velocity: Vec3,
    pub crashed: bool,
    pub finished: bool,
}

impl Rider {
    pub fn position(&self) -> Vec3 {
        self.place.w_axis.truncate()
    }

    /// The bike's centre of mass (bike +0x18), which the signs stand over
    /// (0x4adf80, 0x41e0b0); the model's origin is on the ground.
    pub fn centre(&self) -> Vec3 {
        self.place.transform_point3(mm2_physics::bike::COM)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub it: Option<usize>,
    /// Seconds before IT can be taken again.
    pub hold_off: f32,
    /// Each rider's time as IT: this time (+0x75c), their longest go
    /// (+0x760) and all told (+0x764).
    pub stint: Vec<f32>,
    pub longest: Vec<f32>,
    pub total: Vec<f32>,
    /// Ranked by stunt points, which IT earns.
    pub stunts: bool,
    pub ball: Option<Ball>,
    /// The riders whose touches this machine reports: all of them, off the
    /// network; a player's own in a network game.
    pub local: Vec<bool>,
    /// Whether this machine decides who is IT: the host, or everyone off
    /// the network.
    pub judge: bool,
    /// Claims for IT to send the judge (none: IT dropped the ball), and IT
    /// changes the judge made, to tell everyone.
    pub claims: Vec<(usize, Option<usize>)>,
    pub changes: Vec<Option<usize>>,
    /// When IT last changed hands, in race time.
    pub passed: Option<f32>,
    points_clock: f32,
    claim_wait: f32,
    started: bool,
}

impl Tag {
    pub fn new(riders: usize, ball: bool, stunts: bool) -> Tag {
        Tag {
            it: None,
            hold_off: 0.0,
            stint: vec![0.0; riders],
            longest: vec![0.0; riders],
            total: vec![0.0; riders],
            stunts,
            ball: ball.then_some(Ball {
                position: Vec3::ZERO,
                velocity: Vec3::ZERO,
            }),
            local: vec![true; riders],
            judge: true,
            claims: Vec::new(),
            changes: Vec::new(),
            passed: None,
            points_clock: 0.0,
            claim_wait: 0.0,
            started: false,
        }
    }

    /// Time as IT so far, the stint under way counted.
    pub fn time_it(&self, i: usize) -> f32 {
        self.total.get(i).copied().unwrap_or(0.0) + self.stint.get(i).copied().unwrap_or(0.0)
    }

    /// The rider's longest go as IT, the one under way counted.
    pub fn longest_it(&self, i: usize) -> f32 {
        let l = self.longest.get(i).copied().unwrap_or(0.0);
        l.max(self.stint.get(i).copied().unwrap_or(0.0))
    }

    /// IT changes hands (0x4ae360): `points` are the riders' stunt points.
    pub fn pass(&mut self, to: Option<usize>, time: f32, points: &mut [f32]) {
        let from = self.it;
        if let Some(f) = from.filter(|&f| f < self.stint.len()) {
            self.total[f] += self.stint[f];
            self.longest[f] = self.longest[f].max(self.stint[f]);
            self.stint[f] = 0.0;
        }
        self.hold_off = HOLD_OFF;
        if let (true, Some(t), Some(f)) = (self.stunts, to, from) {
            if t != f {
                if let Some(p) = points.get_mut(t) {
                    *p += TAKEN_POINTS;
                }
            }
        }
        self.it = to;
        self.passed = Some(time);
    }

    /// The judge hears a claim (0x4ae210): `who` wants IT, or with none
    /// IT has dropped the ball. Whether it stands.
    pub fn claim(
        &mut self,
        who: usize,
        wants: Option<usize>,
        time: f32,
        points: &mut [f32],
    ) -> bool {
        let ok = match wants {
            // Only the rider who has the ball can drop it.
            None => self.ball.is_some() && self.it == Some(who),
            Some(w) => {
                w == who && self.it != Some(w) && (self.it.is_none() || self.hold_off <= 0.0)
            }
        };
        if ok {
            self.pass(wants, time, points);
            self.changes.push(wants);
        }
        ok
    }

    /// The race is over (0x45feb0): the last stint goes in.
    pub fn end(&mut self) {
        if let Some(i) = self.it.filter(|&i| i < self.stint.len()) {
            self.total[i] += self.stint[i];
            self.longest[i] = self.longest[i].max(self.stint[i]);
            self.stint[i] = 0.0;
        }
    }

    /// One step (0x4adf80, 0x48ea00). `random` gives a number in 0..1, for
    /// the first IT; `ground` the height of the ground at a spot.
    pub fn tick(
        &mut self,
        riders: &[Rider],
        points: &mut [f32],
        time: f32,
        dt: f32,
        random: impl FnOnce() -> f32,
        ground: impl Fn(f32, f32) -> f32,
    ) {
        if riders.is_empty() {
            return;
        }
        if !self.started {
            self.started = true;
            match &mut self.ball {
                Some(b) => {
                    // The ball drops in among the riders.
                    let n = riders.len() as f32;
                    let mid = riders.iter().map(|r| r.position()).sum::<Vec3>() / n;
                    b.position = mid + Vec3::Y * 10.0;
                    b.velocity = Vec3::ZERO;
                }
                None if self.judge => {
                    let pick = ((random() * riders.len() as f32) as usize).min(riders.len() - 1);
                    self.it = Some(pick);
                    self.passed = Some(time);
                    self.changes.push(Some(pick));
                }
                None => {}
            }
        }
        self.hold_off = (self.hold_off - dt).max(0.0);
        self.claim_wait = (self.claim_wait - dt).max(0.0);
        if let Some(i) = self.it.filter(|&i| i < riders.len()) {
            if !riders[i].finished {
                self.stint[i] += dt;
                if self.stunts {
                    self.points_clock += dt;
                    if self.points_clock > 1.0 {
                        if let Some(p) = points.get_mut(i) {
                            *p += POINTS_EACH_SECOND;
                        }
                        self.points_clock = 0.0;
                    }
                }
            }
        }
        // Who is touching IT, or the loose ball.
        let mut asks = Vec::new();
        for (j, r) in riders.iter().enumerate() {
            if !self.local.get(j).copied().unwrap_or(false) || r.crashed || r.finished {
                continue;
            }
            if self.it == Some(j) {
                continue;
            }
            let touching = match (self.it, &self.ball) {
                (Some(i), _) => riders
                    .get(i)
                    .is_some_and(|it| touches(r.position(), it.position(), 0.0)),
                (None, Some(b)) => touches(r.position(), b.position, BALL_RADIUS),
                (None, None) => false,
            };
            if touching {
                asks.push((j, Some(j)));
            }
        }
        if let Some(i) = self.it.filter(|&i| i < riders.len()) {
            let r = &riders[i];
            if r.crashed && self.ball.is_some() && self.local.get(i).copied().unwrap_or(false) {
                asks.push((i, None));
            }
        }
        for (who, wants) in asks {
            if self.judge {
                self.claim(who, wants, time, points);
            } else if self.claim_wait <= 0.0 {
                self.claims.push((who, wants));
                self.claim_wait = CLAIM_WAIT;
            }
        }
        self.move_ball(riders, dt, &ground);
    }

    /// The ball with its rider, or loose: falling, bouncing and rolling.
    fn move_ball(&mut self, riders: &[Rider], dt: f32, ground: &impl Fn(f32, f32) -> f32) {
        let it = self.it.and_then(|i| riders.get(i));
        let Some(b) = &mut self.ball else { return };
        if let Some(r) = it {
            b.position = r.place.transform_point3(CARRY);
            b.velocity = r.velocity;
            return;
        }
        b.velocity.y -= GRAVITY * dt;
        b.position += b.velocity * dt;
        let (x, z) = (b.position.x, b.position.z);
        let floor = ground(x, z) + BALL_RADIUS;
        if b.position.y < floor {
            b.position.y = floor;
            // The ground's slope, from the heights about the ball.
            let e = 1.0;
            let n = Vec3::new(
                ground(x - e, z) - ground(x + e, z),
                2.0 * e,
                ground(x, z - e) - ground(x, z + e),
            )
            .normalize_or(Vec3::Y);
            let into = b.velocity.dot(n);
            if into < 0.0 {
                b.velocity -= n * into * (1.0 + BOUNCE);
            }
            let along = b.velocity - n * b.velocity.dot(n);
            b.velocity -= along * (ROLL_DRAG * dt).min(1.0);
        }
    }

    /// The riders by time as IT, most first; in stunt mode by points.
    pub fn standings(&self, riders: impl Iterator<Item = usize>, points: &[f32]) -> Vec<usize> {
        let mut order: Vec<usize> = riders.collect();
        if self.stunts {
            order.sort_by(|&a, &b| {
                let p = |i: usize| points.get(i).copied().unwrap_or(0.0);
                p(b).total_cmp(&p(a))
            });
        } else {
            order.sort_by(|&a, &b| self.time_it(b).total_cmp(&self.time_it(a)));
        }
        order
    }

    /// Where the IT sign, the ball and the player's own sign go, at race
    /// time `time`; hidden ones get a zero matrix. The player's sign shows
    /// in a network game when they are not IT.
    pub fn places(&self, riders: &[Rider], me: Option<usize>, time: f32) -> [Mat4; 3] {
        let hidden = Mat4::from_cols(
            glam::Vec4::ZERO,
            glam::Vec4::ZERO,
            glam::Vec4::ZERO,
            glam::Vec4::W,
        );
        let spin = glam::Quat::from_rotation_y((time * SIGN_SPIN).to_radians());
        let over = |r: &Rider| {
            Mat4::from_scale_rotation_translation(
                Vec3::splat(SIGN_SIZE),
                spin,
                r.centre() + Vec3::Y * SIGN_HEIGHT,
            )
        };
        let it = self.it.and_then(|i| riders.get(i));
        // With the ball, the ball shows who is IT.
        let sign = match (it, &self.ball) {
            (Some(r), None) => over(r),
            _ => hidden,
        };
        let ball = match &self.ball {
            Some(b) => Mat4::from_scale_rotation_translation(
                Vec3::splat(BALL_RADIUS),
                glam::Quat::IDENTITY,
                b.position,
            ),
            None => hidden,
        };
        let mine = match me.and_then(|m| riders.get(m).map(|r| (m, r))) {
            Some((m, r)) if self.it != Some(m) => own_sign(r, time),
            _ => hidden,
        };
        [sign, ball, mine]
    }
}

/// The player's own sign over their bike (`youind.slt`), turning, at
/// 0.6 of the model's size (0x418cd0's 0x3f19999a).
pub fn own_sign(r: &Rider, time: f32) -> Mat4 {
    let spin = glam::Quat::from_rotation_y((time * SIGN_SPIN).to_radians());
    Mat4::from_scale_rotation_translation(
        Vec3::splat(SIGN_SIZE * 0.6),
        spin,
        r.centre() + Vec3::Y * SIGN_HEIGHT,
    )
}

/// Whether a bike at `a` touches a thing at `b` this big.
fn touches(a: Vec3, b: Vec3, radius: f32) -> bool {
    let d = a - b;
    Vec3::new(d.x, 0.0, d.z).length() < TOUCH_GAP + radius && d.y.abs() < TOUCH_HEIGHT + radius
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rider(x: f32) -> Rider {
        Rider {
            place: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
            velocity: Vec3::ZERO,
            crashed: false,
            finished: false,
        }
    }

    #[test]
    fn it_passes_on_a_touch_after_the_hold_off() {
        let mut t = Tag::new(2, false, false);
        let mut points = [0.0; 2];
        let mut riders = [rider(0.0), rider(100.0)];
        // Rider 1 is picked.
        t.tick(&riders, &mut points, 0.0, 0.1, || 0.9, |_, _| 0.0);
        assert_eq!(t.it, Some(1));
        assert_eq!(t.changes, [Some(1)]);
        for k in 0..10 {
            t.tick(
                &riders,
                &mut points,
                0.1 * k as f32,
                0.1,
                || 0.0,
                |_, _| 0.0,
            );
        }
        assert!((t.stint[1] - 1.1).abs() < 1e-4);
        // Rider 0 rides up and touches IT: it is theirs, once IT has
        // been held off long enough.
        t.hold_off = 1.0;
        riders[0] = rider(98.0);
        t.tick(&riders, &mut points, 2.0, 0.1, || 0.0, |_, _| 0.0);
        assert_eq!(t.it, Some(1));
        t.hold_off = 0.0;
        t.tick(&riders, &mut points, 2.1, 0.1, || 0.0, |_, _| 0.0);
        assert_eq!(t.it, Some(0));
        assert_eq!(t.stint[1], 0.0);
        assert!((t.total[1] - 1.3).abs() < 1e-4);
        assert_eq!(t.hold_off, HOLD_OFF);
        // And rider 1 cannot take it straight back.
        t.tick(&riders, &mut points, 2.2, 0.1, || 0.0, |_, _| 0.0);
        assert_eq!(t.it, Some(0));
        t.end();
        assert_eq!(t.standings(0..2, &points), [1, 0]);
    }

    #[test]
    fn stunt_mode_pays_it() {
        let mut t = Tag::new(2, false, true);
        let mut points = [0.0; 2];
        let riders = [rider(0.0), rider(100.0)];
        t.tick(&riders, &mut points, 0.0, 0.1, || 0.0, |_, _| 0.0);
        assert_eq!(t.it, Some(0));
        for _ in 0..25 {
            t.tick(&riders, &mut points, 0.0, 0.1, || 0.0, |_, _| 0.0);
        }
        assert_eq!(points[0], 2.0 * POINTS_EACH_SECOND);
        t.claim(1, Some(1), 3.0, &mut points);
        assert_eq!(points[1], TAKEN_POINTS);
        assert_eq!(t.standings(0..2, &points), [1, 0]);
    }

    #[test]
    fn the_ball_is_picked_up_and_dropped() {
        let mut t = Tag::new(2, true, false);
        let mut points = [0.0; 2];
        let mut riders = [rider(0.0), rider(1000.0)];
        t.tick(&riders, &mut points, 0.0, 0.01, || 0.0, |_, _| 0.0);
        assert_eq!(t.it, None);
        // It falls among the riders and settles; rider 0 rides over.
        for _ in 0..1500 {
            t.tick(&riders, &mut points, 0.0, 0.01, || 0.0, |_, _| 0.0);
        }
        let b = t.ball.unwrap();
        assert!((b.position.y - BALL_RADIUS).abs() < 0.5, "{b:?}");
        riders[0] = rider(b.position.x);
        t.hold_off = 3.0;
        t.tick(&riders, &mut points, 1.0, 0.01, || 0.0, |_, _| 0.0);
        // A loose ball has no hold-off.
        assert_eq!(t.it, Some(0));
        let carried = t.ball.unwrap().position;
        assert!((carried - riders[0].position() - CARRY).length() < 1e-3);
        // A crash drops it.
        riders[0].crashed = true;
        t.tick(&riders, &mut points, 1.1, 0.01, || 0.0, |_, _| 0.0);
        assert_eq!(t.it, None);
        assert_eq!(t.changes.last(), Some(&None));
    }

    #[test]
    fn a_player_asks_the_host() {
        let mut t = Tag::new(2, false, false);
        t.judge = false;
        t.local = vec![false, true];
        let mut points = [0.0; 2];
        let riders = [rider(0.0), rider(1.0)];
        t.tick(&riders, &mut points, 0.0, 0.1, || 0.0, |_, _| 0.0);
        // Not the judge: nobody is IT until the host says.
        assert_eq!(t.it, None);
        t.pass(Some(0), 0.0, &mut points);
        t.hold_off = 0.0;
        t.tick(&riders, &mut points, 0.1, 0.1, || 0.0, |_, _| 0.0);
        assert_eq!(t.claims, [(1, Some(1))]);
        assert_eq!(t.it, Some(0));
    }
}
