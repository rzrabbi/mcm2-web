//! The engine and gearbox, after the game's drivetrain (ctor 0x4d7090,
//! throttle ramp 0x4d76a0, shifting 0x4d7780, drive force 0x4d7900).
//!
//! The game keeps a throttle position from 0 to 1 that ramps towards the
//! rider's input, and the engine's speed is that position times the top of
//! the rev range: the revs do not follow the rear wheel. The gearbox shifts
//! by itself, up when the revs pass the gear's shift point while rising and
//! down when they fall below 1500, and each shift rescales the throttle by
//! the ratio of the two gears. While a shift is under way (0.05 s) the
//! clutch is in: the throttle eases off and there is no drive. The force at
//! the rear tyre is the power curve times the gear's ratio, over the road
//! speed.
//!
//! The constructor falls back to a built-in curve when it is given none,
//! and the original's own tapes show the player's bike riding on that
//! curve rather than the presets.pb one: its shifts come at 0.8 s, where
//! the stock curve's 8250 rpm shift point falls, and in top gear it pulls
//! with exactly the stock curve's 25 hp × 1.85 (FINDINGS-1).

use crate::spec::Spec;

/// Gear ratios as the game sets them (0x4d7090).
pub const RATIOS: [f32; 5] = [2.0, 1.97, 1.95, 1.9, 1.85];
/// The game's force constant in 0x4d7900.
const FORCE: f32 = 341.62;
/// Horsepower × 5252.11 / rpm is torque, lb·ft.
const TORQUE: f32 = 5252.11;
/// Shift down below these revs (0x4d7090: +0x7c).
const SHIFT_DOWN: f32 = 1500.0;
/// Shift up somewhere in ± this around the best point; we take the middle.
#[cfg(test)]
const SHIFT_SPREAD: f32 = 100.0;
/// The constructor's built-in curve (0x4d7090 with no rev limits): revs
/// and horsepower, and the top of the range.
const STOCK_RPM: [f32; 12] = [
    3000.0, 3250.0, 3750.0, 4750.0, 5250.0, 5750.0, 6750.0, 7250.0, 8250.0, 8750.0, 9250.0,
    10250.0,
];
const STOCK_POWER: [f32; 12] = [
    10.0, 11.5, 15.0, 20.5, 24.0, 26.0, 39.0, 43.0, 40.5, 37.0, 28.5, 25.0,
];

#[derive(Debug, Clone)]
pub struct Drivetrain {
    /// 0 to 1.
    pub throttle: f32,
    pub rpm: f32,
    pub gear: usize,
    /// Seconds left of a gear change; the throttle eases off meanwhile.
    pub shifting: f32,
    /// The clutch is in this step (+8): a shift is under way or the gas
    /// is off, and the engine gives no drive.
    pub disengaged: bool,
    /// How fast the throttle opens (+0x1dc) and closes (+0x1e0), per second.
    pub open_rate: f32,
    pub close_rate: f32,
    /// The time to shift (+0x88).
    pub shift_time: f32,
    pub redline: f32,
    /// The power curve: revs and horsepower, from 0 rpm up (+0x98, +0xd4).
    rpms: Vec<f32>,
    power: Vec<f32>,
    /// Revs to shift up at, per gear (+0x30).
    up_at: [f32; RATIOS.len()],
    last_rpm: f32,
}

impl Default for Drivetrain {
    fn default() -> Self {
        Drivetrain::new(&Spec::default(), 1.0, 1.0)
    }
}

impl Drivetrain {
    /// The drivetrain for `spec`. `open_rate` and `close_rate` are the
    /// options' Press and Release Throttle sliders, 0.8 to 1.2.
    pub fn new(spec: &Spec, open_rate: f32, close_rate: f32) -> Self {
        let step = (spec.rpm_high - spec.rpm_low) / 10.0;
        let rpms = (0..spec.power.len()).map(|i| spec.rpm_low + step * i as f32);
        Drivetrain::with_curve(rpms.zip(spec.power), spec.rpm_high, open_rate, close_rate)
    }

    /// The drivetrain on the constructor's built-in curve, which is what
    /// the player's bike rides on.
    pub fn stock(open_rate: f32, close_rate: f32) -> Self {
        let curve = STOCK_RPM.into_iter().zip(STOCK_POWER);
        Drivetrain::with_curve(curve, STOCK_RPM[STOCK_RPM.len() - 1], open_rate, close_rate)
    }

    fn with_curve(
        curve: impl Iterator<Item = (f32, f32)>,
        redline: f32,
        open_rate: f32,
        close_rate: f32,
    ) -> Self {
        let (mut rpms, mut power) = (vec![0.0], vec![0.0]);
        for (r, hp) in curve {
            rpms.push(r);
            power.push(hp);
        }
        let mut d = Drivetrain {
            throttle: 0.0,
            rpm: 0.0,
            gear: 0,
            shifting: 0.0,
            disengaged: true,
            open_rate,
            close_rate,
            shift_time: 0.05,
            redline,
            rpms,
            power,
            up_at: [redline; RATIOS.len()],
            last_rpm: 0.0,
        };
        d.up_at = d.shift_points();
        d
    }

    /// The same engine at rest.
    pub fn fresh(&self) -> Self {
        Drivetrain {
            throttle: 0.0,
            rpm: 0.0,
            gear: 0,
            shifting: 0.0,
            disengaged: true,
            last_rpm: 0.0,
            ..self.clone()
        }
    }

    /// Torque at each point of the curve.
    fn torques(&self) -> Vec<f32> {
        self.rpms
            .iter()
            .zip(&self.power)
            .map(|(&r, &p)| if r > 0.0 { p * TORQUE / r } else { 0.0 })
            .collect()
    }

    /// Where to shift up out of each gear (0x4d7090): of the points above
    /// the torque peak, the one whose torque best matches the torque the
    /// next gear would have at the revs it drops to.
    fn shift_points(&self) -> [f32; RATIOS.len()] {
        let torque = self.torques();
        let peak = (0..torque.len())
            .max_by(|&a, &b| torque[a].total_cmp(&torque[b]))
            .unwrap_or(0);
        let at = |rpm: f32| interpolate(&self.rpms, &torque, rpm);
        let mut out = [self.redline; RATIOS.len()];
        for g in 0..RATIOS.len() - 1 {
            let r = RATIOS[g + 1] / RATIOS[g];
            let best = (peak + 1..self.rpms.len()).min_by(|&a, &b| {
                let d = |j: usize| (torque[j] - at((r * self.rpms[j]).min(self.rpms[peak]))).abs();
                d(a).total_cmp(&d(b))
            });
            if let Some(j) = best {
                // The game picks at random in [x − 100, min(x + 100, top)].
                let x = self.rpms[j];
                let (lo, hi) = ((x - 100.0).max(0.0), (x + 100.0).min(self.redline));
                out[g] = (lo + hi) / 2.0;
            }
        }
        out
    }

    /// The throttle ramp (0x4d76a0) and the gearbox (0x4d7780), for one
    /// step of `h` seconds. `clutch` is the Clutch key (or backing up,
    /// 0x493380, 0x4933e0): with the gas on, the engine revs but gives no
    /// drive.
    pub fn update(&mut self, h: f32, gas: bool, clutch: bool) {
        // The shift clock runs down first (0x52ffb0), so a shift holds the
        // clutch in for the steps it has left.
        if self.shifting > 0.0 {
            self.shifting = (self.shifting - h).max(0.0);
        }
        self.disengaged = !gas || self.shifting != 0.0;
        let free = self.disengaged;
        self.disengaged |= clutch;
        if free {
            // Released, or the clutch is in: the throttle closes, slower
            // during a shift.
            let rate = if self.shifting != 0.0 { 0.2 } else { 0.6 };
            self.throttle = (self.throttle - h * self.close_rate * rate).max(0.0);
        } else {
            self.throttle = (self.throttle + h * self.open_rate).min(1.0);
        }
        self.rpm = self.throttle * self.redline;
        if self.shifting == 0.0 {
            if self.rpm >= self.last_rpm
                && self.rpm > self.up_at[self.gear]
                && self.gear + 1 < RATIOS.len()
            {
                self.gear += 1;
                self.shift(self.gear - 1);
            }
            if self.gear > 0
                && (self.rpm < self.last_rpm || self.rpm == 0.0)
                && self.rpm < SHIFT_DOWN
            {
                self.gear -= 1;
                self.shift(self.gear + 1);
            }
        }
        self.last_rpm = self.rpm;
    }

    /// The throttle carries over in proportion to the two gears' ratios.
    fn shift(&mut self, from: usize) {
        self.throttle = (RATIOS[self.gear] * self.throttle / RATIOS[from]).min(1.0);
        self.rpm = self.throttle * self.redline;
        self.shifting = self.shift_time;
    }

    /// Horsepower at `rpm`, interpolated along the curve.
    pub fn power(&self, rpm: f32) -> f32 {
        interpolate(&self.rpms, &self.power, rpm)
    }

    /// The force pushing the bike along at `speed` ft/s, lb (0x4d7900:
    /// power × ratio × 341.62 / speed, the speed at least 1.01).
    pub fn drive_force(&self, speed: f32) -> f32 {
        if self.disengaged {
            return 0.0;
        }
        self.power(self.rpm) * RATIOS[self.gear] * FORCE / speed.abs().max(1.01)
    }
}

fn interpolate(xs: &[f32], ys: &[f32], x: f32) -> f32 {
    let i = xs.iter().position(|&r| x <= r).unwrap_or(xs.len() - 1);
    if i == 0 {
        return ys[0];
    }
    let t = ((x - xs[i - 1]) / (xs[i] - xs[i - 1])).clamp(0.0, 1.0);
    ys[i - 1] + (ys[i] - ys[i - 1]) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_curve_from_the_spec() {
        let d = Drivetrain::default();
        assert_eq!(d.power(5000.0), 24.0);
        assert_eq!(d.power(8500.0), 45.0);
        assert_eq!(d.power(2500.0), 12.0);
        assert!((d.power(8750.0) - 43.0).abs() < 1e-4);
        assert_eq!(d.redline, 10000.0);
        // Shift points sit just past the torque peak.
        for &p in &d.up_at[..4] {
            assert!(p > 5000.0 - SHIFT_SPREAD && p < 10000.0, "{:?}", d.up_at);
        }
    }

    #[test]
    fn revs_up_and_shifts() {
        let mut d = Drivetrain::default();
        for _ in 0..200 {
            d.update(0.02, true, false);
        }
        assert_eq!(d.gear, RATIOS.len() - 1, "never shifted");
        assert!(d.rpm > 9000.0);
        for _ in 0..500 {
            d.update(0.02, false, false);
        }
        assert_eq!(d.throttle, 0.0);
        assert_eq!(d.gear, 0, "never shifted back down");
    }

    #[test]
    fn stock_curve_shifts_at_8250() {
        let mut d = Drivetrain::stock(1.0, 1.0);
        assert_eq!(d.redline, 10250.0);
        assert!(d.up_at[..4].iter().all(|&p| p == 8250.0), "{:?}", d.up_at);
        assert_eq!(d.power(d.redline), 25.0);
        // The first shift comes once the throttle is 8250/10250 open,
        // and the clutch stays in for the two steps after it.
        let mut first = None;
        let mut cut = 0;
        for k in 1..=60 {
            d.update(0.02, true, false);
            if d.gear == 1 && first.is_none() {
                first = Some(k);
                assert!(!d.disengaged, "the step that shifts still drives");
            } else if first.is_some() && d.gear == 1 && d.disengaged {
                cut += 1;
            }
        }
        assert_eq!(first, Some(41));
        assert_eq!(cut, 2);
    }
}
