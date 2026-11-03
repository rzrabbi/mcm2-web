//! The Pro Circuit career (`ProCircuit.cpp`, 0x4d7bd0 to 0x4d8dc0, and
//! the settling in `ProCircuitProcs.cpp` 0x4dd440): the series and their
//! tracks, the money, the rivals and their rides. The screens are in
//! `screens/pro.rs`.

use crate::roster::{BikeChoice, Ride};
use crate::screens::options::engine_class;
use mm2_formats::{rs2, scn};
use std::path::{Path, PathBuf};

pub use mm2_physics::career::Tables;
use mm2_physics::career::{pay, place_points, Entry};

/// Flags (+0x464).
pub const BAILED_OUT: u32 = 1;
pub const ADVERTISED: u32 = 2;
pub const OUT: u32 = 4;
pub const FINISHED: u32 = 8;

/// `PCTables.pb`; what it leaves out keeps the code's own values
/// (0x4d7bd0).
pub fn parse_tables(text: &str) -> Tables {
    let ini = scn::Ini::parse(text);
    let mut t = Tables::default();
    let Some(s) = ini.section("ProCircuit") else {
        return t;
    };
    let num =
        |k: &str, d: f32| -> f32 { s.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(d) };
    for (i, p) in t.place_pct.iter_mut().enumerate() {
        *p = num(&format!("{}_PlacePct", i + 1), 0.0);
    }
    t.top_up();
    t.starting_capital = num("StartingCapital", t.starting_capital as f32) as i32;
    for (event, key) in [
        (1, "BajaFee"),
        (2, "NationalsFee"),
        (3, "SupercrossFee"),
        (5, "EnduroFee"),
    ] {
        t.fees[event] = num(key, t.fees[event] as f32) as i32;
    }
    t.purse_multiplier = num("PurseMultiplier", t.purse_multiplier);
    t.repair_cap = num("RepairCapPct", t.repair_cap);
    t.medical_cap = num("MedicalCapPct", t.medical_cap);
    t.stunt_cap = num("StuntCapPct", t.stunt_cap);
    t
}

fn fee(t: &Tables, event: u32) -> i32 {
    t.fees.get(event as usize).copied().unwrap_or(0)
}

/// One series of `PCSched.pb`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Series {
    /// Track file stems.
    pub tracks: Vec<String>,
    /// The bonus track's number, from 1 (0: none).
    pub bonus: usize,
    pub laps: u32,
}

/// `PCSched.pb`, by event number.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Schedule {
    pub series: [Series; 6],
}

impl Schedule {
    pub fn parse(text: &str) -> Schedule {
        let ini = scn::Ini::parse(text);
        let mut s = Schedule::default();
        for (event, name) in [
            (5, "Enduro"),
            (1, "Baja"),
            (2, "Nationals"),
            (3, "Supercross"),
        ] {
            let Some(sec) = ini.section(name) else {
                continue;
            };
            let int = |k: &str, d: i64| sec.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(d);
            let n = int("NumberOfTracks", 0).max(0) as usize;
            s.series[event] = Series {
                tracks: (1..=n)
                    .map(|i| {
                        sec.get(&format!("Track_{i}"))
                            .unwrap_or("")
                            .trim()
                            .to_string()
                    })
                    .collect(),
                bonus: int("BonusTrack", 0).max(0) as usize,
                laps: int("Laps", 3).max(1) as u32,
            };
        }
        s
    }
}

/// A bike or outfit of the lists, with the level it comes at.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub maker: String,
    pub name: String,
    pub file: String,
    /// A bike's race and menu models.
    pub model: String,
    pub ui_model: String,
    pub cc: u32,
    pub four_stroke: bool,
    pub availability: u32,
}

/// Everything the career reads from the game's files.
#[derive(Debug, Clone, Default)]
pub struct Data {
    pub tables: Tables,
    pub schedule: Schedule,
    pub names: Vec<String>,
    pub bikes: Vec<Item>,
    pub outfits: Vec<Item>,
}

impl Data {
    pub fn load(game: &Path) -> Data {
        let bytes = crate::read_archive(&game.join("UI").join("UIRES.RES")).ok();
        let ar = bytes.as_deref().and_then(|b| rs2::Archive::parse(b).ok());
        let text = |name: &str| -> Option<String> {
            ar.as_ref()?
                .get(name)
                .map(|b| String::from_utf8_lossy(b).into_owned())
        };
        let names = crate::platform::fs::read(game.join("UI").join("PCNAMES.TXT"))
            .map(|b| {
                String::from_utf8_lossy(&b)
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Data {
            tables: text("PCTables.pb")
                .map(|t| parse_tables(&t))
                .unwrap_or_default(),
            schedule: text("PCSched.pb")
                .map(|t| Schedule::parse(&t))
                .unwrap_or_default(),
            names,
            bikes: text("bikes.pb").map(|t| bikes(&t)).unwrap_or_default(),
            outfits: text("riders.pb").map(|t| outfits(&t)).unwrap_or_default(),
        }
    }

    /// The bikes the player may ride at a level, in the class.
    pub fn bikes_for(&self, level: u32, class: u32) -> Vec<&Item> {
        self.bikes
            .iter()
            .filter(|b| b.availability <= level && class_fits(class, engine_class(b.cc)))
            .collect()
    }

    pub fn outfits_for(&self, level: u32) -> Vec<&Item> {
        self.outfits
            .iter()
            .filter(|o| o.availability <= level)
            .collect()
    }
}

fn bikes(text: &str) -> Vec<Item> {
    let ini = scn::Ini::parse(text);
    let int = |s: &scn::Section, k: &str| s.get(k).and_then(|v| v.trim().parse::<i64>().ok());
    let makers = ini
        .section("Information")
        .and_then(|s| int(s, "NumberOfManufacturers"))
        .unwrap_or(0);
    let mut out = Vec::new();
    for m in 1..=makers {
        let Some(s) = ini.section(&format!("Manufacturer_{m:02}")) else {
            continue;
        };
        let maker = s.get("Name").unwrap_or("").trim_matches('"').to_string();
        let model = s.get("ModelFile").unwrap_or("").trim().to_string();
        let ui_model = s.get("UIModelFile").unwrap_or("").trim().to_string();
        let availability = int(s, "Availability").unwrap_or(1) as u32;
        for b in 1..=int(s, "NumberOfBikes").unwrap_or(0) {
            let key = |k: &str| {
                s.get(&format!("{k}_{b:02}"))
                    .map(|v| v.trim().trim_matches('"').to_string())
            };
            out.push(Item {
                maker: maker.clone(),
                name: key("BikeName").unwrap_or_default(),
                file: key("BikeTexture").unwrap_or_default(),
                model: model.clone(),
                ui_model: ui_model.clone(),
                cc: key("BikeEngineSize")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(250),
                four_stroke: key("BikeFourStroke").is_some_and(|v| v == "1"),
                availability,
            });
        }
    }
    out
}

fn outfits(text: &str) -> Vec<Item> {
    let ini = scn::Ini::parse(text);
    let count: i64 = ini
        .section("Information")
        .and_then(|s| s.get("NumberOfRiders"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    (1..=count)
        .filter_map(|r| ini.section(&format!("Rider_{r:02}")))
        .map(|s| Item {
            maker: String::new(),
            name: s
                .get("Name")
                .unwrap_or("")
                .trim()
                .trim_matches('"')
                .to_string(),
            file: s.get("File").unwrap_or("").trim().to_string(),
            model: String::new(),
            ui_model: String::new(),
            cc: 0,
            four_stroke: false,
            availability: s
                .get("Availability")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(1),
        })
        .collect()
}

/// Whether an engine class (0–4) races in a circuit's class (1 125, 2
/// 250, 3 Open).
pub fn class_fits(class: u32, engine: usize) -> bool {
    match class {
        1 => engine == 0,
        2 => engine == 1 || engine == 2,
        _ => engine >= 3,
    }
}

/// One race's or the season's amounts.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Tally {
    pub points: i32,
    pub winnings: i32,
    pub repairs: i32,
    pub medical: i32,
    pub stunt: i32,
}

/// One rider of a circuit (+0x465, 0x140 bytes).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rider {
    pub name: String,
    /// "Honda", the bike's short name "CR250", its paint, its size.
    pub maker: String,
    pub bike: String,
    pub paint: String,
    pub cc: u32,
    pub four_stroke: bool,
    pub outfit: String,
    pub race: Tally,
    pub total: Tally,
    pub race_rank: usize,
    pub rank: usize,
    pub cash: i32,
    pub debts: i32,
}

impl Rider {
    pub fn expenses(&self) -> i32 {
        self.total.repairs + self.total.medical
    }
}

/// What a race reports for one racer.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Finish {
    /// The circuit's rider.
    pub rider: usize,
    /// 1st and on; 0 or past the field for a racer who did not finish.
    pub place: usize,
    /// The race's counts of repair, medical and stunt money units
    /// (bike +0x7ac, +0x7b0, +0x7b4).
    pub repairs: f32,
    pub medical: f32,
    pub stunt: f32,
}

/// A small random source (the game uses the C library's).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub fn from_time() -> Rng {
        let t = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Rng::new(t)
    }

    /// 0 to 1.
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn below(&mut self, n: usize) -> usize {
        ((self.next() * n as f32) as usize).min(n.saturating_sub(1))
    }
}

/// A career.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Circuit {
    pub name: String,
    /// The series' event number.
    pub series: u32,
    /// The race in the series, from 1.
    pub race: usize,
    pub level: u32,
    /// 1 125, 2 250, 3 Open.
    pub class: u32,
    /// 1 Easy, 2 Normal, 3 Hard.
    pub difficulty: u32,
    pub announcement: String,
    pub fees_paid: i32,
    pub flags: u32,
    pub riders: Vec<Rider>,
}

impl Circuit {
    /// A new career (0x4d8250): the player first, on the first bike and
    /// outfit of the class at level 1, and rivals with random names and
    /// rides; `welcome` is strings 5308 to 5311.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        data: &Data,
        name: &str,
        player: &str,
        difficulty: u32,
        class: u32,
        riders: usize,
        welcome: String,
        rng: &mut Rng,
    ) -> Circuit {
        let mut c = Circuit {
            name: name.chars().take(63).collect(),
            series: 5,
            race: 1,
            level: 1,
            class: class.clamp(1, 3),
            difficulty: difficulty.clamp(1, 3),
            announcement: welcome,
            ..Default::default()
        };
        let mut used = vec![false; data.names.len()];
        for i in 0..riders.max(1) {
            let name = if i == 0 {
                player.chars().take(31).collect()
            } else if used.iter().all(|&u| u) {
                format!("Rider {}", i + 1)
            } else {
                let mut n = rng.below(used.len());
                while used[n] {
                    n = rng.below(used.len());
                }
                used[n] = true;
                data.names[n].clone()
            };
            c.riders.push(Rider {
                name,
                cash: data.tables.starting_capital,
                ..Default::default()
            });
        }
        if let Some(o) = data.outfits.iter().find(|o| o.availability == 1) {
            c.riders[0].outfit = o.file.clone();
        }
        if let Some(b) = data
            .bikes
            .iter()
            .find(|b| b.availability <= 1 && class_fits(c.class, engine_class(b.cc)))
            .or_else(|| data.bikes_for(3, c.class).first().copied())
        {
            ride(&mut c.riders[0], b);
        }
        c.pick_rides(data, rng);
        c.rank();
        c
    }

    /// The rivals' bikes and outfits for the level (0x4d8dc0): the best
    /// available, each different.
    pub fn pick_rides(&mut self, data: &Data, rng: &mut Rng) {
        let best_outfit = data
            .outfits
            .iter()
            .map(|o| o.availability)
            .filter(|&a| a <= self.level)
            .max()
            .unwrap_or(0);
        let best_bike = data
            .bikes
            .iter()
            .map(|b| b.availability)
            .filter(|&a| a <= self.level)
            .max()
            .unwrap_or(0);
        let outfits: Vec<&Item> = data
            .outfits
            .iter()
            .filter(|o| o.availability == best_outfit)
            .collect();
        let bikes: Vec<&Item> = data
            .bikes
            .iter()
            .filter(|b| b.availability == best_bike && class_fits(self.class, engine_class(b.cc)))
            .collect();
        let mut used_outfits = vec![false; outfits.len()];
        let mut used_bikes = vec![false; bikes.len()];
        for r in self.riders.iter_mut().skip(1) {
            if !outfits.is_empty() {
                if used_outfits.iter().all(|&u| u) {
                    used_outfits.fill(false);
                }
                let mut n = rng.below(outfits.len());
                while used_outfits[n] {
                    n = rng.below(outfits.len());
                }
                used_outfits[n] = true;
                r.outfit = outfits[n].file.clone();
            }
            if !bikes.is_empty() {
                if used_bikes.iter().all(|&u| u) {
                    used_bikes.fill(false);
                }
                // The 250s mix in a few 400s (0x4d8f0b).
                let n = loop {
                    let n = rng.below(bikes.len());
                    if used_bikes[n] {
                        continue;
                    }
                    let e = engine_class(bikes[n].cc);
                    let r = rng.next();
                    let keep = self.class != 2
                        || (e == 1 && r <= 0.8)
                        || (e == 2 && r > 0.8)
                        || bikes.iter().all(|b| engine_class(b.cc) == e);
                    if keep {
                        break n;
                    }
                };
                used_bikes[n] = true;
                ride(r, bikes[n]);
            }
        }
    }

    pub fn player(&self) -> &Rider {
        &self.riders[0]
    }

    pub fn track(&self, data: &Data) -> Option<String> {
        data.schedule.series[self.series as usize]
            .tracks
            .get(self.race.checked_sub(1)?)
            .cloned()
    }

    /// Whether this race is the series' bonus track.
    pub fn bonus_race(&self, data: &Data) -> bool {
        let b = data.schedule.series[self.series as usize].bonus;
        b != 0 && self.race == b
    }

    /// The race's money and points (0x4dd440).
    pub fn settle(&mut self, data: &Data, finishes: &[Finish]) {
        let entries: Vec<Entry> = finishes
            .iter()
            .map(|f| Entry {
                place: f.place,
                points: place_points(f.place),
                repair: f.repairs,
                medical: f.medical,
                stunts: f.stunt,
            })
            .collect();
        let event = self.series as usize;
        let paid = pay(&data.tables, event, self.riders.len(), &entries);
        self.fees_paid += fee(&data.tables, self.series);
        for (f, (e, p)) in finishes.iter().zip(entries.iter().zip(paid)) {
            let Some(r) = self.riders.get_mut(f.rider) else {
                continue;
            };
            let race = Tally {
                points: e.points,
                winnings: p.prize,
                repairs: p.repair,
                medical: p.medical,
                stunt: p.stunts,
            };
            r.cash += p.net;
            r.total.points += race.points;
            r.total.winnings += race.winnings;
            r.total.repairs += race.repairs;
            r.total.medical += race.medical;
            r.total.stunt += race.stunt;
            r.race = race;
        }
        let order = ranking(&self.riders, |r| r.race.points);
        for (i, &r) in order.iter().enumerate() {
            self.riders[r].race_rank = i + 1;
        }
        self.rank();
    }

    /// The overall ranks, by season points.
    pub fn rank(&mut self) {
        let order = ranking(&self.riders, |r| r.total.points);
        for (i, &r) in order.iter().enumerate() {
            self.riders[r].rank = i + 1;
        }
    }

    /// Whether the player cannot pay the series' fee.
    pub fn broke(&self, data: &Data) -> bool {
        self.player().cash < fee(&data.tables, self.series)
    }

    /// The next race (0x4d88f0); `text` gives the game's strings. A new
    /// level re-picks the rivals' rides.
    pub fn advance(&mut self, data: &Data, rng: &mut Rng, text: impl Fn(u32) -> String) {
        self.race += 1;
        let s = &data.schedule.series[self.series as usize];
        let leader = self.player().rank <= 1;
        let over = self.race > s.tracks.len() || (s.bonus != 0 && self.race == s.bonus && !leader);
        let mut news = String::new();
        if over {
            match self.series {
                5 => {
                    self.series = 1;
                    self.race = 1;
                    self.level = 2;
                    self.pick_rides(data, rng);
                    news = text(5212);
                }
                1 => {
                    self.series = 2;
                    self.race = 1;
                    self.level = 3;
                    self.pick_rides(data, rng);
                    news = text(5212);
                }
                2 => {
                    self.series = 3;
                    self.race = 1;
                    news = text(5212);
                }
                _ => {
                    self.flags |= FINISHED;
                    self.race -= 1;
                }
            }
        }
        let base = match self.series {
            1 => 5289,
            2 => 5349,
            3 => 5399,
            _ => 5279,
        };
        let about = text(base + self.race as u32);
        self.announcement = if news.is_empty() {
            about
        } else {
            format!("{news}\n\n{about}")
        };
    }

    /// Takes a bailout or advertising offer: the fee's shortfall as cash
    /// owed (0x4dd7c0).
    pub fn bail_out(&mut self, data: &Data) -> i32 {
        let need = (fee(&data.tables, self.series) - self.player().cash).max(0);
        let p = &mut self.riders[0];
        p.cash += need;
        p.debts += need;
        need
    }

    /// The player's ride (the Bike & Rider page).
    pub fn set_ride(&mut self, bike: &Item, outfit: &Item) {
        ride(&mut self.riders[0], bike);
        self.riders[0].outfit = outfit.file.clone();
    }

    pub fn done(&self) -> bool {
        self.flags & (OUT | FINISHED) != 0
    }

    // ---- Saving ----

    pub fn text(&self) -> String {
        let mut o = format!(
            "[Circuit]\nName={}\nSeries={}\nRace={}\nLevel={}\nClass={}\nDifficulty={}\nFees={}\nFlags={}\nAnnouncement={}\n",
            self.name,
            self.series,
            self.race,
            self.level,
            self.class,
            self.difficulty,
            self.fees_paid,
            self.flags,
            self.announcement.replace('\n', "\\n"),
        );
        let tally = |t: &Tally| {
            format!(
                "{},{},{},{},{}",
                t.points, t.winnings, t.repairs, t.medical, t.stunt
            )
        };
        for (i, r) in self.riders.iter().enumerate() {
            o += &format!(
                "\n[Rider{i}]\nName={}\nMaker={}\nBike={}\nPaint={}\nCC={}\nFourStroke={}\nOutfit={}\nRace={}\nTotal={}\nRaceRank={}\nRank={}\nCash={}\nDebts={}\n",
                r.name,
                r.maker,
                r.bike,
                r.paint,
                r.cc,
                r.four_stroke as u8,
                r.outfit,
                tally(&r.race),
                tally(&r.total),
                r.race_rank,
                r.rank,
                r.cash,
                r.debts,
            );
        }
        o
    }

    pub fn parse(text: &str) -> Option<Circuit> {
        let ini = scn::Ini::parse(text);
        let s = ini.section("Circuit")?;
        let get = |s: &scn::Section, k: &str| s.get(k).unwrap_or("").to_string();
        let num = |s: &scn::Section, k: &str| {
            s.get(k)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(0)
        };
        let tally = |v: String| {
            let n: Vec<i32> = v
                .split(',')
                .map(|x| x.trim().parse().unwrap_or(0))
                .collect();
            let g = |i: usize| n.get(i).copied().unwrap_or(0);
            Tally {
                points: g(0),
                winnings: g(1),
                repairs: g(2),
                medical: g(3),
                stunt: g(4),
            }
        };
        let mut c = Circuit {
            name: get(s, "Name"),
            series: num(s, "Series") as u32,
            race: num(s, "Race").max(1) as usize,
            level: num(s, "Level").clamp(1, 3) as u32,
            class: num(s, "Class").clamp(1, 3) as u32,
            difficulty: num(s, "Difficulty").clamp(1, 3) as u32,
            announcement: get(s, "Announcement").replace("\\n", "\n"),
            fees_paid: num(s, "Fees") as i32,
            flags: num(s, "Flags") as u32,
            riders: Vec::new(),
        };
        let mut i = 0;
        while let Some(r) = ini.section(&format!("Rider{i}")) {
            c.riders.push(Rider {
                name: get(r, "Name"),
                maker: get(r, "Maker"),
                bike: get(r, "Bike"),
                paint: get(r, "Paint"),
                cc: num(r, "CC") as u32,
                four_stroke: num(r, "FourStroke") != 0,
                outfit: get(r, "Outfit"),
                race: tally(get(r, "Race")),
                total: tally(get(r, "Total")),
                race_rank: num(r, "RaceRank") as usize,
                rank: num(r, "Rank") as usize,
                cash: num(r, "Cash") as i32,
                debts: num(r, "Debts") as i32,
            });
            i += 1;
        }
        (!c.riders.is_empty()).then_some(c)
    }
}

impl Item {
    /// A bike as the Bike & Rider page and the race take it.
    pub fn choice(&self) -> BikeChoice {
        BikeChoice {
            name: format!("{} {}", self.maker, self.name),
            model: self.model.clone(),
            ui_model: self.ui_model.clone(),
            texture: self.file.clone(),
            cc: self.cc,
            four_stroke: self.four_stroke,
        }
    }
}

impl Data {
    /// The player's ride in a career.
    pub fn ride(&self, c: &Circuit, plate: u32) -> Option<Ride> {
        let p = c.player();
        let bike = self
            .bikes
            .iter()
            .find(|b| b.file.eq_ignore_ascii_case(&p.paint))
            .or_else(|| self.bikes_for(c.level, c.class).first().copied())?;
        Some(Ride {
            bike: bike.choice(),
            outfit: p.outfit.clone(),
            plate,
        })
    }
}

impl Data {
    /// Every racer's ride in a career race, the player's left out (racer
    /// `i` is rider `i`): each rival's bike and paint and outfit.
    pub fn rides(&self, c: &Circuit) -> Vec<Option<Ride>> {
        c.riders
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let bike = self
                    .bikes
                    .iter()
                    .find(|b| b.file.eq_ignore_ascii_case(&r.paint))?;
                (i > 0).then(|| Ride {
                    bike: bike.choice(),
                    outfit: r.outfit.clone(),
                    plate: crate::plate_number(i),
                })
            })
            .collect()
    }
}

fn ride(r: &mut Rider, b: &Item) {
    r.maker = b.maker.clone();
    r.bike = b.name.clone();
    r.paint = b.file.clone();
    r.cc = b.cc;
    r.four_stroke = b.four_stroke;
}

/// Riders by a score, highest first; ties keep their order.
fn ranking(riders: &[Rider], score: impl Fn(&Rider) -> i32) -> Vec<usize> {
    let mut order: Vec<usize> = (0..riders.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(score(&riders[i])));
    order
}

/// A player's careers, as files in the user's data folder.
pub struct Careers {
    dir: PathBuf,
}

impl Careers {
    pub fn new(user: &Path, profile: &str) -> Careers {
        Careers {
            dir: user.join("procircuit").join(profile),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || " _-()".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{safe}.txt"))
    }

    pub fn list(&self) -> Vec<Circuit> {
        let mut v: Vec<Circuit> = crate::platform::fs::read_dir(&self.dir)
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .filter_map(|e| crate::platform::fs::read_to_string(e.path()).ok())
                    .filter_map(|t| Circuit::parse(&t))
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|c| c.name.to_ascii_lowercase());
        v
    }

    pub fn save(&self, c: &Circuit) -> std::io::Result<()> {
        crate::platform::fs::create_dir_all(&self.dir)?;
        crate::platform::fs::write(self.path(&c.name), c.text())
    }

    pub fn remove(&self, name: &str) -> std::io::Result<()> {
        crate::platform::fs::remove_file(self.path(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> Data {
        let mut d = Data {
            tables: parse_tables(
                "[ProCircuit]\nStartingCapital = 50000\nEnduroFee = 3000\nBajaFee = 5000\n\
                 NationalsFee = 15000\nSupercrossFee = 25000\nPurseMultiplier = 17.5\n\
                 RepairCapPct = 0.075\nMedicalCapPct = 0.0375\nStuntCapPct = 0.075\n\
                 1_PlacePct = 0.33\n2_PlacePct = 0.23\n3_PlacePct = 0.14\n4_PlacePct = 0.09\n\
                 5_PlacePct = 0.07\n6_PlacePct = 0.05\n7_PlacePct = 0.04\n8_PlacePct = 0.03\n9_PlacePct = 0.02\n",
            ),
            schedule: Schedule::parse(
                "[Enduro]\nNumberOfTracks = 2\nLaps = 5\nTrack_1 = Farm\nTrack_2 = Trailer\n\
                 [Baja]\nNumberOfTracks = 1\nTrack_1 = Baja01\n\
                 [Nationals]\nNumberOfTracks = 2\nBonusTrack = 2\nTrack_1 = Nat01\nTrack_2 = Chanos\n\
                 [Supercross]\nNumberOfTracks = 1\nTrack_1 = SX01\n",
            ),
            names: vec!["Lara".into(), "Pete".into(), "Dennis".into()],
            ..Default::default()
        };
        for (maker, a) in [("Generic", 1), ("Kazuyuki", 2), ("Honda", 3)] {
            for cc in [125, 250, 400, 500] {
                d.bikes.push(Item {
                    maker: maker.into(),
                    name: format!("{cc}"),
                    file: format!("{maker}{cc}.tga"),
                    model: String::new(),
                    ui_model: String::new(),
                    cc,
                    four_stroke: false,
                    availability: a,
                });
            }
            d.outfits.push(Item {
                maker: String::new(),
                name: maker.into(),
                file: format!("{maker}.tga"),
                model: String::new(),
                ui_model: String::new(),
                cc: 0,
                four_stroke: false,
                availability: a,
            });
        }
        d
    }

    #[test]
    fn tables() {
        let d = data();
        assert_eq!(fee(&d.tables, 5), 3000);
        assert_eq!(d.tables.purse(5), 52500);
        // The shares add to 1, so nothing is added to 1st.
        assert!((d.tables.place_pct[0] - 0.33).abs() < 1e-6);
        assert_eq!(d.schedule.series[5].laps, 5);
        assert_eq!(d.schedule.series[1].laps, 3);
        assert_eq!(d.schedule.series[2].bonus, 2);
    }

    #[test]
    fn career() {
        let d = data();
        let mut rng = Rng::new(7);
        let mut c = Circuit::new(&d, "Test", "Rider", 2, 2, 4, "Welcome".into(), &mut rng);
        assert_eq!(c.riders.len(), 4);
        assert_eq!(c.player().name, "Rider");
        assert_eq!(c.player().maker, "Generic");
        assert!(matches!(c.player().cc, 250 | 400));
        let names: std::collections::HashSet<_> = c.riders.iter().map(|r| r.name.clone()).collect();
        assert_eq!(names.len(), 4);
        assert_eq!(c.track(&d).as_deref(), Some("Farm"));

        // Player wins, rider 1 second, 2 third and crashes, 3 out.
        let finishes = [
            Finish {
                rider: 0,
                place: 1,
                stunt: 2.0,
                ..Default::default()
            },
            Finish {
                rider: 1,
                place: 2,
                ..Default::default()
            },
            Finish {
                rider: 2,
                place: 3,
                repairs: 100.0,
                medical: 1.0,
                ..Default::default()
            },
            Finish {
                rider: 3,
                place: 0,
                ..Default::default()
            },
        ];
        c.settle(&d, &finishes);
        let purse = 52500.0;
        let sum = 0.33 + 0.23 + 0.14 + 0.09;
        let w1 = (purse * 0.33 / sum) as i32;
        let w2 = (purse * 0.23 / sum) as i32;
        let w3 = (purse * 0.14 / sum) as i32;
        let rest = 52500 - w1 - w2 - w3;
        assert_eq!(c.riders[0].race.points, 20);
        assert_eq!(c.riders[0].race.stunt, 800);
        assert_eq!(c.riders[0].race.winnings, w1 + rest);
        assert_eq!(c.riders[0].cash, 50000 - 3000 + 800 + w1 + rest);
        assert_eq!(c.riders[2].race.repairs, (purse * 0.075) as i32);
        assert_eq!(c.riders[2].race.medical, 250);
        assert_eq!(c.riders[3].cash, 47000);
        assert_eq!(
            (c.riders[0].rank, c.riders[1].rank, c.riders[2].rank),
            (1, 2, 3)
        );

        // Through the series: Enduro → Baja at level 2 → Nationals.
        let text = |id: u32| format!("#{id}");
        c.advance(&d, &mut rng, text);
        assert_eq!((c.series, c.race), (5, 2));
        assert_eq!(c.announcement, "#5281");
        c.advance(&d, &mut rng, text);
        assert_eq!((c.series, c.race, c.level), (1, 1, 2));
        assert!(c.announcement.starts_with("#5212"));
        assert_eq!(c.riders[1].maker, "Kazuyuki");
        c.advance(&d, &mut rng, text);
        assert_eq!((c.series, c.race, c.level), (2, 1, 3));
        // The bonus track only for the leader.
        c.riders[0].total.points = 0;
        c.rank();
        c.advance(&d, &mut rng, text);
        assert_eq!((c.series, c.race), (3, 1));
        c.advance(&d, &mut rng, text);
        assert!(c.done());

        // Saved and read back whole.
        let back = Circuit::parse(&c.text()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn bailout() {
        let d = data();
        let mut c = Circuit::new(&d, "T", "P", 1, 1, 2, String::new(), &mut Rng::new(3));
        c.riders[0].cash = 1000;
        assert!(c.broke(&d));
        assert_eq!(c.bail_out(&d), 2000);
        assert_eq!((c.player().cash, c.player().debts), (3000, 2000));
        assert!(!c.broke(&d));
    }

    /// The game's own files, when `MM2_DATA` names its folder.
    #[test]
    fn game_files() {
        let Some(game) = std::env::var_os("MM2_DATA") else {
            return;
        };
        let d = Data::load(Path::new(&game));
        assert_eq!(d.tables, Tables::shipped());
        assert_eq!(d.schedule.series[2].tracks.len(), 16);
        assert_eq!(d.schedule.series[2].bonus, 16);
        assert_eq!(d.names.len(), 21);
        assert!(d
            .bikes
            .iter()
            .any(|b| b.maker == "Honda" && b.name == "CR250"));
        let mut rng = Rng::new(1);
        for class in 1..=3 {
            let c = Circuit::new(&d, "T", "P", 2, class, 8, String::new(), &mut rng);
            assert!(!c.player().bike.is_empty(), "class {class}");
            assert!(c
                .riders
                .iter()
                .all(|r| !r.outfit.is_empty() && !r.bike.is_empty()));
        }
    }
}
