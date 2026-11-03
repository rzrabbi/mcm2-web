//! Race tapes in the race: recording one as the game does while a race
//! runs (`VCRtape.dat`, `0x49d4c0`; each bike's packets `0x493620` and
//! `0x493a80`), and playing one back (`0x494610`) with the game's VCR
//! player, `UI/VCR.DAT` (its buttons `0x489c20`, its display `0x48a350`).

use crate::race::{Bike, Race, Racer};
use crate::ui::{Dlg, Event, Gfx, Ui};
use glam::{Mat3, Mat4, Vec3};
use mm2_formats::trn::Terrain;
use mm2_formats::vcr::{self, State, Tape, Timeline};
use mm2_physics::bike::COM;
use std::path::{Path, PathBuf};

/// A bike played from a tape: where its centre of mass is and how it is
/// turned, as the tape says.
#[derive(Debug, Clone)]
pub struct Taped {
    pub position: Vec3,
    pub rotation: Mat3,
    pub velocity: Vec3,
    pub crashed: bool,
    pub airborne: bool,
    /// The trick under way (the packet's first flag byte, less one).
    pub trick: Option<usize>,
    /// Taunting (trick byte 0x11): the replay looks round as the rider did.
    pub taunting: bool,
    /// Not drawn (a ghost with no lap to show yet).
    pub hidden: bool,
}

impl Taped {
    pub fn place(&self) -> Mat4 {
        let mut m = if self.hidden {
            Mat4::ZERO
        } else {
            Mat4::from_mat3(self.rotation)
        };
        m.w_axis = (self.position - self.rotation * COM).extend(1.0);
        m
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation.z_axis
    }

    pub fn from_state(s: &State, terrain: &Terrain) -> Taped {
        let position = Vec3::from(s.position);
        let ground = crate::ride::ground(terrain, position.x, position.z);
        let trick = s.flags[0] & 0x3f;
        Taped {
            position,
            rotation: rotation(s.angles),
            velocity: Vec3::from(s.velocity),
            crashed: s.flags[1] & 8 != 0,
            // The tape has no flag for it; well above the ground is in the
            // air.
            airborne: position.y - ground > COM.y + 1.5,
            trick: match trick {
                1..=16 => Some(trick as usize - 1),
                // The Alts (our numbering of them).
                0x12..=0x21 => Some(trick as usize - 0x12 + 16),
                _ => None,
            },
            taunting: trick == 0x11,
            hidden: false,
        }
    }
}

/// The turn a tape's roll, pitch and heading make (`0x4ba440`): from
/// looking along +z with +y up, roll about z, then pitch about x, then
/// heading about y.
pub fn rotation([roll, pitch, heading]: [f32; 3]) -> Mat3 {
    let (sr, cr) = roll.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sh, ch) = heading.sin_cos();
    let look = Vec3::new(sh * cp, sp, ch * cp);
    let up = Vec3::new(ch * sr - sh * sp * cr, cr * cp, -sh * sr - ch * sp * cr);
    let right = up.cross(look);
    Mat3::from_cols(right, up, look)
}

/// The roll, pitch and heading of a turn, the other way.
pub fn angles(m: Mat3) -> [f32; 3] {
    let (look, up) = (m.z_axis, m.y_axis);
    let heading = look.x.atan2(look.z);
    let pitch = look.y.clamp(-1.0, 1.0).asin();
    let (sh, ch) = heading.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let level_right = Vec3::new(ch, 0.0, -sh);
    let pitched_up = Vec3::new(-sh * sp, cp, -ch * sp);
    let roll = up.dot(level_right).atan2(up.dot(pitched_up));
    [roll, pitch, heading]
}

/// A racer as a tape keeps it, at `clock` milliseconds.
pub fn state_of(r: &Racer, clock: u32, place: u8, index: u8, race_clock: f32) -> State {
    let m = r.bike.place();
    let rot = Mat3::from_mat4(m);
    let com = m.transform_point3(COM);
    let trick = match r.stunt.trick {
        Some(t) if t < 16 => t as u8 + 1,
        Some(t) => (t - 16) as u8 + 0x12,
        None if r.taunt.is_some() => 0x11,
        None => 0,
    };
    State {
        place,
        position: com.into(),
        angles: angles(rot),
        lap_time: r.lap_time(race_clock),
        best_lap: r.best_lap().unwrap_or(0.0),
        total: r.finished.unwrap_or(0.0),
        velocity: r.bike.velocity().into(),
        clock,
        flags: [
            trick,
            ((r.bike.crashed() as u8) << 3) | ((r.finished.is_some() as u8) << 5),
            0,
            place,
        ],
        index,
        ..State::default()
    }
}

/// A bike that stands where `b` is, for a tape or another machine to
/// move.
pub fn taped_like(b: &Bike) -> Bike {
    let place = b.place();
    Bike::Taped(Box::new(Taped {
        position: place.transform_point3(COM),
        rotation: Mat3::from_mat4(place),
        velocity: Vec3::ZERO,
        crashed: false,
        airborne: false,
        trick: None,
        taunting: false,
        hidden: false,
    }))
}

/// How far behind another machine's bikes are shown, so there is a packet
/// either side to slide between, and how often ours go out.
pub const NET_DELAY: f32 = 0.1;
pub const NET_SEND: f32 = 1.0 / 20.0;

/// Another machine's bike: its last packets, by its race clock.
#[derive(Debug, Default)]
pub struct Remote {
    states: std::collections::VecDeque<(f32, State)>,
}

impl Remote {
    pub fn push(&mut self, time: f32, s: State) {
        if self.states.back().is_some_and(|b| b.0 >= time) {
            return;
        }
        self.states.push_back((time, s));
        while self.states.len() > 40 {
            self.states.pop_front();
        }
    }

    /// The bike at `time`: between the packets either side, or the last.
    pub fn at(&self, time: f32) -> Option<State> {
        let i = self.states.partition_point(|s| s.0 <= time);
        match (
            i.checked_sub(1).and_then(|k| self.states.get(k)),
            self.states.get(i),
        ) {
            (Some(a), Some(b)) => {
                let f = ((time - a.0) / (b.0 - a.0).max(1e-3)).clamp(0.0, 1.0);
                Some(a.1.lerp(&b.1, f))
            }
            (a, b) => a.or(b).map(|s| s.1),
        }
    }
}

/// How often a frame goes on the tape: the game writes one each frame it
/// draws; ours keep to a steady 20 a second.
const FRAME: f32 = 1.0 / 20.0;
/// The longest recording kept (the game's `VCRRecordTimeLimit`), in
/// seconds; ours.
const LIMIT: f32 = 30.0 * 60.0;

/// What the menus knew of a race, for the tape's header.
pub struct Heading {
    pub track: String,
    pub course: usize,
    pub event: u32,
    pub mode: u32,
    pub laps: u32,
    pub riders: Vec<vcr::Rider>,
}

/// A race going on tape.
pub struct Recording {
    rec: vcr::Recorder,
    wait: f32,
    /// Each racer's stunt points so far, to put each jump's score on the
    /// tape (type 10) as it lands.
    points: Vec<f32>,
}

/// A tape with the race's header and no records yet.
pub fn header(h: Heading) -> Tape {
    let mut tape = Tape::new(Vec::new(), h.riders);
    tape.set_track(&h.track);
    tape.set_setting(vcr::setting::EVENT, h.event);
    tape.set_setting(vcr::setting::MODE, h.mode);
    tape.set_setting(vcr::setting::LAPS, h.laps);
    tape.set_setting(
        vcr::setting::OPPONENTS,
        tape.riders.len().saturating_sub(1) as u32,
    );
    tape.set_course(h.course);
    tape.description = date();
    tape
}

impl Recording {
    pub fn new(h: Heading) -> Recording {
        Recording {
            rec: vcr::Recorder::new(header(h)),
            wait: 0.0,
            points: Vec::new(),
        }
    }

    /// After each update of the race.
    pub fn tick(&mut self, race: &Race, dt: f32) {
        self.points.resize(race.racers.len(), 0.0);
        for (i, r) in race.racers.iter().enumerate() {
            if r.stunt_points > self.points[i] {
                let score = r.stunt_points - self.points[i];
                self.rec.stunt(i, score as u32);
                self.points[i] = r.stunt_points;
            }
        }
        self.wait -= dt;
        if self.wait > 0.0 || race.time > LIMIT {
            return;
        }
        self.wait += FRAME;
        if self.wait < 0.0 {
            self.wait = FRAME;
        }
        let clock = (race.time * 1000.0) as u32;
        let order = race.standings();
        let bikes: Vec<State> = race
            .racers
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let place = order.iter().position(|&k| k == i).unwrap_or(0) as u8 + 1;
                state_of(r, clock, place, i as u8, race.clock())
            })
            .collect();
        self.rec.frame(race.time, FRAME, &bikes);
    }

    pub fn finish(self) -> Tape {
        self.rec.finish()
    }
}

/// The date and time, as the game names a tape until it is described
/// ("4/9/2000 12:19 AM"). In UTC; the port has no time zone to hand.
fn date() -> String {
    let secs = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;
    let (h, m) = (rest / 3600, rest / 60 % 60);
    let (h12, ampm) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    format!("{month}/{day}/{year} {h12}:{m:02} {ampm}")
}

/// The longest ghost lap kept (the game's `VCRGhostTimeLimit`, 300 s by
/// default).
const GHOST_LIMIT: f32 = 300.0;

/// A ghost race (race mode 4): each of the player's laps goes on tape
/// (`VCRghost.dat`), and the best so far, or the saved ghost chosen, rides
/// along as the ghost from the start of each lap (`0x421b10` swaps in a
/// lap that beats it).
pub struct Ghost {
    /// The racer that shows the ghost.
    pub racer: usize,
    best: Option<(Tape, Timeline)>,
    /// A lap the player rode beat the ghost they started with.
    pub improved: bool,
    header: Tape,
    rec: Option<vcr::Recorder>,
    lap_start: f32,
    laps: u32,
    wait: f32,
}

impl Ghost {
    /// `header` is the race's tape header with the player alone on it;
    /// the first lap starts at `lead`, when the gate drops.
    pub fn new(racer: usize, header: Tape, saved: Option<Tape>, lead: f32) -> Ghost {
        Ghost {
            racer,
            best: saved.map(|t| {
                let line = t.timeline();
                (t, line)
            }),
            improved: false,
            header,
            rec: None,
            lap_start: lead,
            laps: 0,
            wait: 0.0,
        }
    }

    /// The best lap's tape, its length the lap time.
    pub fn best(&self) -> Option<&Tape> {
        self.best.as_ref().map(|b| &b.0)
    }

    /// After each update of the race: tapes the player's lap, takes a
    /// finished lap that beats the ghost, and moves the ghost along.
    pub fn tick(&mut self, race: &mut Race, terrain: &Terrain, dt: f32) {
        let me = race.player;
        let Some(player) = race.racers.get(me) else {
            return;
        };
        if race.gate_down() {
            if player.laps > self.laps {
                // A lap done.
                self.laps = player.laps;
                let lap = race.time - self.lap_start;
                if let Some(rec) = self.rec.take() {
                    let mut tape = rec.finish();
                    tape.length = lap;
                    tape.description = date();
                    let better = self.best.as_ref().is_none_or(|b| lap <= b.0.length);
                    if better {
                        let line = tape.timeline();
                        self.best = Some((tape, line));
                        self.improved = true;
                    }
                }
                self.lap_start = race.time;
            }
            let into = race.time - self.lap_start;
            if self.rec.is_none() && into < 0.5 {
                self.rec = Some(vcr::Recorder::new(self.header.clone()));
                self.wait = 0.0;
            }
            if into > GHOST_LIMIT {
                self.rec = None;
            }
            self.wait -= dt;
            if let (Some(rec), true) = (&mut self.rec, self.wait <= 0.0) {
                self.wait = FRAME;
                let p = &race.racers[me];
                let s = state_of(p, (into * 1000.0) as u32, 1, 0, race.clock());
                rec.frame(into, FRAME, &[s]);
            }
        }
        // The ghost rides its lap from the start of each of the player's.
        let into = race.time - self.lap_start;
        let shown = race
            .gate_down()
            .then_some(())
            .and(self.best.as_ref())
            .and_then(|(_, line)| {
                let start = line.frames.first()?.time;
                (into <= line.length() - start).then(|| line.sample(0, start + into))?
            });
        if let Some(r) = race.racers.get_mut(self.racer) {
            let mut t = match shown {
                Some(s) => Taped::from_state(&s, terrain),
                None => match &r.bike {
                    Bike::Taped(t) => (**t).clone(),
                    b => Taped {
                        position: b.place().transform_point3(COM),
                        rotation: Mat3::from_mat4(b.place()),
                        velocity: Vec3::ZERO,
                        crashed: false,
                        airborne: false,
                        trick: None,
                        taunting: false,
                        hidden: true,
                    },
                },
            };
            t.hidden = shown.is_none();
            if t.trick != r.stunt.trick {
                r.trick_clock = 0.0;
            } else {
                r.trick_clock += dt;
            }
            r.stunt.trick = t.trick;
            r.bike = Bike::Taped(Box::new(t));
        }
    }
}

/// The player's speeds (`0x489c20`, the game's `+0x2e0`): play, the two
/// fast forwards, and to the end, which runs at 30 times.
const PLAY: f32 = 1.0;
const FF: f32 = 2.0;
const FFF: f32 = 8.0;
const TO_END: f32 = 30.0;

/// Where the player's panel sits: across the bottom, centred (our pick).
const PANEL: (f32, f32) = (117.0, 408.0);

/// A tape playing.
pub struct Replay {
    pub tape: Tape,
    pub line: Timeline,
    pub path: PathBuf,
    /// Seconds along the tape.
    pub time: f32,
    /// How fast it runs; negative runs it backwards.
    pub rate: f32,
    pub paused: bool,
    /// The rider the camera follows.
    pub follow: usize,
    panel: Option<Dlg>,
}

/// What the player's panel asks.
pub enum Ask {
    Stay,
    /// The camera button: the race camera's next view, as in the race
    /// (0x489c20 calls the camera's +0x120, 0x531660).
    Camera,
    Exit,
    Save,
}

impl Replay {
    pub fn open(path: &Path) -> Option<Replay> {
        let bytes = crate::platform::fs::read(path)
            .map_err(|e| eprintln!("{}: {e}", path.display()))
            .ok()?;
        let tape = Tape::parse(&bytes)
            .map_err(|e| eprintln!("{}: {e:?}", path.display()))
            .ok()?;
        let line = tape.timeline();
        let time = line.frames.first().map_or(0.0, |f| f.time);
        Some(Replay {
            tape,
            line,
            path: path.to_path_buf(),
            time,
            rate: PLAY,
            paused: false,
            follow: 0,
            panel: None,
        })
    }

    pub fn start(&self) -> f32 {
        self.line.frames.first().map_or(0.0, |f| f.time)
    }

    pub fn end(&self) -> f32 {
        self.line.length()
    }

    /// Moves the tape on by `dt` seconds of real time.
    pub fn advance(&mut self, dt: f32) {
        if !self.paused {
            self.time += dt * self.rate;
        }
        let (start, end) = (self.start(), self.end());
        if self.time >= end && self.rate > 0.0 {
            self.time = end;
            self.paused = true;
            self.rate = PLAY;
        }
        if self.time <= start && self.rate < 0.0 {
            self.time = start;
            self.paused = true;
            self.rate = PLAY;
        }
        self.time = self.time.clamp(start, end.max(start));
    }

    /// Bike `i` as of now.
    pub fn bike(&self, i: usize, terrain: &Terrain) -> Option<Bike> {
        let s = self.line.sample(i, self.time)?;
        Some(Bike::Taped(Box::new(Taped::from_state(&s, terrain))))
    }

    /// The race's state as of now.
    pub fn show(&self, race: &mut Race, terrain: &Terrain) -> (Vec3, Vec3) {
        race.player = self.follow.min(race.racers.len().saturating_sub(1));
        race.replay(self.time, |i| self.bike(i, terrain), terrain)
    }

    fn panel(&mut self, ui: &mut Ui) -> Option<&mut Dlg> {
        if self.panel.is_none() {
            self.panel = Dlg::open(ui, "VCR", PANEL);
        }
        self.panel.as_mut()
    }

    /// The panel's times, slider and toggles (`0x48a350`, `0x48a450`).
    pub fn draw(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32) {
        let (time, start, end) = (self.time, self.start(), self.end());
        let (paused, rate) = (self.paused, self.rate);
        let Some(p) = self.panel(ui) else {
            return;
        };
        p.set_text("TxtTime", clock_text(time));
        p.set_text("TxtTimeEnd", clock_text(end));
        let span = ((end - start) * 10.0).max(1.0) as u32;
        if !p.is_dragging("SliderTime") {
            p.set_slider("SliderTime", ((time - start) * 10.0) as u32, Some(span));
        }
        p.set_on("TogPause", paused);
        p.set_on("TogPlay", !paused && rate == PLAY);
        p.set_on("TogFF", !paused && rate == FF);
        p.set_on("TogFFF", !paused && rate >= FFF);
        p.set_on("TogReverse", !paused && rate < 0.0);
        p.draw(ui, gfx, now);
    }

    pub fn mouse_down(&mut self, ui: &mut Ui, x: f32, y: f32) {
        if let Some(p) = self.panel(ui) {
            p.mouse_down(x, y);
        }
    }

    pub fn mouse_move(&mut self, ui: &mut Ui, x: f32, y: f32) {
        let events = match self.panel(ui) {
            Some(p) => p.mouse_move(x, y),
            None => return,
        };
        for e in events {
            self.event(e);
        }
    }

    pub fn mouse_up(&mut self, ui: &mut Ui, x: f32, y: f32) -> Ask {
        let events = match self.panel(ui) {
            Some(p) => p.mouse_up(x, y),
            None => return Ask::Stay,
        };
        let mut ask = Ask::Stay;
        for e in events {
            match self.event(e) {
                Ask::Stay => {}
                a => ask = a,
            }
        }
        ask
    }

    /// The panel's buttons (`0x489c20`).
    fn event(&mut self, e: Event) -> Ask {
        let name = match e {
            Event::Clicked(n) => n.to_ascii_lowercase(),
            Event::Changed(n) if n.eq_ignore_ascii_case("SliderTime") => {
                // Dragging the slider moves the tape there.
                let span = ((self.end() - self.start()) * 10.0).max(1.0) as u32;
                if let Some(p) = &self.panel {
                    self.time = self.start() + p.slider_at("SliderTime", span) as f32 / 10.0;
                }
                return Ask::Stay;
            }
            _ => return Ask::Stay,
        };
        let riders = self.tape.riders.len().max(1);
        let play = |r: &mut Replay, rate: f32| {
            r.rate = rate;
            r.paused = false;
        };
        match name.as_str() {
            "togplay" => play(self, PLAY),
            "togpause" => self.paused = !self.paused,
            "togff" => play(self, FF),
            "togfff" => play(self, FFF),
            "togreverse" => play(self, -PLAY),
            "butbeginning" => {
                self.time = self.start();
                self.paused = true;
            }
            "butend" => play(self, TO_END),
            "butframe" => {
                // One frame on, then held.
                let i = self.line.frames.partition_point(|f| f.time <= self.time);
                if let Some(f) = self.line.frames.get(i) {
                    self.time = f.time;
                }
                self.paused = true;
            }
            "butcamera" => return Ask::Camera,
            "butprevrider" => self.follow = (self.follow + riders - 1) % riders,
            "butnextrider" => self.follow = (self.follow + 1) % riders,
            "butsave" => {
                // Kept: the button greys out, so it shows it was done.
                if let Some(p) = &mut self.panel {
                    p.enable("ButSave", false);
                }
                return Ask::Save;
            }
            "butexit" => return Ask::Exit,
            _ => {}
        }
        Ask::Stay
    }
}

/// `%02d:%02d:%05.2f` (`0x489b60`).
fn clock_text(t: f32) -> String {
    let t = t.max(0.0);
    let h = (t / 3600.0) as u32;
    let m = ((t - h as f32 * 3600.0) / 60.0) as u32;
    let s = t - h as f32 * 3600.0 - m as f32 * 60.0;
    format!("{h:02}:{m:02}:{s:05.2}")
}

/// Where a race's tape goes while it records, and saved replays.
pub fn tape_path(user_dir: &Path) -> PathBuf {
    user_dir.join("VCRtape.dat")
}

pub fn record_dir(user_dir: &Path) -> PathBuf {
    user_dir.join("Record")
}

/// Keeps the race just run as a replay (`0x421a40` copies the tape to
/// `Record\<name>.vcr`). The game names it track, rider, day and minute
/// (`Week15-Milwaukee_Mike_20261024_1502.vcr`), the minute the race was
/// recorded, so saving the same race again keeps one file.
pub fn save(tape: &Path, user_dir: &Path, track: &str) -> std::io::Result<PathBuf> {
    let dir = record_dir(user_dir);
    crate::platform::fs::create_dir_all(&dir)?;
    let path = match Tape::parse(&crate::platform::fs::read(tape)?).ok().and_then(|t| file_name(&t, track)) {
        Some(name) => dir.join(name),
        None => free_name(user_dir, "Replay", "vcr")?,
    };
    // A saved replay played again is already kept.
    let same = |a: &Path, b: &Path| crate::platform::fs::canonicalize(a).ok().zip(crate::platform::fs::canonicalize(b).ok()).is_some_and(|(a, b)| a == b);
    if !same(tape, &path) {
        crate::platform::fs::copy(tape, &path)?;
    }
    Ok(path)
}

/// `TRACK_RIDER_YYYYMMDD_HHMM.vcr` from the tape's first rider and the
/// date in its description ("10/28/2026 11:58 AM"), with only letters,
/// digits and `-` kept from the names.
fn file_name(tape: &Tape, track: &str) -> Option<String> {
    let keep = |s: &str| -> String { s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect() };
    let (date, time) = tape.description.trim().split_once(' ')?;
    let mut d = date.split('/').map(|n| n.parse::<u32>().ok());
    let (month, day, year) = (d.next()??, d.next()??, d.next()??);
    let (clock, half) = time.trim().split_once(' ').unwrap_or((time.trim(), ""));
    let (h, m) = clock.split_once(':')?;
    let (mut h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    match half.to_ascii_lowercase().as_str() {
        "pm" if h < 12 => h += 12,
        "am" if h == 12 => h = 0,
        _ => {}
    }
    let rider = tape.riders.first().map(|r| keep(&r.name)).unwrap_or_default();
    Some(format!("{}_{}_{year:04}{month:02}{day:02}_{h:02}{m:02}.vcr", keep(track), rider))
}

/// Keeps a ghost race's best lap as the next free "Ghost N.gho".
pub fn save_ghost(tape: &Tape, user_dir: &Path) -> std::io::Result<PathBuf> {
    let bytes = tape.write();
    crate::platform::fs::write(user_dir.join("VCRghost.dat"), &bytes)?;
    let path = free_name(user_dir, "Ghost", "gho")?;
    crate::platform::fs::write(&path, bytes)?;
    Ok(path)
}

fn free_name(user_dir: &Path, stem: &str, ext: &str) -> std::io::Result<PathBuf> {
    let dir = record_dir(user_dir);
    crate::platform::fs::create_dir_all(&dir)?;
    let mut n = 1;
    loop {
        let p = dir.join(format!("{stem} {n}.{ext}"));
        if !crate::platform::fs::exists(&p) {
            return Ok(p);
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_replay_is_named_as_the_game_names_it() {
        let mut t = Tape::new(vec![0; vcr::SETTINGS_LEN], Vec::new());
        t.riders.push(vcr::Rider { name: "Mike".into(), ..Default::default() });
        t.description = "10/28/2026 11:58 AM".into();
        assert_eq!(file_name(&t, "Morenci Mine").as_deref(), Some("MorenciMine_Mike_20261028_1158.vcr"));
        t.description = "10/24/2026 3:02 PM".into();
        assert_eq!(file_name(&t, "Week15-Milwaukee").as_deref(), Some("Week15-Milwaukee_Mike_20261024_1502.vcr"));
        t.description = "my best run".into();
        assert_eq!(file_name(&t, "Morenci Mine"), None);
    }

    #[test]
    fn angles_go_both_ways() {
        for a in [
            [0.0, 0.0, 0.0],
            [0.3, -0.2, 1.1],
            [-0.5, 0.4, -2.9],
            [0.1, 1.2, 3.0],
        ] {
            let back = angles(rotation(a));
            for k in 0..3 {
                assert!((back[k] - a[k]).abs() < 1e-4, "{a:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn the_tape_turns_as_the_physics_does() {
        // The physics: heading about y, then the nose up by pitch.
        let (h, p) = (0.7f32, 0.25f32);
        let m = Mat3::from_rotation_y(h) * Mat3::from_rotation_x(-p);
        let a = angles(m);
        assert!((a[2] - h).abs() < 1e-5 && (a[1] - p).abs() < 1e-5 && a[0].abs() < 1e-5);
        let r = rotation(a);
        assert!((r.z_axis - m.z_axis).length() < 1e-5);
        assert!((r.y_axis - m.y_axis).length() < 1e-5);
    }

    #[test]
    fn clock_text_is_the_games() {
        assert_eq!(clock_text(14.132), "00:00:14.13");
        assert_eq!(clock_text(3725.5), "01:02:05.50");
    }
}
