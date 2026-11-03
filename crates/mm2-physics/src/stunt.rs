//! Stunts: which trick a button and stick pick, how long a trick scores,
//! the points and multiplier on landing, and the prize money stunts earn
//! in a paying event (KrustyBike 0x48e8c0, 0x48ff..0x4908b6, 0x4967d0,
//! 0x496fa0).
//!
//! The rider and bike animations themselves belong to the caller: this
//! needs only the rider animation's current frame each tick.

/// The rider's tricks in the game's order (rider motions at +0x1420, the
/// bike's at +0x1460). Button 6 picks 0..7 by stick direction, button 7
/// picks 8..15.
pub const TRICKS: [&str; 16] = [
    "BarKneel",
    "SplitX",
    "BKahuna",
    "Cordova",
    "HeelClicker",
    "NacNac",
    "SuperMan",
    "AirWalk",
    "BarHop",
    "SeatGrab",
    "SaranWrap",
    "CliffHanger",
    "HeartAttack",
    "TailGrab",
    "LazyBoy",
    "DCanCan",
];

/// Tricks with an Alt variant (`<name>Alt`, index + 16).
pub const HAS_ALT: [bool; 16] = [
    true, false, false, false, true, true, true, false, false, false, true, false, false, false,
    false, true,
];

/// How long each trick's time counts before it stops growing, s (0x572ce0).
pub const TRICK_TIME: [f32; 16] = [
    1.611, 1.3, 2.267, 2.333, 1.667, 1.5, 2.333, 2.4, 1.722, 1.8, 1.765, 2.333, 2.133, 2.133,
    1.938, 1.722,
];

/// The rider frame each trick holds at (0x5730e0): holding a trick button
/// while the frame is strictly between this and this + 3 scores extra time.
pub const HOLD_FRAME: [i32; 16] = [
    14, 16, 17, 18, 11, 14, 17, 18, 15, 17, 15, 18, 15, 15, 17, 16,
];

/// The one Alt hold window that falls inside the table: the game reads
/// past the end for Alts, and only BarKneelAlt (index 16) finds a usable
/// frame there (1). The others never hold.
pub const BAR_KNEEL_ALT_HOLD_FRAME: i32 = 1;

/// Points per second of trick time (0x4967d0).
pub const POINTS_PER_SECOND: f32 = 2500.0;

/// The stick direction 0..7 the game reads for tricks (0x52cbf0), from the
/// lean (`f_508`) and the raw steer (`f_474`), or `None` while the stick is
/// within 0.5 of centre. Each axis counts when it is over 0.52057 of the
/// stick's length:
///
/// | | steer + | steer 0 | steer − |
/// |---|---|---|---|
/// | lean + | 3 | 4 | 0 |
/// | lean 0 | 7 | | 5 |
/// | lean − | 2 | 6 | 1 |
pub fn direction(lean: f32, steer: f32) -> Option<u8> {
    let len2 = lean * lean + steer * steer;
    if len2 <= 0.25 {
        return None;
    }
    let k = len2.sqrt() * 0.52057;
    Some(if lean > k {
        if steer > k {
            3
        } else if steer < -k {
            0
        } else {
            4
        }
    } else if lean < -k {
        if steer > k {
            2
        } else if steer < -k {
            1
        } else {
            6
        }
    } else if steer > 0.0 {
        7
    } else {
        5
    })
}

/// The two directions next to `d` around the stick (the switch in the
/// trick update): moving the stick to these does not start an Alt.
pub fn neighbours(d: u8) -> [u8; 2] {
    match d & 7 {
        0 => [4, 5],
        1 => [5, 6],
        2 => [6, 7],
        3 => [4, 7],
        4 => [0, 3],
        5 => [0, 1],
        6 => [1, 2],
        _ => [2, 3],
    }
}

/// The trick a press picks: button 6 gives the direction, button 7 alone
/// gives 8 + direction (0x48fa00).
pub fn trick_for(button6: bool, dir: u8) -> usize {
    if button6 {
        dir as usize
    } else {
        8 + dir as usize
    }
}

/// One rider's stunt state through a jump (+0x1520 to +0x153f).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stunt {
    /// The trick in progress, 0..15, or 16..31 for an Alt (+0x433).
    pub trick: Option<usize>,
    /// Time in the trick (+0x1520), the hold bonus (+0x1528) and the time
    /// in the trick before it became an Alt (+0x1524).
    pub time: f32,
    pub held: f32,
    pub before_alt: f32,
    /// Holding at the hold frame last tick (+0x153c).
    pub holding: bool,
    /// Points this jump (+0x1530) and tricks finished this jump (+0x153d).
    pub points: f32,
    pub tricks: u8,
    /// An Alt was used this jump (+0x153f).
    pub used_alt: bool,
}

impl Stunt {
    /// Whether a trick is under way (a new one can't start until it ends).
    pub fn busy(&self) -> bool {
        self.trick.is_some()
    }

    /// Starts `trick` (0x48e8c0). The game starts one only for the player's
    /// rider, uncrashed, with both wheels off the ground, on a trick
    /// button press with the stick off centre.
    pub fn start(&mut self, trick: usize) {
        self.trick = Some(trick);
        self.time = 0.0;
        self.held = 0.0;
        self.before_alt = 0.0;
    }

    /// Advances the trick by `dt` (the trick branch of the rider update).
    /// `frame` is the rider animation's frame, `button` whether trick
    /// button 6 or 7 is down, and `dir` the stick ([`direction`]).
    pub fn tick(&mut self, dt: f32, frame: i32, button: bool, dir: Option<u8>) {
        let Some(t) = self.trick else {
            return;
        };
        self.time += dt;
        if t < 16 && self.time > TRICK_TIME[t] {
            self.time = TRICK_TIME[t];
        }
        if button {
            let hold = match t {
                0..=15 => Some(HOLD_FRAME[t]),
                16 => Some(BAR_KNEEL_ALT_HOLD_FRAME),
                _ => None,
            };
            if let Some(f) = hold {
                if f < frame && frame < f + 3 {
                    // Holding moves time into the uncapped bonus.
                    self.holding = true;
                    self.held += dt;
                    self.time -= dt;
                }
            }
            return;
        }
        // Letting go after a hold and moving the stick away from the
        // trick's direction (not to either side of it) turns it into its Alt.
        let was_holding = std::mem::take(&mut self.holding);
        if was_holding && t < 16 && HAS_ALT[t] {
            if let Some(d) = dir {
                let own = (t & 7) as u8;
                if d != own && !neighbours(own).contains(&d) {
                    self.enter_alt(t);
                }
            }
        }
    }

    /// 0x48e940: the trick's time so far is kept and the Alt starts afresh.
    fn enter_alt(&mut self, t: usize) {
        self.before_alt = self.time;
        self.time = 0.0;
        self.trick = Some(t + 16);
        self.used_alt = true;
    }

    /// The trick's animation finished (0x4967d0): its time scores at
    /// [`POINTS_PER_SECOND`] and the multiplier becomes the trick count.
    /// Returns the seconds scored (the game adds them to a stat, +0x758).
    pub fn finish(&mut self) -> f32 {
        if self.trick.take().is_none() {
            return 0.0;
        }
        let secs = self.held + self.time + self.before_alt;
        self.time = 0.0;
        self.held = 0.0;
        self.before_alt = 0.0;
        self.tricks = self.tricks.saturating_add(1);
        self.points += secs * POINTS_PER_SECOND;
        secs
    }

    /// The multiplier on landing: the tricks this jump, plus 0.5 when an
    /// Alt was used (0x496fa0).
    pub fn multiplier(&self) -> f32 {
        let m = if self.tricks == 0 {
            1.0
        } else {
            self.tricks as f32
        };
        if self.used_alt {
            m + 0.5
        } else {
            m
        }
    }

    /// A clean landing (0x496fa0). Returns the base points rounded down to
    /// 100 and the multiplier; the score is their product. The state
    /// resets for the next jump (0x48e860), as it also does on a crash.
    pub fn land(&mut self) -> (f32, f32) {
        let base = self.points - self.points % 100.0;
        let m = self.multiplier();
        *self = Stunt::default();
        (base, m)
    }
}

/// Stunt prize money in a paying event (Baja, Nationals, Supercross,
/// Enduro), from the Pro Circuit tables (PCTables.pb; 0x4d7bd0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Purse {
    /// The event's entry fee, $ (BajaFee 500, NationalsFee 1500,
    /// SupercrossFee 2500, EnduroFee 300).
    pub fee: i32,
    /// PurseMultiplier, 17.5 by default.
    pub multiplier: f32,
    /// StuntCapPct, 0.075 by default.
    pub stunt_cap: f32,
}

impl Purse {
    /// The most stunts can pay in the event: `int(fee × multiplier) ×
    /// stunt_cap`.
    pub fn cap(&self) -> f32 {
        ((self.fee as f32 * self.multiplier) as i32) as f32 * self.stunt_cap
    }

    /// The money one landed stunt pays (0x496fa0). `base` is the base
    /// points from [`Stunt::land`] (the multiplier doesn't count) and
    /// `earned` the rider's stunt total so far in units of $400 (+0x7b4).
    ///
    /// A stunt pays $400 × min(base / 62500, 1). The stunt that would go
    /// over the cap pays nothing but fills the total up to the cap, as in
    /// the game.
    pub fn pay(&self, base: f32, earned: &mut f32) -> f32 {
        let share = (base * 1.6e-5).min(1.0);
        let cap = self.cap();
        if *earned * 400.0 >= cap {
            return 0.0;
        }
        if (*earned + share) * 400.0 <= cap {
            *earned += share;
            share * 400.0
        } else {
            *earned = cap * 0.0025;
            0.0
        }
    }
}

/// The computer riders' trick sets by air time (0x572d20, 12 rows of 0x50
/// bytes): a row applies when its time is under the air time to spare,
/// and lists the tricks no longer than that time, button 6 tricks first.
/// `(time, all, button6, button7, tricks)`.
pub const AI_TRICKS: [(f32, usize, usize, usize, &[usize]); 12] = [
    (
        2.4,
        16,
        8,
        8,
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    ),
    (
        2.3333,
        15,
        7,
        8,
        &[0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15],
    ),
    (2.2666, 12, 5, 7, &[0, 1, 2, 4, 5, 8, 9, 10, 12, 13, 14, 15]),
    (2.1333, 11, 4, 7, &[0, 1, 4, 5, 8, 9, 10, 12, 13, 14, 15]),
    (1.9375, 8, 4, 4, &[0, 1, 4, 5, 8, 9, 10, 14, 15]),
    (1.8, 8, 4, 5, &[0, 1, 4, 5, 8, 9, 10, 15]),
    (1.7647, 7, 4, 3, &[0, 1, 4, 5, 8, 10, 15]),
    (1.7222, 6, 4, 2, &[0, 1, 4, 5, 8, 15]),
    (1.6667, 4, 4, 0, &[0, 1, 4, 5]),
    (1.6111, 3, 3, 0, &[0, 1, 5]),
    (1.5, 2, 2, 0, &[1, 5]),
    (1.3, 1, 1, 0, &[1]),
];

/// Which set [`ai_pick`] draws from (0x48e730's third argument).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Set {
    All,
    Button6,
    /// The button 7 tricks, or the button 6 ones where a row has none.
    Button7,
}

/// Picks a trick that fits `budget` seconds (0x48e730), with `rand` giving
/// 0 to 1. A computer rider also gets a hold time: `rand × (budget −
/// trick time) × 0.8`, and no trick when less than 0.1 s would be spare.
/// Returns the trick and the hold time.
pub fn ai_pick(budget: f32, set: Set, mut rand: impl FnMut() -> f32) -> Option<(usize, f32)> {
    let &(_, all, six, seven, tricks) = AI_TRICKS.iter().find(|r| r.0 < budget)?;
    let (from, count) = match set {
        Set::All => (0, all),
        Set::Button6 => (0, six),
        Set::Button7 if seven == 0 => (0, six),
        Set::Button7 => (six, seven),
    };
    let i = ((rand() * count as f32 - 0.5).round().max(0.0) as usize).min(count - 1);
    // Two rows' counts disagree with their lists (1.9375 counts 8 of 9,
    // 1.8 counts 9 of 8); past the list the game reads the row's zero
    // padding, BarKneel.
    let trick = tricks.get(from + i).copied().unwrap_or(0);
    let spare = budget - TRICK_TIME[trick];
    if spare < 0.1 {
        return None;
    }
    Some((trick, rand() * spare * 0.8))
}

/// How long a jump will stay up (0x40f170): a ghost from `at` with
/// `velocity` steps 0.1 s at a time (y drops 0.16 more each step, and its
/// vertical speed 3.2) until it is 6 ft under the terrain below it. The
/// game's AI uses this at take-off to plan its tricks.
pub fn flight_time(ground: &impl crate::Ground, at: glam::Vec3, velocity: glam::Vec3) -> f32 {
    let (mut p, mut vy, mut t) = (at, velocity.y, 0.0);
    let below = |p: glam::Vec3| ground.terrain(p.x, p.z).0 >= p.y + 6.0;
    // 60 s is far past any jump; the game has no limit.
    while !below(p) && t < 60.0 {
        p.x += velocity.x * 0.1;
        p.z += velocity.z * 0.1;
        p.y += vy * 0.1 - 0.16;
        vy -= 3.2;
        t += 0.1;
    }
    t
}

/// A computer rider's trick, from the AI controller (0x415170). With its
/// trick timer past the wait (`ready`), in the air and not in a trick, it
/// tries with chance `chance` (0 to 100) to fit a trick into the flight,
/// first at a fifth of the air time less 0.5 s, then a quarter, and so on
/// up to the whole: the first that fits starts and the timer restarts.
pub fn ai_trick(air: f32, chance: f32, mut rand: impl FnMut() -> f32) -> Option<(usize, f32)> {
    if rand() * 100.0 > chance {
        return None;
    }
    (1..=5)
        .rev()
        .find_map(|n| ai_pick(air / n as f32 - 0.5, Set::All, &mut rand))
}

/// The wait between a computer rider's tricks by AI level 1 to 3 (bike
/// +0x7d0..+0x7d8, s) and its trick chance by level in the Stunt Quarry
/// (race +0x9c..+0xa4, %). Elsewhere the chance is the race's +0xa4.
pub const AI_TRICK_WAIT: [f32; 3] = [25.0, 15.0, 5.0];
pub const AI_TRICK_CHANCE: [f32; 3] = [10.0, 50.0, 100.0];
/// A computer rider's trick chance in any other event, % a frame
/// (0x490c30 sets bike +0x15dc to 2).
pub const AI_RACE_TRICK_CHANCE: f32 = 2.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_stick() {
        assert_eq!(direction(0.1, 0.1), None);
        assert_eq!(direction(1.0, 0.0), Some(4));
        assert_eq!(direction(1.0, -1.0), Some(0));
        assert_eq!(direction(0.0, 1.0), Some(7));
        assert_eq!(direction(-1.0, 0.0), Some(6));
        // Each direction's neighbours are the two next to it in the table.
        for d in 0..8u8 {
            for n in neighbours(d) {
                assert!(neighbours(n).contains(&d));
            }
        }
        assert_eq!(TRICKS[trick_for(false, 2)], "SaranWrap");
    }

    #[test]
    fn scores_a_held_trick() {
        let mut s = Stunt::default();
        s.start(0); // BarKneel, 1.611 s, holds at frames 15..16
        for frame in 0..40 {
            s.tick(0.1, frame, true, Some(0));
        }
        // The clock stops at the trick's 1.611 s, but the two ticks in the
        // hold window moved 0.2 s into the uncapped bonus first.
        assert!((s.held - 0.2).abs() < 1e-5);
        let secs = s.finish();
        assert!((secs - 1.811).abs() < 1e-3);
        // 1.811 s × 2500 = 4527.5, rounded down to 100.
        assert_eq!(s.land(), (4500.0, 1.0));
    }

    #[test]
    fn alt_and_multiplier() {
        let mut s = Stunt::default();
        s.start(4); // HeelClicker, has an Alt, holds at 12..13
        s.tick(0.5, 12, true, Some(4));
        // A neighbour doesn't switch.
        s.tick(0.1, 12, false, Some(0));
        assert_eq!(s.trick, Some(4));
        s.tick(0.5, 12, true, Some(4));
        // The opposite side does.
        s.tick(0.1, 20, false, Some(1));
        assert_eq!(s.trick, Some(20));
        s.finish();
        s.start(9);
        s.tick(1.0, 0, false, Some(1));
        s.finish();
        let (_, m) = s.land();
        assert_eq!(m, 2.5);
    }

    #[test]
    fn computer_tricks_fit_the_air() {
        // Rows: every listed trick fits the row's time.
        for (t, _, _, _, tricks) in AI_TRICKS {
            assert!(tricks.iter().all(|&i| TRICK_TIME[i] <= t + 1e-3));
        }
        // 1.39 s: only SplitX (1.3 s) fits, with under 0.1 s to spare.
        assert_eq!(ai_pick(1.39, Set::All, || 0.5), None);
        let (trick, hold) = ai_pick(1.5, Set::All, || 0.5).unwrap();
        assert_eq!(TRICKS[trick], "SplitX");
        assert!((hold - 0.08).abs() < 1e-4);
        // A 3 s flight: a fifth less 0.5 fits nothing, a half (1.0) nothing,
        // the whole 2.5 s draws from all 16.
        assert!(ai_trick(3.0, 100.0, || 0.99).is_some());
        assert!(ai_trick(1.0, 100.0, || 0.5).is_none());
    }

    #[test]
    fn times_a_flight() {
        // Level ground, 32 ft/s up: back to the ground near 2 s, plus the
        // fall to 6 ft under.
        let t = flight_time(
            &crate::Flat,
            glam::Vec3::ZERO,
            glam::Vec3::new(0.0, 32.0, 50.0),
        );
        assert!((2.0..2.6).contains(&t), "{t}");
    }

    #[test]
    fn pays_up_to_the_cap() {
        // Nationals: int(1500 × 17.5) × 0.075 = $1968.75.
        let p = Purse {
            fee: 1500,
            multiplier: 17.5,
            stunt_cap: 0.075,
        };
        let mut earned = 0.0;
        assert!((p.pay(62500.0, &mut earned) - 400.0).abs() < 1e-2);
        assert!((p.pay(31250.0, &mut earned) - 200.0).abs() < 1e-2);
        // Three more full stunts reach $1800; the next would pass the cap,
        // so it pays nothing and fills the total to the cap.
        for _ in 0..3 {
            p.pay(100000.0, &mut earned);
        }
        assert_eq!(p.pay(100000.0, &mut earned), 0.0);
        assert!((earned * 400.0 - 1968.75).abs() < 1e-2);
        assert_eq!(p.pay(100000.0, &mut earned), 0.0);
    }
}
