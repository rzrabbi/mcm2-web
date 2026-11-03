//! Lap counting, timing and race positions, after the game's progress
//! tracker (0x4eb170 for lap races, 0x4eb7e0 for the Baja and Enduro gate
//! races).
//!
//! The course graph itself (where along the course each rider is, from
//! the track's `.tdf`) belongs to the caller; this holds the rules the
//! game applies on top of it.

use glam::Vec3;

/// A finish line or a checkpoint gate (the line structs 0x47c5b0 reads).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub centre: Vec3,
    /// The way riders should cross it.
    pub forward: Vec3,
    /// From the centre to one end; the line runs from `centre − half_width`
    /// to `centre + half_width`.
    pub half_width: Vec3,
    /// How far above the centre a crossing still counts, ft.
    pub height: f32,
}

impl Line {
    /// Whether moving from `from` to `to` crosses the line (0x47c5b0): the
    /// level paths meet, the crossing is no more than `height` above the
    /// centre, and, when `one_way`, the move goes along `forward`.
    pub fn crossed(&self, from: Vec3, to: Vec3, one_way: bool) -> bool {
        // The game skips a rider whose last place is the origin (not set).
        if from == Vec3::ZERO {
            return false;
        }
        let a = self.centre - self.half_width;
        let span = self.half_width * 2.0;
        let path = to - from;
        // Solve from + path·t = a + span·u in the level plane.
        let det = path.x * span.z - path.z * span.x;
        if det == 0.0 {
            return false;
        }
        let q = Vec3::new(a.x - from.x, 0.0, a.z - from.z);
        let t = (q.x * span.z - q.z * span.x) / det;
        let u = (q.x * path.z - q.z * path.x) / det;
        if !(0.0..=1.0).contains(&t) || !(0.0..=1.0).contains(&u) {
            return false;
        }
        let y = from.y + path.y * t;
        y - self.centre.y <= self.height && (!one_way || path.dot(self.forward) >= 0.0)
    }
}

/// How many splits a rider keeps: one per line crossing in a lap race
/// (100), one per gate in a gate race (600).
pub const LAP_SPLITS: usize = 100;
pub const GATE_SPLITS: usize = 600;

/// One rider's race clock (bike +0x74c to +0x7a4).
#[derive(Debug, Clone, PartialEq)]
pub struct Timing {
    /// Times the rider has crossed the finish line (+0x7a0), which is the
    /// laps done: the HUD shows `Lap: min(crossings + 1, laps) / laps`.
    /// The first crossing ends lap 1, timed from the gate drop.
    pub crossings: u32,
    /// Gates passed in all (+0x790) and the gate the rider is heading for
    /// (+0x7b8), in a gate race.
    pub gates: u32,
    pub gate: usize,
    pub finished: bool,
    /// The race clock (+0x754), the lap in progress (+0x74c) and the best
    /// lap (+0x750; 0 until there is one).
    pub race_time: f32,
    pub lap_time: f32,
    pub best_lap: f32,
    /// The race clock at the last crossing (+0x774).
    pub lap_start: f32,
    /// The race clock at each crossing or gate, by count (+0x77c, +0x780).
    pub splits: Vec<f32>,
    /// Seconds behind the leader at the same count (the HUD's TBL, +0x770).
    pub behind_leader: f32,
    /// Seconds off the course (+0x76c); the drive is halved meanwhile.
    pub off_course: f32,
}

impl Timing {
    pub fn new(gates: bool) -> Self {
        Timing {
            crossings: 0,
            gates: 0,
            gate: 0,
            finished: false,
            race_time: 0.0,
            lap_time: 0.0,
            best_lap: 0.0,
            lap_start: 0.0,
            splits: vec![0.0; if gates { GATE_SPLITS } else { LAP_SPLITS }],
            behind_leader: 0.0,
            off_course: 0.0,
        }
    }

    /// The clock runs until the rider finishes.
    pub fn tick(&mut self, dt: f32, on_course: bool) {
        if !self.finished {
            self.race_time += dt;
        }
        if on_course {
            self.off_course = 0.0;
        } else {
            self.off_course += dt;
        }
    }

    /// The rider crossed the finish line (0x4ea650). The race is over for
    /// them at `finish_at` crossings (the race's lap count, global
    /// +0x2d90), unless it has no end (practice). The best lap is first
    /// set at the second crossing, so lap 1 from the gate never counts.
    pub fn cross_line(&mut self, finish_at: u32, endless: bool) {
        self.crossings += 1;
        if !endless && self.crossings >= finish_at {
            self.finished = true;
        }
        self.lap_time = (self.race_time - self.lap_start).max(0.0);
        let n = self.splits.len();
        self.splits[self.crossings as usize % n] = self.race_time;
        if self.crossings == 2 || self.lap_time < self.best_lap {
            self.best_lap = self.lap_time;
        }
        self.lap_start = self.race_time;
    }

    /// The rider passed their next gate of `count` (0x4ea750). Past the
    /// last gate a lap is done and the rider heads for the first again.
    /// `boxed` is the game's `f_735` case, where the WaypointBox trigger
    /// reports the pass instead of a line crossing; the best lap is then
    /// not kept.
    pub fn pass_gate(&mut self, count: usize, finish_at: u32, endless: bool, boxed: bool) {
        self.gate += 1;
        if self.gate >= count {
            self.gate = 0;
            self.crossings += 1;
            if !endless && self.crossings >= finish_at {
                self.finished = true;
            }
            let lap = self.race_time - self.lap_start;
            self.lap_time = lap.max(0.0);
            if !boxed && (lap < self.best_lap || self.best_lap <= 0.0) {
                self.best_lap = lap;
            }
            self.lap_start = self.race_time;
        }
        self.gates += 1;
        let n = self.splits.len();
        self.splits[self.gates as usize % n] = self.race_time;
    }
}

/// Where a rider stands for [`positions`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Standing {
    pub finished: bool,
    /// Laps (crossings) in a lap race, gates in a gate race.
    pub count: u32,
    /// In a lap race, the rider has passed the finish's place on the course
    /// but not yet crossed the line, which counts one lap less (0x4eacd0:
    /// entry +0x18).
    pub short_of_line: bool,
    /// Course distance to the finish line, or the squared level distance to
    /// the next gate; less is ahead.
    pub to_go: f32,
}

/// The order of the riders, from the last order `order` (indices into
/// `riders`): finished riders keep their places at the front, and the rest
/// sort by count, then by distance to go (0x4eacd0, 0x4eaeb0).
pub fn positions(order: &[usize], riders: &[Standing]) -> Vec<usize> {
    let key = |s: &Standing| s.count as i64 - s.short_of_line as i64;
    let (mut done, mut racing): (Vec<usize>, Vec<usize>) =
        order.iter().partition(|&&i| riders[i].finished);
    racing.sort_by(|&a, &b| {
        let (a, b) = (&riders[a], &riders[b]);
        key(b).cmp(&key(a)).then(
            a.to_go
                .partial_cmp(&b.to_go)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    done.extend(racing);
    done
}

/// Each rider's time behind the leader at the same count (0x4eafa0), in
/// `order` (positions first to last). `gates` picks the gate splits.
pub fn behind_leader(order: &[usize], timings: &mut [Timing], gates: bool) {
    let Some(&leader) = order.first() else {
        return;
    };
    for (place, &i) in order.iter().enumerate() {
        let gap = if place == 0 {
            0.0
        } else {
            let t = &timings[i];
            let n = t.splits.len();
            let k = if gates { t.gates } else { t.crossings } as usize % n;
            (t.splits[k] - timings[leader].splits[k]).max(0.0)
        };
        timings[i].behind_leader = gap;
    }
}

/// When the one player finishes a single-player race, the game finishes
/// every computer rider still out at once (0x4eb170, 0x4eb7e0), in their
/// current order. `laps` is the race's lap count (global +0x2d90, the
/// same number [`Timing::cross_line`] finishes at) and `rand` gives 0 to 1.
/// For each, with `left` laps to go:
///
/// * the race time becomes `left × lap time` (0 with no lap done), and at
///   least 1 to 11 s more than the rider ahead's;
/// * in a one-lap race the lap time is the race time, and a rider with no
///   lap done gets the player's lap time plus 1 to 11 s;
/// * the best lap takes the lap time if better, and the rider is finished
///   with all laps done.
///
/// (The game overwrites the race time with that product, as here.)
pub fn finish_the_rest(
    order: &[usize],
    timings: &mut [Timing],
    computer: &[bool],
    laps: u32,
    player_lap: f32,
    mut rand: impl FnMut() -> f32,
) {
    let mut ahead: Option<f32> = None;
    for &i in order {
        let t = &mut timings[i];
        if !t.finished && computer[i] {
            let left = laps.saturating_sub(t.crossings);
            if left > 0 {
                t.race_time = if t.crossings == 0 {
                    0.0
                } else {
                    left as f32 * t.lap_time
                };
            }
            if let Some(a) = ahead {
                if t.race_time < a {
                    t.race_time = a + 1.0 + rand() * 10.0;
                }
            }
            if laps == 1 {
                t.lap_time = t.race_time;
            } else if t.crossings == 0 {
                t.lap_time = player_lap + 1.0 + rand() * 10.0;
            }
            if t.lap_time < t.best_lap || t.best_lap <= 0.0 {
                t.best_lap = t.lap_time;
            }
            t.crossings = laps;
            t.finished = true;
        }
        ahead = Some(timings[i].race_time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Line {
        Line {
            centre: Vec3::new(0.0, 0.0, 10.0),
            forward: Vec3::Z,
            half_width: Vec3::new(20.0, 0.0, 0.0),
            height: 30.0,
        }
    }

    #[test]
    fn crosses_the_line_one_way() {
        let l = line();
        let (a, b) = (Vec3::new(1.0, 0.0, 9.0), Vec3::new(1.0, 0.0, 11.0));
        assert!(l.crossed(a, b, true));
        assert!(!l.crossed(b, a, true));
        assert!(l.crossed(b, a, false));
        // Off the end, or too high above it.
        assert!(!l.crossed(Vec3::new(25.0, 0.0, 9.0), Vec3::new(25.0, 0.0, 11.0), true));
        assert!(!l.crossed(Vec3::new(1.0, 40.0, 9.0), Vec3::new(1.0, 40.0, 11.0), true));
    }

    #[test]
    fn times_laps() {
        let mut t = Timing::new(false);
        t.tick(40.0, true);
        t.cross_line(4, false); // lap 1, from the gate: not a best lap
        assert_eq!(t.best_lap, 0.0);
        t.tick(30.0, true);
        t.cross_line(4, false);
        assert_eq!(t.best_lap, 30.0);
        t.tick(28.0, true);
        t.cross_line(4, false);
        assert_eq!((t.lap_time, t.best_lap), (28.0, 28.0));
        t.tick(31.0, true);
        t.cross_line(4, false);
        assert!(t.finished);
        assert_eq!(t.best_lap, 28.0);
        assert_eq!(t.splits[3], 98.0);
        t.tick(5.0, false);
        assert_eq!(t.race_time, 129.0);
        assert_eq!(t.off_course, 5.0);
    }

    #[test]
    fn orders_riders() {
        let s = |finished, count, short_of_line, to_go| Standing {
            finished,
            count,
            short_of_line,
            to_go,
        };
        let riders = [
            s(false, 2, false, 500.0),
            s(false, 2, false, 100.0),
            s(true, 4, false, 0.0),
            s(false, 3, true, 900.0),
        ];
        assert_eq!(positions(&[0, 1, 2, 3], &riders), vec![2, 1, 0, 3]);
    }

    #[test]
    fn finishes_the_computer_riders() {
        let mut ts = vec![Timing::new(false), Timing::new(false)];
        ts[0].finished = true;
        ts[0].race_time = 100.0;
        ts[1].crossings = 2;
        ts[1].lap_time = 30.0;
        finish_the_rest(&[0, 1], &mut ts, &[false, true], 4, 25.0, || 0.5);
        assert!(ts[1].finished);
        // 2 laps left at 30 s is 60 s, under the leader's 100: 100 + 6.
        assert_eq!(ts[1].race_time, 106.0);
        assert_eq!(ts[1].crossings, 4);
    }
}
