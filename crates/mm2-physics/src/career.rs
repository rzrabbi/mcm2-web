//! The Pro Circuit career's money and points (ProCircuit.cpp: tables
//! 0x4d7bd0, payout 0x4dd440; race points 0x4600f0; crash costs 0x48ea00).
//!
//! Each event has an entry fee; the purse is the fee times the purse
//! multiplier, split by finishing place. Crashes run up repair and medical
//! bills and stunts earn money, each capped at a share of the purse.

/// The Pro Circuit tables (UI/UIRES.RES PCTables.pb, `[ProCircuit]`).
/// [`Tables::default`] is the exe's fallback when the file is missing;
/// [`Tables::shipped`] is what the game's own file says.
#[derive(Debug, Clone, PartialEq)]
pub struct Tables {
    /// `StartingCapital`, $.
    pub starting_capital: i32,
    /// Entry fees by event (0 Stunt Quarry, 1 Baja, 2 Nationals,
    /// 3 Supercross, 4 none, 5 Enduro): `BajaFee`, `NationalsFee`,
    /// `SupercrossFee`, `EnduroFee`.
    pub fees: [i32; 6],
    /// `PurseMultiplier`.
    pub purse_multiplier: f32,
    /// `RepairCapPct`, `MedicalCapPct`, `StuntCapPct`: the most each can
    /// cost or pay, as a share of the purse.
    pub repair_cap: f32,
    pub medical_cap: f32,
    pub stunt_cap: f32,
    /// `1_PlacePct` .. `11_PlacePct`: each place's share of the purse
    /// (only the ratios matter; see [`pay`]).
    pub place_pct: [f32; 11],
}

impl Default for Tables {
    fn default() -> Self {
        Tables {
            starting_capital: 5000,
            fees: [0, 500, 1500, 2500, 0, 300],
            purse_multiplier: 17.5,
            repair_cap: 0.1,
            medical_cap: 0.05,
            stunt_cap: 0.075,
            place_pct: [33.0, 23.0, 14.0, 9.0, 7.0, 5.0, 4.0, 3.0, 2.0, 0.0, 0.0],
        }
    }
}

impl Tables {
    /// PCTables.pb as shipped: ten times the fallback fees and capital,
    /// lower repair and medical caps, and shares as fractions of 1.
    pub fn shipped() -> Self {
        let mut t = Tables {
            starting_capital: 50000,
            fees: [0, 5000, 15000, 25000, 0, 3000],
            purse_multiplier: 17.5,
            repair_cap: 0.075,
            medical_cap: 0.0375,
            stunt_cap: 0.075,
            place_pct: [
                0.33, 0.23, 0.14, 0.09, 0.07, 0.05, 0.04, 0.03, 0.02, 0.0, 0.0,
            ],
        };
        t.top_up();
        t
    }

    /// When the file's place shares add up to 1 or less, the rest goes to
    /// first place (0x4d7bd0).
    pub fn top_up(&mut self) {
        let sum: f32 = self.place_pct.iter().sum();
        if sum <= 1.0 {
            self.place_pct[0] += 1.0 - sum;
        }
    }

    /// The event's purse: `int(fee × PurseMultiplier)`.
    pub fn purse(&self, event: usize) -> i32 {
        (self.fees[event] as f32 * self.purse_multiplier) as i32
    }

    /// The stunt purse settings for [`crate::stunt::Purse`].
    pub fn stunt_purse(&self, event: usize) -> crate::stunt::Purse {
        crate::stunt::Purse {
            fee: self.fees[event],
            multiplier: self.purse_multiplier,
            stunt_cap: self.stunt_cap,
        }
    }
}

/// Championship points by finishing place, 1st to 10th (0x4600f0).
pub const PLACE_POINTS: [i32; 10] = [20, 17, 15, 12, 10, 8, 6, 5, 3, 1];

/// The points a finishing place earns; none past 10th.
pub fn place_points(place: usize) -> i32 {
    match place {
        1..=10 => PLACE_POINTS[place - 1],
        _ => 0,
    }
}

/// Each crash adds to a rider's repair and medical units (0x48ea00, on
/// the first step the bike is crashed: `f_444` set, `f_5b4`, last step's
/// crash flag, still clear). Both grow by `rand × 0.667`, and a computer
/// rider (`f_734`) adds another `rand × 0.15` to each first.
pub const CRASH_UNITS: f32 = 0.667;
pub const AI_CRASH_UNITS: f32 = 0.15;

/// The (repair, medical) units one crash adds. `rand` returns 0..1 (the
/// game's `rand() / 32768`) and is drawn in the game's order.
pub fn crash_units(ai: bool, mut rand: impl FnMut() -> f32) -> (f32, f32) {
    let (mut repair, mut medical) = (0.0, 0.0);
    if ai {
        repair += rand() * AI_CRASH_UNITS;
        medical += rand() * AI_CRASH_UNITS;
    }
    repair += rand() * CRASH_UNITS;
    medical += rand() * CRASH_UNITS;
    (repair, medical)
}

/// What a unit costs: $500 of repairs, $250 of medical bills. Stunt units
/// pay $400 (see [`crate::stunt::Purse::pay`]).
pub const REPAIR_PER_UNIT: f32 = 500.0;
pub const MEDICAL_PER_UNIT: f32 = 250.0;

/// One rider's race for the payout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    /// Finishing place, 1 = first.
    pub place: usize,
    /// Championship points this race.
    pub points: i32,
    /// Repair, medical and stunt units run up in the race (bike +0x7ac,
    /// +0x7b0, +0x7b4).
    pub repair: f32,
    pub medical: f32,
    pub stunts: f32,
}

/// One rider's money from a race (ProCircuit rider record +0x465..).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Payout {
    pub prize: i32,
    pub repair: i32,
    pub medical: i32,
    pub stunts: i32,
    /// prize + stunts − repair − medical − fee.
    pub net: i32,
}

/// Pays out a race of `event` to `entries` (0x4dd440). `riders` is the
/// career's field size: the place shares are normalised over that many.
///
/// * prize: `floor(purse × share[place])`, and whatever rounding leaves
///   over goes to the rider with the most points this race;
/// * repair and medical: units × $500 / $250, capped at the purse ×
///   RepairCapPct / MedicalCapPct;
/// * stunts: units × $400, capped at the purse × StuntCapPct;
/// * every rider pays the entry fee.
pub fn pay(tables: &Tables, event: usize, riders: usize, entries: &[Entry]) -> Vec<Payout> {
    let purse = tables.purse(event);
    let p = purse as f32;
    let fee = tables.fees[event];
    let n = riders.min(tables.place_pct.len());
    let total: f32 = tables.place_pct[..n].iter().sum();
    let share = |place: usize| {
        if (1..=n).contains(&place) && total > 0.0 {
            tables.place_pct[place - 1] / total
        } else {
            0.0
        }
    };
    let mut out: Vec<Payout> = entries
        .iter()
        .map(|e| {
            let medical = (p * tables.medical_cap).min(e.medical * MEDICAL_PER_UNIT) as i32;
            let repair = (p * tables.repair_cap).min(e.repair * REPAIR_PER_UNIT) as i32;
            let stunts = (p * tables.stunt_cap).min(e.stunts * 400.0) as i32;
            let prize = (p * share(e.place)).floor() as i32;
            Payout {
                prize,
                repair,
                medical,
                stunts,
                net: prize + stunts - repair - medical - fee,
            }
        })
        .collect();
    let paid: i32 = out.iter().map(|o| o.prize).sum();
    // The game sorts by points, most first (qsort, so a tie has no set
    // order); here the first listed wins a tie.
    if let Some(top) = (0..entries.len())
        .max_by(|&a, &b| entries[a].points.cmp(&entries[b].points).then(b.cmp(&a)))
    {
        out[top].prize += purse - paid;
        out[top].net += purse - paid;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pays_a_nationals() {
        let t = Tables::default();
        // Nationals: $1500 × 17.5 = $26250 purse.
        assert_eq!(t.purse(2), 26250);
        let e = |place, points, repair| Entry {
            place,
            points,
            repair,
            medical: 0.0,
            stunts: 0.0,
        };
        let entries = [e(1, 20, 0.0), e(2, 17, 10.0), e(3, 15, 0.0)];
        let out = pay(&t, 2, 3, &entries);
        // Shares over three riders: 33, 23, 14 of 70.
        assert_eq!(out[1].prize, (26250.0 * 23.0 / 70.0_f32).floor() as i32);
        // Repairs: 10 units × $500 = $5000, capped at 10 % = $2625.
        assert_eq!(out[1].repair, 2625);
        // The whole purse is paid; the leftover goes to the winner.
        assert_eq!(out.iter().map(|o| o.prize).sum::<i32>(), 26250);
        assert_eq!(out[0].net, out[0].prize - 1500);
        assert_eq!(place_points(1) + place_points(10) + place_points(11), 21);
        assert_eq!(crash_units(false, || 0.5), (0.3335, 0.3335));
        let (r, _) = crash_units(true, || 1.0);
        assert!((r - 0.817).abs() < 1e-6);
        // The shipped file's shares add to 1; the same split, ten times the
        // money.
        let s = Tables::shipped();
        assert_eq!(s.purse(2), 262500);
        assert!((s.place_pct.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }
}
