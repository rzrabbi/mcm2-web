//! Pro Circuit, from `UI/PC*.DAT`. Screens covered: picking a saved
//! career (`PCStartupDlg` `0x4ddce0`), starting one (`PCNewDlg`
//! `0x4d9470`), the career hub (`PCCentralDlg` `0x4da6c0`) whose tabs
//! (`0x4dacc0`) are Next Race, Bike & Rider and Standings, results after a
//! race (`0x4dce60`), sponsor offers (bailout `0x4dd7c0`, advertising
//! `0x4de060`, bonus track `0x4de4d0`), the screens shown between series
//! (`0x4de7d0`), when a career fails (`0x4ddb40`) or is completed
//! (`0x4de9b0`), and the final table (`0x4ddce0`). Game rules live in
//! `procircuit.rs`.
use super::{options::Options, popup, Cx, Go, MsgBox, Screen};
use crate::frontend::{
    list_tracks, preview_image, EventKind, RaceResults, Setup, TrackEntry, PRACTICE,
};
use crate::procircuit::{self as pc, Careers, Circuit, Data, Finish, Rng};
use crate::roster::Ride;
use crate::ui::{Dlg, Event, Gfx, Ui};
use std::collections::HashMap;

/// Where the career screen's pages go (`0x4dacc0`).
const PAGE: (f32, f32) = (16.0, 41.0);

/// The Bike & Rider page's window for the turning bike: between the
/// outfit and bike lists, inside the page's corner marks.
const WINDOW: [f32; 4] = [82.0, 115.0, 452.0, 220.0];

/// The text colours of the offers' amounts (BGR): money in hand, and
/// money owed.
const GREEN: i32 = 0xff00;
const RED: i32 = 0xff;

/// The bonus offer's cash (`0x4de4d0`, added as the offer opens).
const BONUS_CASH: i32 = 3000;
/// The advertising offer's mascot bonus, as it shows (`0x4de30f`); the
/// code pays nothing for it.
const MASCOT_BONUS: i32 = 1000;

fn money(n: i32) -> String {
    format!("${n}")
}

fn class_name(ui: &Ui, class: u32) -> String {
    match class {
        1 => "125".into(),
        2 => "250".into(),
        _ => ui.lang(5327),
    }
}

fn difficulty_name(ui: &Ui, d: u32) -> String {
    ui.lang(5322 + d.clamp(1, 3))
}

fn event_name(ui: &Ui, series: u32) -> String {
    ui.lang(match series {
        1 => 5101,
        2 => 5103,
        3 => 5106,
        _ => 5104,
    })
}

/// A sort key of a list's cell.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    N(i64),
    T(String),
}

fn num(text: String, n: i64) -> (String, Key) {
    (text, Key::N(n))
}

fn text(text: String) -> (String, Key) {
    let key = Key::T(text.to_ascii_lowercase());
    (text, key)
}

/// Side by side list boxes that make one table, with a header button over
/// each that sorts by its column (`0x4742c0`).
struct Table {
    /// Each column's button and list.
    cols: Vec<(&'static str, &'static str)>,
    rows: Vec<Vec<(String, Key)>>,
    /// The rows in the order shown.
    order: Vec<usize>,
    /// The row picked, if the lines can be picked.
    picked: Option<usize>,
    selectable: bool,
}

impl Table {
    fn new(
        cols: &[(&'static str, &'static str)],
        rows: Vec<Vec<(String, Key)>>,
        selectable: bool,
    ) -> Table {
        let order = (0..rows.len()).collect();
        Table {
            cols: cols.to_vec(),
            picked: (selectable && !rows.is_empty()).then_some(0),
            rows,
            order,
            selectable,
        }
    }

    fn show(&self, d: &mut Dlg) {
        let line = self
            .picked
            .and_then(|p| self.order.iter().position(|&r| r == p));
        for (c, (_, list)) in self.cols.iter().enumerate() {
            let items = self
                .order
                .iter()
                .map(|&r| self.rows[r][c].0.clone())
                .collect();
            d.set_items(list, items, 0);
            d.set_selectable(list, self.selectable);
            d.mark(list, line);
        }
    }

    /// Sorts by a column: names A to Z and ranks from 1st, amounts most
    /// first.
    fn sort(&mut self, c: usize) {
        let up = self.cols[c].0.to_ascii_lowercase().contains("rank");
        let rows = &self.rows;
        self.order.sort_by(|&a, &b| {
            let (x, y) = (&rows[a][c].1, &rows[b][c].1);
            match x {
                Key::N(_) if !up => y.cmp(x),
                _ => x.cmp(y),
            }
        });
    }

    /// A header button or a line clicked; whether it was this table's.
    fn event(&mut self, d: &mut Dlg, e: &Event) -> bool {
        match e {
            Event::Clicked(n) => {
                let Some(c) = self
                    .cols
                    .iter()
                    .position(|(b, _)| b.eq_ignore_ascii_case(n))
                else {
                    return false;
                };
                self.sort(c);
            }
            Event::Selected(n, line) => {
                if !self.cols.iter().any(|(_, l)| l.eq_ignore_ascii_case(n)) {
                    return false;
                }
                self.picked = self.order.get(*line).copied();
            }
            _ => return false,
        }
        self.show(d);
        true
    }
}

/// The columns of the standings lists, by the career's riders.
fn standings(
    ui: &Ui,
    c: &Circuit,
    columns: &[(&'static str, &'static str)],
    last_race: bool,
) -> Table {
    let rows = c
        .riders
        .iter()
        .map(|r| {
            columns
                .iter()
                .map(|(_, list)| match list.to_ascii_lowercase().as_str() {
                    "lstrank" if last_race => num(r.race_rank.to_string(), r.race_rank as i64),
                    "lstrank" => num(r.rank.to_string(), r.rank as i64),
                    "lstname" => text(r.name.clone()),
                    "lstpoints" if last_race => {
                        num(r.race.points.to_string(), r.race.points as i64)
                    }
                    "lstpoints" => num(r.total.points.to_string(), r.total.points as i64),
                    "lstbike" => text(r.bike.clone()),
                    "lstcash" => num(money(r.cash), r.cash as i64),
                    "lstwinnings" if last_race => {
                        num(money(r.race.winnings), r.race.winnings as i64)
                    }
                    "lstwinnings" => num(money(r.total.winnings), r.total.winnings as i64),
                    "lstrepairs" if last_race => num(money(r.race.repairs), r.race.repairs as i64),
                    "lstrepairs" => num(money(r.total.repairs), r.total.repairs as i64),
                    "lstmedical" if last_race => num(money(r.race.medical), r.race.medical as i64),
                    "lstmedical" => num(money(r.total.medical), r.total.medical as i64),
                    "lstexpenses" => num(money(r.expenses()), r.expenses() as i64),
                    _ => text(String::new()),
                })
                .collect()
        })
        .collect();
    let _ = ui;
    let mut t = Table::new(columns, rows, false);
    t.sort(0);
    t
}

const STANDINGS: [(&str, &str); 7] = [
    ("ButRank", "LstRank"),
    ("ButName", "LstName"),
    ("ButPoints", "LstPoints"),
    ("ButBike", "LstBike"),
    ("ButCash", "LstCash"),
    ("ButWinnings", "LstWinnings"),
    ("ButExpenses", "LstExpenses"),
];

const LAST_RACE: [(&str, &str); 7] = [
    ("ButRank", "LstRank"),
    ("ButName", "LstName"),
    ("ButPoints", "LstPoints"),
    ("ButBike", "LstBike"),
    ("ButWinnings", "LstWinnings"),
    ("ButRepairs", "LstRepairs"),
    ("ButMedical", "LstMedical"),
];

const FINAL: [(&str, &str); 7] = [
    ("ButRank", "LstRank"),
    ("ButName", "LstName"),
    ("ButPoints", "LstPoints"),
    ("ButBike", "LstBike"),
    ("ButCash", "LstCash"),
    ("ButRepairs", "LstRepairs"),
    ("ButMedical", "LstMedical"),
];

const CURRENT: [(&str, &str); 6] = [
    ("ButName", "LstName"),
    ("ButRank", "LstRank"),
    ("ButClass", "LstClass"),
    ("ButDiff", "LstDiff"),
    ("ButPoints", "LstPoints"),
    ("ButCash", "LstCash"),
];

const DONE: [(&str, &str); 7] = [
    ("ButDoneName", "LstDoneName"),
    ("ButDoneRank", "LstDoneRank"),
    ("ButDoneClass", "LstDoneClass"),
    ("ButDoneDiff", "LstDoneDiff"),
    ("ButDonePoints", "LstDonePoints"),
    ("ButDoneCash", "LstDoneCash"),
    ("ButDoneStatus", "LstDoneStatus"),
];

/// The PCStart lists' row for a career.
fn career_row(ui: &Ui, c: &Circuit, done: bool) -> Vec<(String, Key)> {
    let p = c.player();
    let mut row = vec![
        text(c.name.clone()),
        num(format!("{} / {}", p.rank, c.riders.len()), p.rank as i64),
        num(class_name(ui, c.class), c.class as i64),
        num(difficulty_name(ui, c.difficulty), c.difficulty as i64),
        num(p.total.points.to_string(), p.total.points as i64),
        num(money(p.cash), p.cash as i64),
    ];
    if done {
        let failed = c.flags & pc::OUT != 0;
        row.push(text(ui.lang(if failed { 5186 } else { 5187 })));
    }
    row
}

/// The offers after a race.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Deal {
    Bailout,
    Advertising,
    Bonus,
}

/// The pages between the career's screens (PCTrans).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Trans {
    /// A new series begins.
    Series,
    Failed,
    Won,
}

enum Popup {
    /// The new career box, and its number of opponents.
    New(Dlg, usize),
    /// Deleting a career, by name.
    Delete(MsgBox, String),
}

enum View {
    Start {
        dlg: Dlg,
        current: Table,
        done: Table,
        careers: Vec<Circuit>,
        finished: Vec<Circuit>,
        popup: Option<Popup>,
    },
    Central {
        base: Dlg,
        page: Dlg,
        /// 0 Next Race, 1 Bike & Rider, 2 Standings.
        tab: usize,
        table: Option<Table>,
    },
    Results(Dlg, Table),
    Deal(Dlg, Deal, i32),
    Trans(Dlg, Trans),
    Finish(Dlg, Table),
}

pub struct Pro {
    data: Data,
    careers: Careers,
    rng: Rng,
    view: View,
    /// The career being played.
    c: Option<Circuit>,
    tracks: HashMap<u32, Vec<TrackEntry>>,
    /// A series whose track files are still being fetched (browser only).
    awaiting: Option<u32>,
    /// Over the career screen: the options screen.
    sub: Option<Box<dyn Screen>>,
    /// A race is on: whether it counts (Start) or is practice, and
    /// whether it is the free bonus race.
    racing: Option<(bool, bool)>,
}

impl Pro {
    pub fn open(cx: &mut Cx) -> Option<Pro> {
        let data = Data::load(cx.game());
        let player = profile_name(cx);
        let mut p = Pro {
            data,
            careers: Careers::new(&cx.user.store.dir, &player),
            rng: Rng::from_time(),
            view: View::Trans(Dlg::open(cx.ui, "PCTRANS", (0.0, 0.0))?, Trans::Series),
            c: None,
            tracks: HashMap::new(),
            awaiting: None,
            sub: None,
            racing: None,
        };
        p.start_screen(cx);
        matches!(p.view, View::Start { .. }).then_some(p)
    }

    /// The careers list (`0x4ddce0`).
    fn start_screen(&mut self, cx: &mut Cx) {
        let Some(mut dlg) = Dlg::open(cx.ui, "PCSTART", (0.0, 0.0)) else {
            return;
        };
        let (finished, careers): (Vec<Circuit>, Vec<Circuit>) =
            self.careers.list().into_iter().partition(|c| c.done());
        let rows = careers
            .iter()
            .map(|c| career_row(cx.ui, c, false))
            .collect();
        let current = Table::new(&CURRENT, rows, true);
        let rows = finished
            .iter()
            .map(|c| career_row(cx.ui, c, true))
            .collect();
        let done = Table::new(&DONE, rows, true);
        current.show(&mut dlg);
        done.show(&mut dlg);
        // With nothing to pick the original hides the buttons.
        dlg.show("ButContinue", !careers.is_empty());
        dlg.show("ButDelete", !careers.is_empty());
        dlg.show("ButDoneDelete", !finished.is_empty());
        self.c = None;
        self.view = View::Start {
            dlg,
            current,
            done,
            careers,
            finished,
            popup: None,
        };
    }

    fn save(&self) {
        if let Some(c) = &self.c {
            if let Err(e) = self.careers.save(c) {
                eprintln!("saving Pro Circuit career {}: {e}", c.name);
            }
        }
    }

    /// Into a career (`0x4d92f0`): the page for a new series, or the
    /// career's screen.
    fn resume(&mut self, cx: &mut Cx) {
        let Some(c) = &self.c else {
            return;
        };
        if c.flags & pc::FINISHED != 0 {
            self.trans(cx, Trans::Won);
        } else if c.race == 1 {
            self.trans(cx, Trans::Series);
        } else {
            self.central(cx, 0);
        }
    }

    fn trans(&mut self, cx: &mut Cx, t: Trans) {
        let Some(c) = &self.c else {
            return;
        };
        let Some(mut d) = Dlg::open(cx.ui, "PCTRANS", (0.0, 0.0)) else {
            return;
        };
        let (title, about, accept) = match t {
            Trans::Series => (
                cx.ui.lang(5223),
                cx.ui.lang(match c.series {
                    1 => 5204,
                    2 => 5205,
                    3 => 5206,
                    _ => 5221,
                }),
                2326,
            ),
            Trans::Failed => (cx.ui.lang(5223), cx.ui.lang(5220), 5198),
            Trans::Won => {
                let mut about = cx.ui.lang(5214);
                if c.player().rank == 1 {
                    about = format!("{about}\n{}", cx.ui.lang(5215));
                }
                (cx.ui.lang(5213), about, 5198)
            }
        };
        if t != Trans::Series {
            self.save();
        }
        d.set_text("TitleText", title);
        d.set_text("TxtDescription", about);
        d.set_wrap("TxtDescription", true);
        d.show("ButDecline", false);
        d.set_text("ButAccept", cx.ui.lang(accept));
        self.view = View::Trans(d, t);
    }

    /// The picture of a PCTrans page.
    fn trans_picture(&self, t: Trans) -> &'static str {
        match t {
            Trans::Failed => "FailScreen",
            Trans::Won => "WinScreen",
            Trans::Series => match self.c.as_ref().map_or(5, |c| c.series) {
                1 => "BajaScreen",
                2 => "NatScreen",
                3 => "SuperScreen",
                _ => "WinScreen",
            },
        }
    }

    /// The career's screen on a page (`0x4da6c0` open).
    fn central(&mut self, cx: &mut Cx, tab: usize) {
        let Some(mut base) = Dlg::open(cx.ui, "PCC", (0.0, 0.0)) else {
            return;
        };
        for (name, id) in [
            ("TabNext", 5190),
            ("TabBikeRider", 5191),
            ("TabStandings", 5192),
        ] {
            base.set_text(name, cx.ui.lang(id));
            base.set_font(name, 18.0);
        }
        let Some(page) = Dlg::open(cx.ui, "PCCNEXT", PAGE) else {
            return;
        };
        self.view = View::Central {
            base,
            page,
            tab: 0,
            table: None,
        };
        self.page(cx, tab);
    }

    /// Switches the career screen's page (`0x4dacc0`).
    fn page(&mut self, cx: &mut Cx, tab: usize) {
        let pack = ["PCCNEXT", "PCCBIKER", "PCCSTAND"][tab.min(2)];
        let Some(mut d) = Dlg::open(cx.ui, pack, PAGE) else {
            return;
        };
        let Some(c) = self.c.clone() else {
            return;
        };
        let mut new_table = None;
        match tab {
            0 => self.fill_next(cx.ui, &mut d),
            1 => {
                let plate = cx.user.profile.plate;
                self.fill_bike(&mut d, plate);
            }
            _ => {
                let t = standings(cx.ui, &c, &STANDINGS, false);
                t.show(&mut d);
                new_table = Some(t);
            }
        }
        if let View::Central {
            base,
            page,
            tab: at,
            table,
        } = &mut self.view
        {
            for (i, name) in ["TabNext", "TabBikeRider", "TabStandings"]
                .iter()
                .enumerate()
            {
                base.set_on(name, i == tab);
            }
            *page = d;
            *at = tab;
            *table = new_table;
        }
    }

    /// The Next Race page.
    fn fill_next(&mut self, ui: &Ui, d: &mut Dlg) {
        let Some(c) = self.c.clone() else {
            return;
        };
        let track = self.entry(ui, c.series, c.track(&self.data).unwrap_or_default());
        let t = &self.data.tables;
        let fee = t.fees.get(c.series as usize).copied().unwrap_or(0);
        let count = self.data.schedule.series[c.series as usize].tracks.len();
        let p = c.player();
        d.set_text("TxtNextFee", money(fee));
        d.set_text("TxtNextPurse", money(t.purse(c.series as usize)));
        d.set_text("TxtNextTrack", track.map(|t| t.name).unwrap_or_default());
        d.set_text("TxtNextType", event_name(ui, c.series));
        d.set_text("TxtNextRace", format!("{} / {}", c.race, count));
        d.set_text("TxtCash", money(p.cash));
        d.set_text("TxtFees", money(c.fees_paid));
        d.set_text("TxtRepairs", money(p.total.repairs));
        d.set_text("TxtMedical", money(p.total.medical));
        d.set_text("TxtAnnouncement", c.announcement.clone());
        d.set_wrap("TxtAnnouncement", true);
    }

    /// The Bike & Rider page: the bikes of the career's class and level,
    /// and its outfits.
    fn fill_bike(&self, d: &mut Dlg, plate: u32) {
        let Some(c) = self.c.as_ref() else {
            return;
        };
        let bikes = self.data.bikes_for(c.level, c.class);
        let outfits = self.data.outfits_for(c.level);
        let p = c.player();
        let bike = bikes.iter().position(|b| b.file == p.paint).unwrap_or(0);
        let outfit = outfits.iter().position(|o| o.file == p.outfit).unwrap_or(0);
        d.set_items(
            "DDLBikes",
            bikes
                .iter()
                .map(|b| format!("{} {}", b.maker, b.name))
                .collect(),
            bike,
        );
        d.set_items(
            "DDLRiders",
            outfits.iter().map(|o| o.name.clone()).collect(),
            outfit,
        );
        d.set_items("DDLEngineSize", vec![format!("{}cc", p.cc)], 0);
        d.enable("DDLEngineSize", false);
        d.enable("TxtEngineSize", false);
        d.enable("ButWrench", false);
        d.set_on("ChkAutoRotate", true);
        d.set_text("EditPlateNumber", plate.to_string());
    }

    /// Takes the Bike & Rider page's picks.
    fn pick_ride(&mut self, bike: usize, outfit: usize) {
        let Some(c) = self.c.as_mut() else {
            return;
        };
        let bikes = self.data.bikes_for(c.level, c.class);
        let outfits = self.data.outfits_for(c.level);
        if let (Some(b), Some(o)) = (bikes.get(bike), outfits.get(outfit)) {
            let (b, o) = ((*b).clone(), (*o).clone());
            c.set_ride(&b, &o);
        }
        self.save();
    }

    /// A track of the schedule, by its file's stem.
    fn entry(&mut self, ui: &Ui, series: u32, stem: String) -> Option<TrackEntry> {
        let event = EventKind::from_number(series)?;
        let game = ui.game().to_path_buf();
        // In the browser the series' files may still be on their way.
        if !self.tracks.contains_key(&series) && !crate::platform::fs::ensure_folder(game.join(event.folder())) {
            self.awaiting = Some(series);
            return None;
        }
        let list = self
            .tracks
            .entry(series)
            .or_insert_with(|| list_tracks(&game.join(event.folder()), event));
        list.iter()
            .find(|t| {
                t.course == 0
                    && t.path
                        .file_stem()
                        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&stem))
            })
            .cloned()
    }

    /// Start or Practice (`0x4da6c0`): the schedule's track and laps, the
    /// career's riders and its difficulty.
    fn race(&mut self, cx: &mut Cx, counts: bool, bonus: bool) -> Go {
        let Some(c) = self.c.clone() else {
            return Go::Stay;
        };
        let Some(event) = EventKind::from_number(c.series) else {
            return Go::Stay;
        };
        let Some(track) = self.entry(cx.ui, c.series, c.track(&self.data).unwrap_or_default())
        else {
            return Go::Stay;
        };
        self.save();
        self.racing = Some((counts, bonus));
        let ride: Option<Ride> = self.data.ride(&c, cx.user.profile.plate);
        Go::Race(Box::new(Setup {
            event,
            track: track.path,
            course: track.course,
            laps: self.data.schedule.series[c.series as usize].laps,
            opponents: if counts { c.riders.len() as u32 - 1 } else { 0 },
            mode: if counts { 1 } else { PRACTICE },
            name: track.name,
            ride,
            level: (c.difficulty as usize).saturating_sub(1),
            names: c.riders.iter().map(|r| r.name.clone()).collect(),
            rides: self.data.rides(&c),
            ghost: None,
            net: None,
        }))
    }

    /// The last race's results (`0x4dce60` open): the race is settled and
    /// the career moves on to its next race.
    fn results(&mut self, cx: &mut Cx) {
        let Some(c) = self.c.as_ref() else {
            return;
        };
        let Some(mut d) = Dlg::open(cx.ui, "PCLRACE", (0.0, 0.0)) else {
            return;
        };
        let t = standings(cx.ui, c, &LAST_RACE, true);
        t.show(&mut d);
        self.view = View::Results(d, t);
    }

    /// Back from the results (`0x4dce60`): the bonus track for the
    /// leader, the offers for a player who cannot pay the next fee, or on.
    fn after_results(&mut self, cx: &mut Cx) {
        let data = &self.data;
        let Some(c) = self.c.as_mut() else {
            return;
        };
        if c.bonus_race(data) && c.player().rank == 1 {
            return self.deal(cx, Deal::Bonus);
        }
        if c.broke(data) {
            if c.flags & pc::BAILED_OUT == 0 {
                c.flags |= pc::BAILED_OUT;
                return self.deal(cx, Deal::Bailout);
            }
            if c.flags & pc::ADVERTISED == 0 {
                c.flags |= pc::ADVERTISED;
                return self.deal(cx, Deal::Advertising);
            }
            c.flags |= pc::OUT;
            return self.trans(cx, Trans::Failed);
        }
        self.resume(cx);
    }

    fn deal(&mut self, cx: &mut Cx, deal: Deal) {
        let Some(c) = self.c.as_mut() else {
            return;
        };
        let Some(mut d) = Dlg::open(cx.ui, "PCDEAL", (0.0, 0.0)) else {
            return;
        };
        let fee = self
            .data
            .tables
            .fees
            .get(c.series as usize)
            .copied()
            .unwrap_or(0);
        let (cash, debts) = (c.player().cash, c.player().debts);
        let balance = if cash < 0 { RED } else { GREEN };
        let (title, about, offer, items): (u32, u32, i32, Vec<(u32, i32, i32)>) = match deal {
            Deal::Bailout => {
                let offer = fee - cash;
                (
                    5194,
                    5193,
                    offer,
                    vec![
                        (5195, cash, balance),
                        (5197, fee, RED),
                        (0, 0, 0),
                        (5196, offer, GREEN),
                    ],
                )
            }
            Deal::Advertising => {
                let offer = fee - cash + debts;
                (
                    5202,
                    5201,
                    offer,
                    vec![
                        (5195, cash, balance),
                        (5197, fee, RED),
                        (5203, debts, RED),
                        (5199, offer, GREEN),
                        (5200, MASCOT_BONUS, GREEN),
                    ],
                )
            }
            Deal::Bonus => {
                c.riders[0].cash += BONUS_CASH;
                let (title, about) = if c.series == 3 {
                    (5208, 5210)
                } else {
                    (5207, 5209)
                };
                (title, about, BONUS_CASH, vec![(5211, BONUS_CASH, GREEN)])
            }
        };
        d.set_text("TitleText", cx.ui.lang(title));
        d.set_text("TxtDescription", cx.ui.lang(about));
        d.set_wrap("TxtDescription", true);
        for i in 0..5 {
            let (label, value) = (format!("TxtItem{}", i + 1), format!("Item{}", i + 1));
            match items.get(i) {
                Some(&(id, n, colour)) if id != 0 => {
                    d.set_text(&label, cx.ui.lang(id));
                    d.set_text(&value, money(n));
                    d.set_color(&value, colour);
                }
                _ => {
                    d.set_text(&label, "");
                    d.set_text(&value, "");
                }
            }
        }
        d.show("Back", false);
        self.save();
        self.view = View::Deal(d, deal, offer);
    }

    /// An offer taken or turned down.
    fn answer(&mut self, cx: &mut Cx, deal: Deal, offer: i32, accept: bool) -> Go {
        let data = &self.data;
        let Some(c) = self.c.as_mut() else {
            return Go::Stay;
        };
        match (deal, accept) {
            (Deal::Bailout, true) => {
                c.bail_out(data);
                self.save();
                self.central(cx, 0);
            }
            (Deal::Advertising, true) => {
                c.riders[0].cash += offer - c.riders[0].debts;
                self.save();
                self.central(cx, 0);
            }
            (Deal::Bailout | Deal::Advertising, false) => {
                c.flags |= pc::OUT;
                self.trans(cx, Trans::Failed);
            }
            (Deal::Bonus, true) => return self.race(cx, true, true),
            (Deal::Bonus, false) => {
                let lang = |id| cx.ui.lang(id);
                c.advance(data, &mut self.rng, lang);
                self.save();
                self.resume(cx);
            }
        }
        Go::Stay
    }

    fn finish(&mut self, cx: &mut Cx) {
        let Some(c) = self.c.as_ref() else {
            return;
        };
        let Some(mut d) = Dlg::open(cx.ui, "PCFINISH", (0.0, 0.0)) else {
            return;
        };
        let t = standings(cx.ui, c, &FINAL, false);
        t.show(&mut d);
        self.view = View::Finish(d, t);
    }

    /// The new career box (`0x4d9470`): 250s, Normal and 7 opponents to
    /// begin with.
    fn new_box(&mut self, cx: &mut Cx) {
        let Some(mut d) = popup(cx.ui, "PCNEW") else {
            return;
        };
        d.set_on("RadClass250", true);
        d.set_on("RadLODMedium", true);
        d.set_text("EditBox", "");
        d.edit(Some("EditBox"));
        let opponents = 7;
        d.set_items("OpponentsListBox", vec![opponents.to_string()], 0);
        if let View::Start { popup, .. } = &mut self.view {
            *popup = Some(Popup::New(d, opponents));
        }
    }

    /// OK in the new career box.
    fn create(&mut self, cx: &mut Cx, d: &Dlg, opponents: usize) -> bool {
        let name = d
            .get("EditBox")
            .and_then(|c| c.text.clone())
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty()
            || self
                .careers
                .list()
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case(&name))
        {
            return false;
        }
        let pick = |names: [&str; 3]| {
            names
                .iter()
                .position(|n| d.get(n).is_some_and(|c| c.on))
                .unwrap_or(1) as u32
                + 1
        };
        let class = pick(["RadClass125", "RadClass250", "RadClassOpen"]);
        let difficulty = pick(["RadLODEasy", "RadLODMedium", "RadLODHard"]);
        let welcome = (5308..=5311)
            .map(|id| cx.ui.lang(id))
            .collect::<Vec<_>>()
            .join("\n");
        let player = profile_name(cx);
        self.c = Some(Circuit::new(
            &self.data,
            &name,
            &player,
            difficulty,
            class,
            opponents + 1,
            welcome,
            &mut self.rng,
        ));
        self.save();
        self.resume(cx);
        true
    }

    fn start_event(&mut self, cx: &mut Cx, e: Event) -> Go {
        let View::Start {
            dlg,
            current,
            done,
            careers,
            finished,
            popup,
        } = &mut self.view
        else {
            return Go::Stay;
        };
        match popup.take() {
            Some(Popup::New(mut d, mut opponents)) => {
                match &e {
                    Event::Clicked(n) if n.eq_ignore_ascii_case("CancelButton") => return Go::Stay,
                    Event::Clicked(n) if n.eq_ignore_ascii_case("OKButton") => {
                        if !self.create(cx, &d, opponents) {
                            if let View::Start { popup, .. } = &mut self.view {
                                *popup = Some(Popup::New(d, opponents));
                            }
                        }
                        return Go::Stay;
                    }
                    Event::Released(n) if n.eq_ignore_ascii_case("EditBox") => {
                        if !self.create(cx, &d, opponents) {
                            d.edit(Some("EditBox"));
                            if let View::Start { popup, .. } = &mut self.view {
                                *popup = Some(Popup::New(d, opponents));
                            }
                        }
                        return Go::Stay;
                    }
                    Event::Clicked(n) if n.eq_ignore_ascii_case("EditBox") => {
                        d.edit(Some("EditBox"))
                    }
                    Event::Clicked(n) if n.eq_ignore_ascii_case("OpponentsLeftArrow") => {
                        opponents = (opponents - 1).max(3);
                        d.set_items("OpponentsListBox", vec![opponents.to_string()], 0);
                    }
                    Event::Clicked(n) if n.eq_ignore_ascii_case("OpponentsRightArrow") => {
                        opponents = (opponents + 1).min(10);
                        d.set_items("OpponentsListBox", vec![opponents.to_string()], 0);
                    }
                    _ => {}
                }
                *popup = Some(Popup::New(d, opponents));
                return Go::Stay;
            }
            Some(Popup::Delete(m, name)) => {
                if let Event::Clicked(n) = &e {
                    if n.eq_ignore_ascii_case("ButLeft") {
                        if let Err(err) = self.careers.remove(&name) {
                            eprintln!("deleting Pro Circuit career {name}: {err}");
                        }
                        self.start_screen(cx);
                    }
                    return Go::Stay;
                }
                *popup = Some(Popup::Delete(m, name));
                return Go::Stay;
            }
            None => {}
        }
        if current.event(dlg, &e) || done.event(dlg, &e) {
            return Go::Stay;
        }
        let Event::Clicked(n) = e else {
            return Go::Stay;
        };
        match n.to_ascii_lowercase().as_str() {
            "back" => return Go::Back,
            "butnew" => self.new_box(cx),
            "butcontinue" => {
                if let Some(c) = current.picked.and_then(|i| careers.get(i)) {
                    self.c = Some(c.clone());
                    self.resume(cx);
                }
            }
            "butdelete" | "butdonedelete" => {
                let (table, list) = if n.eq_ignore_ascii_case("butdelete") {
                    (&*current, &*careers)
                } else {
                    (&*done, &*finished)
                };
                if let Some(c) = table.picked.and_then(|i| list.get(i)) {
                    let name = c.name.clone();
                    let prompt = cx.ui.lang(5180);
                    if let Some(m) = MsgBox::ask(cx.ui, "", &prompt, [5182, 0, 5181]) {
                        *popup = Some(Popup::Delete(m, name));
                    }
                }
            }
            _ => {}
        }
        Go::Stay
    }

    fn central_event(&mut self, cx: &mut Cx, e: Event) -> Go {
        let View::Central {
            page, tab, table, ..
        } = &mut self.view
        else {
            return Go::Stay;
        };
        let tab = *tab;
        if let Some(t) = table {
            if t.event(page, &e) {
                return Go::Stay;
            }
        }
        match e {
            Event::Clicked(n) => match n.to_ascii_lowercase().as_str() {
                "back" => {
                    self.save();
                    self.start_screen(cx);
                }
                "start" => return self.race(cx, true, false),
                "practice" => return self.race(cx, false, false),
                "options" | "controls" => {
                    let at = if n.eq_ignore_ascii_case("controls") {
                        3
                    } else {
                        0
                    };
                    if let Some(o) = Options::open(cx, at) {
                        self.sub = Some(Box::new(o));
                    }
                }
                "tabnext" => self.page(cx, 0),
                "tabbikerider" => self.page(cx, 1),
                "tabstandings" => self.page(cx, 2),
                "editplatenumber" if tab == 1 => page.edit(Some("EditPlateNumber")),
                "bikeleft" | "bikeright" | "riderleft" | "riderright" if tab == 1 => {
                    let list = if n.to_ascii_lowercase().starts_with("bike") {
                        "DDLBikes"
                    } else {
                        "DDLRiders"
                    };
                    let count = page.get(list).map_or(0, |c| c.items.len()).max(1);
                    let by = if n.to_ascii_lowercase().ends_with("left") {
                        count - 1
                    } else {
                        1
                    };
                    let at = (page.selected(list) + by) % count;
                    page.select(list, at);
                    let picks = (page.selected("DDLBikes"), page.selected("DDLRiders"));
                    self.pick_ride(picks.0, picks.1);
                }
                _ => {}
            },
            Event::Selected(n, _) if tab == 1 && n.to_ascii_lowercase().starts_with("ddl") => {
                let picks = (page.selected("DDLBikes"), page.selected("DDLRiders"));
                self.pick_ride(picks.0, picks.1);
            }
            Event::Released(n) if n.eq_ignore_ascii_case("EditPlateNumber") => {
                let text = page
                    .get("EditPlateNumber")
                    .and_then(|c| c.text.clone())
                    .unwrap_or_default();
                let mut plate: u32 = text.trim().parse().unwrap_or(0);
                if plate < 100 {
                    plate += 100;
                }
                let plate = plate.clamp(101, 999);
                page.set_text("EditPlateNumber", plate.to_string());
                cx.user.profile.plate = plate;
                cx.user.save();
            }
            _ => {}
        }
        Go::Stay
    }
}

/// Whose careers: the profile's name.
fn profile_name(cx: &Cx) -> String {
    let name = cx.user.profile.name.trim();
    if name.is_empty() { "Player" } else { name }.to_string()
}

impl Screen for Pro {
    fn layers(&mut self) -> Vec<&mut Dlg> {
        if let Some(s) = &mut self.sub {
            return s.layers();
        }
        match &mut self.view {
            View::Start { dlg, popup, .. } => match popup {
                Some(Popup::New(d, _)) => vec![dlg, d],
                Some(Popup::Delete(m, _)) => vec![dlg, &mut m.dlg],
                None => vec![dlg],
            },
            View::Central { base, page, .. } => vec![base, page],
            View::Results(d, _) | View::Deal(d, ..) | View::Trans(d, _) | View::Finish(d, _) => {
                vec![d]
            }
        }
    }

    fn modal(&self) -> bool {
        if let Some(s) = &self.sub {
            return s.modal();
        }
        matches!(&self.view, View::Start { popup: Some(_), .. })
    }

    fn event(&mut self, cx: &mut Cx, e: Event) -> Go {
        if let Some(s) = &mut self.sub {
            match s.event(cx, e) {
                Go::Back => {
                    self.sub = None;
                    let tab = match &self.view {
                        View::Central { tab, .. } => *tab,
                        _ => 0,
                    };
                    self.page(cx, tab);
                }
                Go::Open(o) => self.sub = Some(o),
                Go::Quit => return Go::Quit,
                _ => {}
            }
            return Go::Stay;
        }
        let click = |n: &str| matches!(&e, Event::Clicked(c) if c.eq_ignore_ascii_case(n));
        match &mut self.view {
            View::Start { .. } => return self.start_event(cx, e),
            View::Central { .. } => return self.central_event(cx, e),
            View::Results(d, t) => {
                if !t.event(d, &e) && click("Back") {
                    self.after_results(cx);
                }
            }
            View::Deal(_, deal, offer) => {
                let (deal, offer) = (*deal, *offer);
                if click("ButAccept") {
                    return self.answer(cx, deal, offer, true);
                }
                if click("ButDecline") {
                    return self.answer(cx, deal, offer, false);
                }
            }
            View::Trans(_, t) => {
                if click("ButAccept") {
                    match *t {
                        Trans::Series => self.central(cx, 0),
                        Trans::Failed | Trans::Won => self.finish(cx),
                    }
                }
            }
            View::Finish(d, t) => {
                if !t.event(d, &e) && click("Back") {
                    self.start_screen(cx);
                }
            }
        }
        Go::Stay
    }

    fn escape(&mut self, cx: &mut Cx) -> Go {
        if let Some(s) = &mut self.sub {
            if let Go::Back = s.escape(cx) {
                self.sub = None;
                self.page(cx, 0);
            }
            return Go::Stay;
        }
        match &mut self.view {
            View::Start { popup, .. } if popup.is_some() => {
                *popup = None;
                Go::Stay
            }
            View::Start { .. } => Go::Back,
            View::Central { page, base, .. } if page.list_open() || base.list_open() => {
                page.close_list();
                base.close_list();
                Go::Stay
            }
            View::Central { .. } | View::Results(..) | View::Finish(..) => {
                self.event(cx, Event::Clicked("Back".into()))
            }
            View::Deal(..) | View::Trans(..) => Go::Stay,
        }
    }

    fn key(&mut self, cx: &mut Cx, code: u32) -> Go {
        match &mut self.sub {
            Some(s) => s.key(cx, code),
            None => Go::Stay,
        }
    }

    fn tick(&mut self, cx: &mut Cx) -> Go {
        if let Some(series) = self.awaiting {
            let ready = EventKind::from_number(series)
                .is_none_or(|e| crate::platform::fs::ensure_folder(cx.ui.game().join(e.folder())));
            if ready {
                self.awaiting = None;
                if matches!(self.view, View::Central { tab: 0, .. }) {
                    self.page(cx, 0);
                }
            }
        }
        match &mut self.sub {
            Some(s) => s.tick(cx),
            None => Go::Stay,
        }
    }

    fn race_over(&mut self, cx: &mut Cx, results: Option<&RaceResults>) {
        let racing = self.racing.take();
        let Some(c) = self.c.as_mut() else {
            return self.start_screen(cx);
        };
        match (racing, results) {
            // The bonus race pays only its bonus cash: no results, on to
            // the next race (`0x49aaa0`, 0x88e).
            (Some((_, true)), _) => {
                let ui = &*cx.ui;
                c.advance(&self.data, &mut self.rng, |id| ui.lang(id));
                self.save();
                self.resume(cx);
            }
            (Some((true, false)), Some(r)) if !r.placings.is_empty() => {
                let finishes: Vec<Finish> = r
                    .placings
                    .iter()
                    .enumerate()
                    .map(|(rider, p)| Finish {
                        rider,
                        place: p.place,
                        repairs: p.repair,
                        medical: p.medical,
                        stunt: p.stunt,
                    })
                    .collect();
                c.settle(&self.data, &finishes);
                let ui = &*cx.ui;
                c.advance(&self.data, &mut self.rng, |id| ui.lang(id));
                self.save();
                self.results(cx);
            }
            _ => self.central(cx, 0),
        }
    }

    fn preview(&self) -> Option<(Ride, bool, [f32; 4])> {
        if self.sub.is_some() {
            return None;
        }
        let View::Central { page, tab: 1, .. } = &self.view else {
            return None;
        };
        let c = self.c.as_ref()?;
        let turning = page.get("ChkAutoRotate").is_none_or(|c| c.on);
        let plate = page
            .get("EditPlateNumber")
            .and_then(|c| c.text.as_deref())
            .and_then(|t| t.parse().ok())
            .unwrap_or(101);
        Some((self.data.ride(c, plate)?, turning, WINDOW))
    }

    fn draw(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32) {
        if let Some(s) = &mut self.sub {
            return s.draw(ui, gfx, now);
        }
        // The next race's track, and the PCTrans page's picture.
        let track = match &self.view {
            View::Central { tab: 0, .. } => self
                .c
                .clone()
                .and_then(|c| self.entry(ui, c.series, c.track(&self.data).unwrap_or_default())),
            _ => None,
        };
        let picture = match &self.view {
            View::Trans(_, t) => Some(self.trans_picture(*t)),
            _ => None,
        };
        for d in self.layers() {
            d.tick(now);
            d.draw(ui, gfx, now);
        }
        match &self.view {
            View::Trans(d, _) => {
                if let (Some(name), Some(r)) = (picture, d.rect_of("Pic")) {
                    if let Some(t) = ui.image(gfx, &d.pack, name) {
                        gfx.sprites.image(t, r[0], r[1]);
                    }
                }
            }
            View::Central { page, .. } => {
                if let (Some(t), Some(r)) = (track, page.rect_of("NextTrackPic")) {
                    let key = format!("{}#{}", t.path.display(), t.preview);
                    // The track's picture fills the frame, its middle
                    // kept where the shapes differ.
                    if let Some(tex) = ui.picture(gfx, &key, || preview_image(&t)) {
                        let (w, h) = gfx.sprites.size(tex);
                        let (sx, sy) = (r[2] / w as f32, r[3] / h as f32);
                        let s = sx.max(sy);
                        let (u, v) = (r[2] / (w as f32 * s), r[3] / (h as f32 * s));
                        let uv = [
                            (1.0 - u) / 2.0,
                            (1.0 - v) / 2.0,
                            (1.0 + u) / 2.0,
                            (1.0 + v) / 2.0,
                        ];
                        gfx.sprites.quad(tex, uv, r, [1.0; 4]);
                    }
                }
            }
            _ => {}
        }
    }
}
