//! The rider's and the bike's riding poses (KrustyBike 0x48f390 picks them
//! each frame, 0x48fe50 plays them through 0x4aa250, 0x4aa2b0 and
//! 0x4aa6b0): Rider.mcf's turn and lean motions mixed by the steer, the
//! rider's weight fore and aft, and the rear tyre's speed.
//!
//! Each turn motion sweeps the bars from the centre to full right and
//! back (its first half), then to full left and back (its second half).
//! The steer picks a phase through it (`f_65c`), and a small state
//! machine (`f_520`) keeps where a turn is: steering in plays the lean-in
//! quarter, and once the bars have passed 0.9 and come back, the return
//! quarter plays instead, so the rider swings back through the centre.
//!
//! The rider's place fore and aft (`f_640`, 0.25 back to 0.75 forward,
//! eased over 0.126 s) picks two neighbouring motions of three (forward,
//! centred, back) and how much of each (`f_650`, `f_654`). At 14 ft/s and
//! more the rider rides the Lean motions (Turn1D–3D); slower, the Turn
//! motions (LeanR01–03, foot out) mix in, all of them at a standstill
//! (`f_660`). In the air with the air pose on, the cross-up motions
//! (CrosOv1–3) play at the air pose's phase instead, and the bike plays
//! CrosBik1–3. Backing up plays BackUpTop at the steer and BackUpBottom,
//! the paddling legs, at the paddle's phase.

/// The rider's motions by the game's index (`f_66c[i]`, set up in
/// 0x4087e0); index 9 is never loaded.
const RIDER: [&str; 17] = [
    "LeanR01", "LeanR02", "LeanR03", "Turn1D", "Turn1U", "Turn2D", "Turn2U", "Turn3D", "Turn3U", "",
    "Wheelie", "CrosOv1", "CrosOv2", "CrosOv3", "LookBack", "Victory", "BackUpTop",
];
/// The bike's (`f_6b4[i]`): the bars' turn, or the cross-up.
const BIKE: [&str; 17] = [
    "BLeanR02", "BLeanR02", "BLeanR02", "BTurn2D", "BTurn2U", "BTurn2D", "BTurn2U", "BTurn2D",
    "BTurn2U", "", "BWheelie", "CrosBik1", "CrosBik2", "CrosBik3", "BLookBack", "", "bBackUpTop",
];
const BACK_UP_BOTTOM: &str = "BackUpBottom";
/// `f_650` while backing up.
const BACKING: usize = 16;
/// The first cross-up row.
const CROSS: usize = 11;

/// Below this rear tyre speed the slow motions mix in, ft/s (0x5558f8),
/// all of them at a standstill (1/14, 0x5558ec).
const SLOW: f32 = 14.0;
/// The fore-aft place's lag, s: 6.3 steps of 0.02 (0x555b64).
const PLACE_LAG: f32 = 0.126;
/// Bars past this are a full turn, whose return plays on the way back
/// (0x5558d8).
const FULL: f32 = 0.9;
/// Bars within this of the centre end the return (0x5558d0).
const CENTRE: f32 = 0.032;
/// The air pose turns on with the front tyre this far clear and the rear
/// 1 ft (0x5557e0, 0x5558c8).
const AIR_FRONT: f32 = 2.0;
const AIR_REAR: f32 = 1.0;

/// What a frame's pose reads from the bike.
#[derive(Debug, Clone, Copy, Default)]
pub struct Inputs {
    /// The bars as the game reads them (`f_504`): −1..1, left positive.
    pub steer: f32,
    /// The rider's weight (`f_508`): −1 (back) to 1 (forward).
    pub lean: f32,
    /// The computer's riders skip the lag (0x414000 sets `f_640` straight
    /// from the lean).
    pub computer: bool,
    pub bike: mm2_physics::bike::PoseInputs,
}

/// A motion to mix: its name, the phase through it (0 to 1), its weight,
/// and its layer (a later layer replaces the frames it moves).
pub type Mix = (&'static str, f32, f32, u8);

/// One rider's pose between frames.
#[derive(Debug, Clone, Copy)]
pub struct Pose {
    /// `f_520`: 1 steering, 2 at full right, 3 coming back from it, 4 at
    /// full left, 5 coming back from that.
    state: u8,
    /// `f_65c`, the phase through the turn motions.
    phase: f32,
    /// `f_640`, the rider's place fore and aft.
    place: f32,
    /// The bars last frame (`f_510`).
    last: f32,
    /// The air pose was on last frame.
    air: bool,
}

impl Default for Pose {
    /// As a reset leaves it (0x408450).
    fn default() -> Self {
        Pose {
            state: 1,
            phase: 0.5,
            place: 0.5,
            last: 0.0,
            air: false,
        }
    }
}

impl Pose {
    /// Moves on by `dt` seconds and returns the rider's and the bike's
    /// mixes, and whether the bike is drawn crossed up by its motion
    /// (rather than by the physics' turn).
    pub fn update(&mut self, i: &Inputs, dt: f32) -> (Vec<Mix>, Vec<Mix>) {
        // The weight's lag (0x406ee0, every step).
        let target = 0.5 + 0.25 * i.lean.clamp(-1.0, 1.0);
        if i.computer {
            self.place = target;
        } else {
            self.place += (target - self.place) * dt.min(PLACE_LAG) / PLACE_LAG;
        }
        let b = &i.bike;
        let s = i.steer;
        let x = (0.75 - self.place) * 3.99;
        // round(x − 0.5) in the FPU's round-to-even.
        let row = (x - 0.5).round_ties_even().max(0.0) as usize;
        let front = if b.airborne || b.rear_down { -b.depth[0] } else { 0.0 };
        let clear = b.airborne && front > AIR_FRONT && b.depth[1] < -AIR_REAR;
        let (rows, weight, slow);
        match b.air_pose.filter(|_| clear) {
            Some(p) => {
                // In the air: the cross-ups, at the air pose's phase.
                self.air = true;
                self.phase = p;
                rows = (row + CROSS, (row + CROSS + 1).min(CROSS + 2));
                weight = x - row as f32;
                slow = None;
            }
            None => {
                if self.air {
                    // The pose ended (0x40d9d0): carry on from the bars.
                    self.air = false;
                    self.state = resume(s);
                }
                if let Some(paddle) = b.paddle.filter(|_| b.rear_speed <= SLOW) {
                    self.steer(s);
                    self.last = s;
                    let rider = vec![
                        (RIDER[BACKING], self.phase, 1.0, 0),
                        (BACK_UP_BOTTOM, paddle, 1.0, 1),
                    ];
                    return (rider, vec![(BIKE[BACKING], self.phase, 1.0, 0)]);
                }
                self.steer(s);
                if b.rear_speed <= SLOW {
                    // LeanR rows, with the Turn D rows mixed in by speed.
                    let d = row * 2 + 3;
                    rows = (row, row + 1);
                    weight = x - row as f32;
                    slow = Some((d, (d + 2).min(8), (SLOW - b.rear_speed) / SLOW));
                } else {
                    // Turn{row+1}D and the next.
                    let r = row * 2 + 3;
                    rows = (r, r + 2);
                    // Fully back (out of reach, as x stays under 2) the
                    // speed would pick between the last two.
                    weight = match row {
                        2 if b.rear_speed <= 32.0 => (b.rear_speed - SLOW) * 0.0555,
                        _ => x - row as f32,
                    };
                    slow = None;
                }
            }
        }
        self.last = s;
        let w = weight.clamp(0.0, 1.0);
        let p = self.phase;
        let pair = |names: &[&'static str; 17], scale: f32| {
            [
                (names[rows.0], p, (1.0 - w) * scale, 0),
                (names[rows.1], p, w * scale, 0),
            ]
        };
        match slow {
            Some((c, d, g)) => {
                // 0x4aa6b0: (A, B) and (C, D) by the weight, then the two
                // by 1 − g.
                let g = g.clamp(0.0, 1.0);
                let mut rider = pair(&RIDER, g).to_vec();
                rider.push((RIDER[c], p, (1.0 - w) * (1.0 - g), 0));
                rider.push((RIDER[d], p, w * (1.0 - g), 0));
                let mut bike = pair(&BIKE, g).to_vec();
                bike.push((BIKE[c], p, (1.0 - w) * (1.0 - g), 0));
                bike.push((BIKE[d], p, w * (1.0 - g), 0));
                (rider, bike)
            }
            None => (pair(&RIDER, 1.0).to_vec(), pair(&BIKE, 1.0).to_vec()),
        }
    }

    /// Whether the bike's cross-up motion is playing: the bike is then
    /// drawn turned by it, not by the physics.
    pub fn crossed(&self) -> bool {
        self.air
    }

    /// The bars' phase through the turn motions on the ground (0x48f390's
    /// `f_520` switch).
    fn steer(&mut self, s: f32) {
        // The lean-in quarters: right 0 to 0.25, left 0.5 to 0.75.
        let right = || (-0.25 * s).clamp(0.0, 0.25);
        let left = || (0.25 * s + 0.5).clamp(0.5, 0.75);
        self.phase = match self.state {
            1 => {
                if s >= 0.0 {
                    if s > FULL {
                        self.state = 4;
                    }
                    left()
                } else {
                    if s < -FULL {
                        self.state = 2;
                    }
                    right()
                }
            }
            2 => {
                if self.last < s {
                    self.state = 3;
                    (0.25 * s + 0.5).clamp(0.25, 0.5)
                } else {
                    right()
                }
            }
            3 => {
                if s <= -CENTRE {
                    (0.25 * s + 0.5).clamp(0.25, 0.5)
                } else {
                    self.state = 1;
                    if s >= 0.0 {
                        left()
                    } else {
                        right()
                    }
                }
            }
            4 => {
                if s < self.last {
                    self.state = 5;
                    (1.0 - 0.25 * s).clamp(0.75, 1.0)
                } else {
                    left()
                }
            }
            _ => {
                if s >= CENTRE {
                    (1.0 - 0.25 * s).clamp(0.75, 1.0)
                } else {
                    self.state = 1;
                    // As the game has it: the centre either way.
                    if s < 0.0 {
                        0.5
                    } else {
                        right()
                    }
                }
            }
        };
    }
}

/// The steer state the bars leave when the air pose ends (0x40d9d0).
fn resume(s: f32) -> u8 {
    if s > 0.0 {
        if s > FULL {
            4
        } else if s >= CENTRE {
            5
        } else {
            1
        }
    } else if s >= -FULL {
        if s <= -CENTRE {
            3
        } else {
            1
        }
    } else {
        2
    }
}

/// Taunting (action 9, "Taunting", Space from the factory): in the air
/// with the air pose on, the rider looks round at the nearest rider
/// (0x48f230 starts it, 0x48fe50 runs it). LookBack and bLookBack play at
/// a phase 0.5 ± the turn of the head: it turns toward the rider, follows
/// it while it is near, holds 0.24 s and turns back. It counts as a trick
/// (`f_430`), so landing before it is over throws the rider.
#[derive(Debug, Clone, Copy)]
pub struct Taunt {
    /// The rider looked at.
    pub target: usize,
    /// Where the rider is, from the bike's heading (`f_1558`), rad.
    angle: f32,
    /// The head's goal and where it is, in its own units (`f_1550`,
    /// `f_1554`); in the hold, the hold's time left.
    aim: f32,
    at: f32,
    turning: bool,
    settling: bool,
}

/// Turns of the head, in its units per radian of the rider's angle
/// (0x559538 × 0x55584c), and phase per unit (0x559554).
const HEAD: f32 = 0.369_63 * 0.7;
const HEAD_PHASE: f32 = 0.71;
/// The farthest the head turns, rad (0x559540).
const HEAD_MAX: f32 = 2.7;
/// Riders nearer than this to straight ahead are not looked at, rad
/// (0x559544).
const TAUNT_MIN: f32 = 0.698;
const TAUNT_HOLD: f32 = 0.24;

impl Taunt {
    /// Starts looking at rider `target`, `angle` from the bike's heading,
    /// unless it is ahead.
    pub fn start(target: usize, angle: f32) -> Option<Taunt> {
        if angle.abs() <= TAUNT_MIN {
            return None;
        }
        let (angle, aim) = if angle > HEAD_MAX {
            (HEAD_MAX, 0.7)
        } else if angle < -HEAD_MAX {
            (-HEAD_MAX, -0.7)
        } else {
            (angle, angle * HEAD)
        };
        Some(Taunt {
            target,
            angle,
            aim,
            at: 0.0,
            turning: true,
            settling: false,
        })
    }

    /// The phase through LookBack this frame, with the target `angle` from
    /// the heading now; `None` once it is over.
    pub fn update(&mut self, dt: f32, angle: f32) -> Option<f32> {
        let phase = |u: f32| Some(u * HEAD_PHASE + 0.5);
        if self.settling {
            // Back to the front.
            if self.aim >= 0.0 {
                self.at -= dt;
                if self.at >= 0.0 {
                    return phase(self.at);
                }
            } else {
                self.at += dt;
                if self.at <= 0.0 {
                    return phase(self.at);
                }
            }
            return None;
        }
        if !self.turning {
            // The hold.
            self.at -= dt;
            if self.at < 0.0 {
                let u = self.angle * HEAD;
                self.settling = true;
                self.at = u;
                self.aim = u;
                return phase(u);
            }
            return phase(self.angle * HEAD);
        }
        let toward = self.aim.signum();
        if self.aim != 0.0 {
            self.at += dt * toward;
            if (self.aim - self.at) * toward >= 0.0 {
                if (self.aim - self.at) * toward >= 0.1 {
                    return phase(self.at);
                }
                // Near the goal: follow the rider.
                let u = angle * HEAD;
                if (angle - self.angle) * toward >= 0.0 {
                    if angle.abs() <= HEAD_MAX {
                        self.angle = angle;
                        self.aim = u;
                    } else {
                        self.angle = HEAD_MAX * toward;
                        self.aim = 0.7 * toward;
                    }
                    return phase(self.at);
                }
                if (u - self.at) * toward >= 0.0 {
                    self.angle = angle;
                    self.aim = u;
                    return phase(self.at);
                }
                // It has gone past the head: hold where the head is.
                self.angle = self.at / HEAD;
                self.at = TAUNT_HOLD;
                self.turning = false;
                return phase(self.at);
            }
        }
        self.turning = false;
        self.at = TAUNT_HOLD;
        phase(self.angle * HEAD)
    }
}

/// The angle from a bike at `from` heading along `forward` to `to`, as the
/// game measures it (atan2 of x over z, less the heading), −π..π.
pub fn angle_to(from: glam::Vec3, forward: glam::Vec3, to: glam::Vec3) -> f32 {
    use std::f32::consts::{PI, TAU};
    let d = to - from;
    let mut a = d.x.atan2(d.z) - forward.x.atan2(forward.z);
    if a < -PI {
        a += TAU;
    } else if a > PI {
        a -= TAU;
    }
    a
}

/// The rider's and the bike's motions while taunting.
pub fn taunting(phase: f32) -> (Vec<Mix>, Vec<Mix>) {
    (vec![(RIDER[14], phase, 1.0, 0)], vec![(BIKE[14], phase, 1.0, 0)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_physics::bike::PoseInputs;

    fn riding(speed: f32) -> Inputs {
        Inputs {
            bike: PoseInputs {
                rear_down: true,
                rear_speed: speed,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn main(mix: &[Mix]) -> Mix {
        *mix.iter().max_by(|a, b| a.2.total_cmp(&b.2)).unwrap()
    }

    #[test]
    fn centred_rider_at_speed() {
        let mut pose = Pose::default();
        let (rider, bike) = pose.update(&riding(50.0), 0.02);
        let m = main(&rider);
        assert_eq!((m.0, m.1), ("Turn2D", 0.5));
        assert!(m.2 > 0.99);
        assert_eq!(main(&bike).0, "BTurn2D");
    }

    #[test]
    fn standstill_rides_the_slow_motions() {
        let mut pose = Pose::default();
        let (rider, bike) = pose.update(&riding(0.0), 0.02);
        assert_eq!(main(&rider).0, "LeanR02");
        assert_eq!(main(&bike).0, "BLeanR02");
        // Halfway to 14 ft/s, half each.
        let (rider, _) = pose.update(&riding(7.0), 0.02);
        let lean: f32 = rider.iter().filter(|m| m.0 == "LeanR02").map(|m| m.2).sum();
        assert!((lean - 0.5).abs() < 0.01, "{rider:?}");
    }

    #[test]
    fn weight_back_eases_in() {
        let mut pose = Pose::default();
        let mut i = riding(50.0);
        i.lean = -1.0;
        let (rider, _) = pose.update(&i, 0.02);
        assert_eq!(main(&rider).0, "Turn2D");
        for _ in 0..20 {
            pose.update(&i, 0.02);
        }
        let (rider, _) = pose.update(&i, 0.02);
        assert_eq!(main(&rider).0, "Turn3D");
    }

    #[test]
    fn full_turn_returns_through_the_far_quarter() {
        let mut pose = Pose::default();
        let mut i = riding(50.0);
        // Full left: the lean-in quarter to 0.75.
        i.steer = 1.0;
        let (rider, _) = pose.update(&i, 0.02);
        assert_eq!(rider[0].1, 0.75);
        // Letting go plays the return quarter, 0.75 to 1.
        i.steer = 0.5;
        let (rider, _) = pose.update(&i, 0.02);
        assert!((rider[0].1 - 0.875).abs() < 1e-6);
        // A small turn stays on the lean-in quarter.
        let mut pose = Pose::default();
        i.steer = 0.5;
        pose.update(&i, 0.02);
        i.steer = 0.25;
        let (rider, _) = pose.update(&i, 0.02);
        assert!((rider[0].1 - 0.5625).abs() < 1e-6);
    }

    #[test]
    fn air_pose_plays_the_cross_ups() {
        let mut pose = Pose::default();
        let mut i = riding(50.0);
        i.bike.airborne = true;
        i.bike.depth = [-3.0, -3.0];
        i.bike.air_pose = Some(0.25);
        let (rider, bike) = pose.update(&i, 0.02);
        assert_eq!((main(&rider).0, main(&rider).1), ("CrosOv2", 0.25));
        assert_eq!(main(&bike).0, "CrosBik2");
        assert!(pose.crossed());
    }

    #[test]
    fn backing_up_paddles() {
        let mut pose = Pose::default();
        let mut i = riding(-2.0);
        i.bike.paddle = Some(0.3);
        let (rider, _) = pose.update(&i, 0.02);
        assert_eq!(rider[0].0, "BackUpTop");
        assert_eq!((rider[1].0, rider[1].1, rider[1].3), ("BackUpBottom", 0.3, 1));
    }
    #[test]
    fn taunt_turns_holds_and_comes_back() {
        assert!(Taunt::start(0, 0.5).is_none());
        let mut t = Taunt::start(0, 1.5).unwrap();
        let mut last = 0.5;
        let mut phases = Vec::new();
        while let Some(p) = t.update(0.02, 1.5) {
            phases.push(p);
            last = p;
            assert!(phases.len() < 500);
        }
        let top = phases.iter().cloned().fold(0.0, f32::max);
        // 1.5 rad round: 0.5 + 1.5 × 0.2587 × 0.71.
        assert!((top - (0.5 + 1.5 * HEAD * HEAD_PHASE)).abs() < 0.02, "{top}");
        assert!((last - 0.5).abs() < 0.02, "{last}");
        // Turning, holding 0.24 s and back: about 0.39 + 0.24 + 0.39 s.
        assert!((45..=60).contains(&phases.len()), "{}", phases.len());
    }

    #[test]
    fn taunt_looks_the_other_way_too() {
        let mut t = Taunt::start(0, -2.0).unwrap();
        let low = (0..30).filter_map(|_| t.update(0.02, -2.0)).fold(1.0, f32::min);
        assert!(low < 0.5 - 0.3, "{low}");
    }

    #[test]
    fn angle_to_a_rider_behind_on_the_left() {
        let a = angle_to(glam::Vec3::ZERO, glam::Vec3::Z, glam::Vec3::new(1.0, 0.0, -1.0));
        assert!((a - 2.356).abs() < 1e-3);
    }
}
