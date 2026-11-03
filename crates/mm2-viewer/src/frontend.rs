//! The game's front end as the original lays it out: the main menu with its
//! pull-downs, the single player screen (event, bike and race tabs), and
//! the loading screen while a track loads.
//!
//! The layouts, art and sounds are the game's own (`UI/*.DAT`); what the
//! buttons do follows `Mcm2.exe` (main menu `0x44c1d0`, single player tabs
//! `0x44e600`, event page `0x44e830`, `0x44ee90` and `0x44f2c0`).

use crate::audio::Audio;
use crate::movie::Movie;
use crate::roster::{Ride, Roster};
use crate::screens::{self, profiles, Cx, Go};
use crate::ui::{Dlg, Event, Gfx, Ui};
use mm2_formats::{rs2, scn, tga};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The kinds of event, numbered as the game numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    Stunt = 0,
    Baja = 1,
    Nationals = 2,
    Supercross = 3,
    /// Moto-Tag, multiplayer only.
    Tag = 4,
    Enduro = 5,
}

impl EventKind {
    /// The order of the event page's drop-down, with each line's text in
    /// `LANG.DLL`.
    const LIST: [(EventKind, u32); 5] = [
        (EventKind::Baja, 5101),
        (EventKind::Stunt, 5102),
        (EventKind::Enduro, 5104),
        (EventKind::Nationals, 5103),
        (EventKind::Supercross, 5106),
    ];

    /// The event the game numbers `n`.
    pub fn from_number(n: u32) -> Option<EventKind> {
        Some(match n {
            0 => EventKind::Stunt,
            1 => EventKind::Baja,
            2 => EventKind::Nationals,
            3 => EventKind::Supercross,
            4 => EventKind::Tag,
            5 => EventKind::Enduro,
            _ => return None,
        })
    }

    /// Where the event's tracks are, under the game folder.
    pub fn folder(self) -> &'static str {
        match self {
            EventKind::Stunt => "TERAFORM/QUARRIES",
            EventKind::Baja => "TERAFORM/BAJA",
            EventKind::Nationals => "TERAFORM/NATIONAL",
            EventKind::Supercross => "TERAFORM/SX",
            EventKind::Enduro => "TERAFORM/ENDURO",
            EventKind::Tag => "TERAFORM/TAG",
        }
    }

    /// The loading screen's dialog.
    pub fn loading(self) -> &'static str {
        match self {
            EventKind::Stunt => "LOADQUAR",
            EventKind::Baja => "LOADBAJA",
            EventKind::Nationals => "LOADNAT",
            EventKind::Supercross => "LOADSX",
            EventKind::Enduro => "LOADEND",
            EventKind::Tag => "LOADTAG",
        }
    }

    /// Baja and enduro tracks hold several courses, each its own race.
    pub fn courses(self) -> bool {
        matches!(self, EventKind::Baja | EventKind::Enduro)
    }

    /// Laps start at 2 for the stadium and outdoor races.
    fn fewest_laps(self) -> u32 {
        match self {
            EventKind::Nationals | EventKind::Supercross => 2,
            _ => 1,
        }
    }
}

/// The race modes, as the game numbers them, with their text.
const MODES: [(u32, u32); 4] = [(0, 5107), (1, 5108), (2, 5109), (4, 5110)];
pub const GHOST: u32 = 4;
pub const PRACTICE: u32 = 0;

/// What the player chose to race.
#[derive(Debug, Clone)]
pub struct Setup {
    pub event: EventKind,
    pub track: PathBuf,
    /// Which course of the track (Baja and enduro have several).
    pub course: usize,
    /// Laps, or minutes for the stunt event.
    pub laps: u32,
    pub opponents: u32,
    pub mode: u32,
    /// The track's name as the list shows it.
    pub name: String,
    /// The player's bike, outfit and number.
    pub ride: Option<Ride>,
    /// The computer riders' level, 0 to 2.
    pub level: usize,
    /// Each racer's name, the player's first; any left out take the
    /// game's list.
    pub names: Vec<String>,
    /// The computer riders' rides by racer (the player's is `ride`); any
    /// left out take the default grid.
    pub rides: Vec<Option<Ride>>,
    /// A ghost race's saved ghost to race (`Record\*.gho`); none records
    /// a new one.
    pub ghost: Option<PathBuf>,
    /// A network race: which racer is this player's, and which are other
    /// players' (the rest are the host's computer riders).
    pub net: Option<NetRace>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetRace {
    pub me: usize,
    pub humans: usize,
    pub host: bool,
    /// Moto-Tag with the ball, and in stunt mode.
    pub tag_ball: bool,
    pub tag_stunts: bool,
}

/// One rider's line of the Race Info page: name, place, best lap and
/// total time.
pub type ResultLine = [String; 4];

/// The last race's lines for the Race Info page, and whether it was the
/// Stunt Quarry, which shows names and scores only (`0x4521b0`).
#[derive(Debug, Clone, Default)]
pub struct RaceResults {
    pub lines: Vec<ResultLine>,
    pub stunt: bool,
    /// Each racer's place and what the race ran up, by racer (the player
    /// first), for the Pro Circuit's money (`0x45d670`).
    pub placings: Vec<Placing>,
    /// The race's tape, for the View Replay button.
    pub replay: Option<PathBuf>,
}

/// One racer's end of a race.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Placing {
    /// From 1.
    pub place: usize,
    /// Repair, medical and stunt money units (bike +0x7ac, +0x7b0, +0x7b4).
    pub repair: f32,
    pub medical: f32,
    pub stunt: f32,
}

/// One line of the track list.
#[derive(Debug, Clone)]
pub struct TrackEntry {
    pub path: PathBuf,
    pub course: usize,
    pub name: String,
    /// The preview picture's name inside the track's archive.
    pub preview: String,
}

/// What the app should do after an input.
#[allow(clippy::large_enum_variant)]
pub enum Next {
    Stay,
    Quit,
    Race(Setup),
    /// A tape to play.
    Replay(PathBuf),
}

// There is only ever one screen, so its size does not matter.
#[allow(clippy::large_enum_variant)]
enum Screen {
    Main(Dlg),
    Single {
        base: Dlg,
        page: Dlg,
    },
    Loading(Dlg),
    /// One of the screens in `screens`: options, high scores, credits.
    Extra(Box<dyn screens::Screen>),
}

pub struct Frontend {
    ui: Ui,
    screen: Screen,
    event: EventKind,
    mode: u32,
    laps: u32,
    opponents: u32,
    track: usize,
    tracks: HashMap<EventKind, Vec<TrackEntry>>,
    /// An event whose track files are still being fetched (browser only).
    awaiting: Option<EventKind>,
    /// A pull-down waiting for the one that is closing to be out of the way.
    pending_pull: Option<String>,
    /// The pull-downs' pictures still to load, a few a frame, so that the
    /// first one to open does not stall on loading them.
    warm: Vec<(String, String)>,
    warmed: bool,
    /// The last race's results, for the Race Info page.
    results: RaceResults,
    /// The bikes and outfits on offer, and the player's picks.
    roster: Roster,
    bike: usize,
    /// The player has picked a bike on the Bike & Rider page.
    bike_picked: bool,
    outfit: usize,
    plate: String,
    /// The plate number box has the keyboard.
    typing: bool,
    /// The player's profile and the boxes the main menu's profile
    /// buttons open.
    user: screens::User,
    boxes: profiles::Profiles,
    /// After a ghost race whose best lap beat the ghost: "Do you want to
    /// save the current ghost lap?" over the Race Info page, and the lap.
    ghost_ask: Option<(Dlg, mm2_formats::vcr::Tape)>,
    /// The screen a race was started from (the Pro Circuit), to go back
    /// to with its results.
    parked: Option<Box<dyn screens::Screen>>,
    /// The single player screen while its Options or Joystick button's
    /// options are up, to go back to.
    beneath: Option<Screen>,
    /// The main menu's little screen of event clips (`UI/EVENTS.AVI`),
    /// and how long the menu has waited with no pull-down open.
    events: Option<Movie>,
    events_wait: f32,
    now: f32,
    /// The ghost race list's files (after its first line, "Record a new
    /// ghost race"), and the line picked.
    ghosts: Vec<PathBuf>,
    ghost: usize,
    /// What a screen asked between inputs (a network race starting).
    pending: Option<Next>,
}

/// The Bike & Rider page's engine sizes (`0x44fe50`), for the bikes made
/// from the player's own paint (`Bikes/*.tga`); the makers' bikes come
/// with theirs and leave the list disabled.
const ENGINE_SIZES: [(u32, &str); 5] = [
    (125, "125cc 2-stroke"),
    (250, "250cc 2-stroke"),
    (400, "400cc 4-stroke"),
    (500, "500cc 2-stroke"),
    (600, "600cc 4-stroke"),
];

/// Where the single player screen's tab pages go (`0x44e600`).
const PAGE: (f32, f32) = (16.0, 41.0);

impl Frontend {
    pub fn new(game: &Path) -> Option<Frontend> {
        let mut ui = Ui::new(game);
        let main = Dlg::open(&mut ui, "MAINMENU", (0.0, 0.0))?;
        let user = screens::User::load(&ui);
        ui.volume = user.profile.sound.ui_gain();
        let mut f = Frontend {
            ui,
            screen: Screen::Main(main),
            event: EventKind::Supercross,
            mode: 1,
            laps: 3,
            opponents: 3,
            track: 0,
            tracks: HashMap::new(),
            awaiting: None,
            pending_pull: None,
            warm: Vec::new(),
            warmed: false,
            results: RaceResults::default(),
            roster: Roster::load(game),
            bike: 0,
            bike_picked: false,
            outfit: 0,
            plate: "107".into(),
            typing: false,
            user,
            boxes: profiles::Profiles::default(),
            ghost_ask: None,
            parked: None,
            beneath: None,
            events: None,
            events_wait: 0.0,
            now: 0.0,
            ghosts: Vec::new(),
            ghost: 0,
            pending: None,
        };
        f.fill_main();
        Some(f)
    }

    /// The main menu's profile list; the name box first if there is no
    /// profile yet (`0x44c1d0` open).
    fn fill_main(&mut self) {
        let Screen::Main(d) = &mut self.screen else {
            return;
        };
        let mut cx = Cx {
            ui: &mut self.ui,
            audio: None,
            user: &mut self.user,
            now: self.now,
        };
        profiles::fill(&mut cx, d);
        // The connections DirectPlay offered (`0x44c1d0`): the port has
        // its own TCP/IP play to come, and none of the others. In the
        // browser there is no play over the network yet (`COMING_SOON`).
        for (name, on) in [
            ("ButTCP", !COMING_SOON),
            ("ButIPX", false),
            ("ButZone", false),
            ("ButCable", false),
            ("ButModem", false),
        ] {
            d.enable(name, on);
        }
        self.boxes.start(&mut cx);
    }

    /// The logos before the main menu, unless the player's profile
    /// turned the intro off.
    pub fn intro(&mut self) {
        if !self.user.profile.game.intro_movie && !self.user.profile.name.is_empty() {
            return;
        }
        if let Some(i) = screens::intro::Intro::open(&mut self.ui) {
            self.screen = Screen::Extra(Box::new(i));
        }
    }

    /// The main menu's clips play on the screen of the Single Player
    /// pull-down: four seconds after it is all the way down they start,
    /// and go round until it goes up again (`0x44c1d0`, `0x44d6f0`,
    /// `0x44c190`).
    fn events_tick(&mut self, dt: f32) {
        let Screen::Main(d) = &self.screen else {
            self.events = None;
            self.events_wait = 0.0;
            self.pending_pull = None;
            return;
        };
        // A pull-down that waited for another to close opens now.
        if let Some(pull) = self.pending_pull.clone() {
            if PULL_DOWNS.iter().filter(|(_, p)| *p != pull).all(|(_, p)| d.is_shut(p)) {
                self.pending_pull = None;
                let animate = self.user.profile.game.ui_anims;
                let now = self.now;
                if let Screen::Main(d) = &mut self.screen {
                    let events = d.pull(&pull, true, now, animate);
                    if let Some(audio) = self.ui.audio.as_ref().map(|a| a.share()) {
                        self.sounds(&audio, &events);
                    }
                }
                return;
            }
        }
        let down = d.is_down("MenuSingleAnimation");
        let audio = self.ui.audio.as_ref();
        let events = self.events.get_or_insert_with(|| {
            Movie::open(&self.ui.game().join("UI/EVENTS.AVI"), true).unwrap_or_else(Movie::none)
        });
        if !down {
            events.stop();
            self.events_wait = 0.0;
        } else if events.playing() {
            events.tick(audio, self.now);
        } else {
            self.events_wait += dt;
            if self.events_wait >= 4.0 {
                events.play(audio, self.now);
            }
        }
    }

    /// The mixer, for the movies.
    pub fn set_audio(&mut self, audio: &Audio) {
        self.ui.audio = Some(audio.share());
    }

    /// The player's profile.
    pub fn user(&self) -> &screens::User {
        &self.user
    }

    pub fn user_mut(&mut self) -> &mut screens::User {
        &mut self.user
    }

    /// Back to the main menu (after a race), or to the screen the race
    /// was started from.
    pub fn main_menu(&mut self) {
        self.beneath = None;
        if self.unpark(None) {
            return;
        }
        if let Some(main) = Dlg::open(&mut self.ui, "MAINMENU", (0.0, 0.0)) {
            self.screen = Screen::Main(main);
            self.fill_main();
        }
    }

    fn dialogs(&mut self) -> Vec<&mut Dlg> {
        if let Some((d, _)) = &mut self.ghost_ask {
            return vec![d];
        }
        match &mut self.screen {
            Screen::Main(d) => match self.boxes.dialog() {
                Some(b) => vec![b],
                None => vec![d],
            },
            Screen::Loading(d) => vec![d],
            Screen::Single { base, page } => vec![base, page],
            Screen::Extra(s) => screens::inputs(s.as_mut()),
        }
    }

    pub fn mouse_move(&mut self, audio: &Audio, x: f32, y: f32) {
        let mut events = Vec::new();
        for d in self.dialogs() {
            events.extend(d.mouse_move(x, y));
        }
        self.sounds(audio, &events);
        // A slider being dragged.
        for e in events {
            if matches!(e, Event::Changed(_)) {
                self.extra(audio, e);
            }
        }
    }

    /// An event for the screen in `screens` that is up, or the main
    /// menu's profile boxes.
    fn extra(&mut self, audio: &Audio, e: Event) -> Next {
        let mut cx = Cx {
            ui: &mut self.ui,
            audio: Some(audio),
            user: &mut self.user,
            now: self.now,
        };
        match &mut self.screen {
            Screen::Extra(s) => {
                let go = s.event(&mut cx, e);
                self.go(go)
            }
            Screen::Main(d) => {
                self.boxes.event(&mut cx, d, e);
                Next::Stay
            }
            _ => Next::Stay,
        }
    }

    fn go(&mut self, go: Go) -> Next {
        match go {
            Go::Stay => Next::Stay,
            Go::Quit => Next::Quit,
            Go::Back => {
                match self.beneath.take() {
                    Some(s) => self.screen = s,
                    None => self.main_menu(),
                }
                Next::Stay
            }
            Go::Open(s) => {
                self.screen = Screen::Extra(s);
                Next::Stay
            }
            Go::Race(setup) => {
                if let Some(d) = Dlg::open(&mut self.ui, setup.event.loading(), (0.0, 0.0)) {
                    if let Screen::Extra(s) =
                        std::mem::replace(&mut self.screen, Screen::Loading(d))
                    {
                        self.parked = Some(s);
                    }
                }
                Next::Race(*setup)
            }
            Go::Replay(path) => Next::Replay(path),
            Go::Ghost(path) => match self.ghost_setup(&path) {
                Some(setup) => self.go(Go::Race(Box::new(setup))),
                None => Next::Stay,
            },
        }
    }

    /// A ghost race against the ghost in `path`: its track and course, the
    /// player on their bike.
    fn ghost_setup(&mut self, path: &Path) -> Option<Setup> {
        let tape = mm2_formats::vcr::Tape::parse(&crate::platform::fs::read(path).ok()?).ok()?;
        let event = EventKind::from_number(tape.setting(mm2_formats::vcr::setting::EVENT))?;
        let (stem, course) = (tape.track(), tape.course());
        let entry = self
            .tracks(event)
            .iter()
            .filter(|e| {
                e.path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case(&stem))
            })
            .find(|e| e.course == course || !event.courses())?
            .clone();
        Some(Setup {
            event,
            track: entry.path,
            course: entry.course,
            laps: 99,
            opponents: 0,
            mode: GHOST,
            name: entry.name,
            ride: self.ride(),
            level: 1,
            names: vec![self.user.profile.name.clone()],
            rides: Vec::new(),
            ghost: Some(path.to_path_buf()),
            net: None,
        })
    }

    /// What a screen asked since the last input.
    pub fn take_next(&mut self) -> Option<Next> {
        self.pending.take()
    }

    /// The loading screen of `event`'s tracks, while a tape's track loads.
    pub fn loading(&mut self, event: EventKind) {
        if let Some(d) = Dlg::open(&mut self.ui, event.loading(), (0.0, 0.0)) {
            if let Screen::Extra(s) = std::mem::replace(&mut self.screen, Screen::Loading(d)) {
                self.parked = Some(s);
            }
        }
    }

    /// Back from a replay: to the screen it was started from, else the
    /// Race Info page it was viewed from.
    pub fn replay_over(&mut self) {
        if self.unpark(None) {
            return;
        }
        self.open_single();
        self.show_tab("raceinfotab");
    }

    /// Back to the screen a race was started from, with its results (none
    /// if the race was left).
    fn unpark(&mut self, results: Option<&RaceResults>) -> bool {
        let Some(mut s) = self.parked.take() else {
            return false;
        };
        let mut cx = Cx {
            ui: &mut self.ui,
            audio: None,
            user: &mut self.user,
            now: self.now,
        };
        s.race_over(&mut cx, results);
        self.screen = Screen::Extra(s);
        true
    }

    /// A key pressed, for the controls page.
    pub fn key(&mut self, audio: &Audio, key: winit::keyboard::KeyCode) -> Next {
        let Some(code) = screens::keys::scan_code(key) else {
            return Next::Stay;
        };
        let Screen::Extra(s) = &mut self.screen else {
            return Next::Stay;
        };
        let mut cx = Cx {
            ui: &mut self.ui,
            audio: Some(audio),
            user: &mut self.user,
            now: self.now,
        };
        let go = s.key(&mut cx, code);
        self.go(go)
    }

    /// Whether the screen up waits for a joystick input to bind.
    #[cfg_attr(not(feature = "gamepad"), allow(dead_code))]
    pub fn wants_pad(&self) -> bool {
        matches!(&self.screen, Screen::Extra(s) if s.wants_pad())
    }

    /// A joystick button or axis, for the controls page.
    #[cfg_attr(not(feature = "gamepad"), allow(dead_code))]
    pub fn pad_input(&mut self, audio: &Audio, input: (i32, i32)) -> Next {
        let Screen::Extra(s) = &mut self.screen else {
            return Next::Stay;
        };
        let mut cx = Cx {
            ui: &mut self.ui,
            audio: Some(audio),
            user: &mut self.user,
            now: self.now,
        };
        let go = s.pad(&mut cx, input);
        self.go(go)
    }

    /// Whether the screen up is one of `screens`, or a box is open over
    /// the main menu.
    fn extra_up(&self) -> bool {
        matches!(self.screen, Screen::Extra(_))
            || matches!(self.screen, Screen::Main(_)) && self.boxes.is_open()
    }

    pub fn mouse_down(&mut self, x: f32, y: f32) {
        for d in self.dialogs() {
            d.mouse_down(x, y);
        }
    }

    pub fn wheel(&mut self, lines: i32) {
        for d in self.dialogs() {
            d.wheel(lines);
        }
    }

    pub fn mouse_up(&mut self, audio: &Audio, x: f32, y: f32, now: f32) -> Next {
        // An open drop-down list takes the click, wherever it lands.
        let mut events = Vec::new();
        let dialogs = self.dialogs();
        let with_list = dialogs.iter().position(|d| d.list_open());
        for (i, d) in dialogs.into_iter().enumerate() {
            if with_list.is_none_or(|l| l == i) {
                events.extend(d.mouse_up(x, y));
            }
        }
        self.sounds(audio, &events);
        if let Screen::Extra(s) = &mut self.screen {
            let mut cx = Cx {
                ui: &mut self.ui,
                audio: Some(audio),
                user: &mut self.user,
                now,
            };
            let go = s.pressed(&mut cx);
            if !matches!(go, Go::Stay) {
                return self.go(go);
            }
        }
        let mut next = Next::Stay;
        for e in events {
            if self.ghost_ask.is_some() {
                if let Event::Clicked(name) = &e {
                    self.ghost_answer(name);
                }
                continue;
            }
            if self.extra_up() {
                next = self.extra(audio, e);
                if !matches!(next, Next::Stay) {
                    break;
                }
                continue;
            }
            next = match e {
                Event::Clicked(name) => self.clicked(audio, &name, now),
                Event::Selected(name, line) => {
                    self.selected(&name, line);
                    Next::Stay
                }
                Event::Sound(_) | Event::Changed(_) | Event::Released(_) => Next::Stay,
            };
            if !matches!(next, Next::Stay) {
                break;
            }
        }
        next
    }

    /// Esc: closes a list or a pull-down, or goes back a screen.
    pub fn escape(&mut self, now: f32) -> Next {
        match &mut self.screen {
            Screen::Main(_) if self.boxes.is_open() => self.boxes.escape(),
            Screen::Extra(s) => {
                let mut cx = Cx {
                    ui: &mut self.ui,
                    audio: None,
                    user: &mut self.user,
                    now,
                };
                let go = s.escape(&mut cx);
                return self.go(go);
            }
            Screen::Main(d) => {
                let open: Vec<_> = PULL_DOWNS.iter().filter(|(_, p)| d.is_open(p)).collect();
                if open.is_empty() {
                    return Next::Quit;
                }
                let animate = self.user.profile.game.ui_anims;
                for (_, p) in open {
                    d.pull(p, false, now, animate);
                }
            }
            _ if self.ghost_ask.is_some() => self.ghost_ask = None,
            Screen::Single { base, page } => {
                if page.list_open() || base.list_open() {
                    page.close_list();
                    base.close_list();
                } else {
                    self.main_menu();
                }
            }
            Screen::Loading(_) => {}
        }
        Next::Stay
    }

    fn sounds(&mut self, audio: &Audio, events: &[Event]) {
        let pack = match &mut self.screen {
            Screen::Main(d) => match self.boxes.dialog() {
                Some(b) => b.pack.clone(),
                None => d.pack.clone(),
            },
            Screen::Loading(d) => d.pack.clone(),
            Screen::Single { base, .. } => base.pack.clone(),
            Screen::Extra(s) => s.pack(),
        };
        for e in events {
            if let Event::Sound(s) = e {
                self.ui.play(audio, &pack, s);
            }
        }
    }

    fn clicked(&mut self, audio: &Audio, name: &str, now: f32) -> Next {
        let name = name.to_ascii_lowercase();
        match &mut self.screen {
            Screen::Main(d) => {
                if let Some((_, pull)) = PULL_DOWNS.iter().find(|(b, _)| *b == name) {
                    // One pull-down at a time: the one that is out closes, and
                    // the new one opens when it has gone, not over it.
                    let open = !d.is_open(pull);
                    let animate = self.user.profile.game.ui_anims;
                    let mut events = Vec::new();
                    let mut closing = false;
                    for (_, other) in PULL_DOWNS.iter().filter(|(_, p)| p != pull) {
                        if !d.is_shut(other) {
                            events.extend(d.pull(other, false, now, animate));
                            closing |= animate;
                        }
                    }
                    if open && closing {
                        self.pending_pull = Some(pull.to_string());
                    } else {
                        self.pending_pull = None;
                        events.extend(d.pull(pull, open, now, animate));
                    }
                    self.sounds(audio, &events);
                    return Next::Stay;
                }
                let mut cx = Cx {
                    ui: &mut self.ui,
                    audio: Some(audio),
                    user: &mut self.user,
                    now,
                };
                if self.boxes.clicked(&mut cx, d, &name) {
                    return Next::Stay;
                }
                let event = match name.as_str() {
                    "menuexit" => return Next::Quit,
                    "menuhighscore" => {
                        if let Some(h) = screens::records::HighScores::open(&mut cx) {
                            self.screen = Screen::Extra(Box::new(h));
                        }
                        return Next::Stay;
                    }
                    "buttcp" => {
                        if let Some(m) = screens::multi::Multi::open(&mut cx) {
                            self.screen = Screen::Extra(Box::new(m));
                        }
                        return Next::Stay;
                    }
                    "menucredits" => {
                        if let Some(c) = screens::credits::Credits::open(&mut cx) {
                            self.screen = Screen::Extra(Box::new(c));
                        }
                        return Next::Stay;
                    }
                    "menureplay" => {
                        if let Some(r) = screens::replays::Replays::open(&mut cx) {
                            self.screen = Screen::Extra(Box::new(r));
                        }
                        return Next::Stay;
                    }
                    "menuoptions" => {
                        let o = screens::options::Options::open(&mut cx, 0);
                        if let Some(o) = o {
                            self.screen = Screen::Extra(Box::new(o));
                        }
                        return Next::Stay;
                    }
                    "butstuntquarry" => EventKind::Stunt,
                    "butbaja" => EventKind::Baja,
                    "butprocircuit" => {
                        if let Some(p) = screens::pro::Pro::open(&mut cx) {
                            self.screen = Screen::Extra(Box::new(p));
                        }
                        return Next::Stay;
                    }
                    "butnationals" => EventKind::Nationals,
                    "butsupercross" => EventKind::Supercross,
                    "butenduro" => EventKind::Enduro,
                    "butcancelprofile" => {
                        let events = d.pull("MenuUserAnimation", false, now, self.user.profile.game.ui_anims);
                        self.sounds(audio, &events);
                        return Next::Stay;
                    }
                    _ => return Next::Stay,
                };
                if self.event != event {
                    self.track = 0;
                }
                self.event = event;
                self.open_single();
            }
            Screen::Single { .. } => match name.as_str() {
                "back" => self.main_menu(),
                "start" => return self.start(),
                "butviewreplay" => {
                    if let Some(p) = self.results.replay.clone() {
                        return Next::Replay(p);
                    }
                }
                "eventtab" | "bikeridertab" | "raceinfotab" => self.show_tab(&name),
                // The options, at the controls page for Joystick, and back
                // here after.
                "options" | "joystick" => {
                    let at = if name == "joystick" { 3 } else { 0 };
                    let mut cx = Cx {
                        ui: &mut self.ui,
                        audio: Some(audio),
                        user: &mut self.user,
                        now,
                    };
                    if let Some(o) = screens::options::Options::open(&mut cx, at) {
                        let single = std::mem::replace(&mut self.screen, Screen::Extra(Box::new(o)));
                        self.beneath = Some(single);
                    }
                }
                _ => self.page_clicked(&name),
            },
            Screen::Loading(_) | Screen::Extra(_) => {}
        }
        Next::Stay
    }

    /// Back from a race: the single player screen, on the Race Info page
    /// with these results.
    /// Asks whether to keep a ghost race's new best lap (lang 5184, Yes
    /// and No).
    pub fn ask_ghost(&mut self, lap: mm2_formats::vcr::Tape) {
        let title = self.ui.lang(5320);
        let prompt = self.ui.lang(5184);
        if let Some(b) = screens::MsgBox::ask(&mut self.ui, &title, &prompt, [5182, 0, 5181]) {
            self.ghost_ask = Some((b.dlg, lap));
        }
    }

    fn ghost_answer(&mut self, button: &str) {
        let button = button.to_ascii_lowercase();
        if button != "butleft" && button != "butright" {
            return;
        }
        if let (Some((_, lap)), "butleft") = (self.ghost_ask.take(), button.as_str()) {
            match crate::replay::save_ghost(&lap, &self.user.store.dir) {
                Ok(p) => eprintln!("kept the ghost lap as {}", p.display()),
                Err(e) => eprintln!("keeping the ghost lap: {e}"),
            }
        }
    }

    pub fn race_over(&mut self, results: RaceResults) {
        if self.unpark(Some(&results)) {
            return;
        }
        self.results = results;
        self.open_single();
        self.show_tab("raceinfotab");
    }

    /// Switches the single player screen's page.
    fn show_tab(&mut self, name: &str) {
        let Screen::Single { base, .. } = &mut self.screen else {
            return;
        };
        for tab in ["EventTab", "BikeRiderTab", "RaceInfoTab"] {
            base.set_on(tab, tab.eq_ignore_ascii_case(name));
        }
        let pack = match name {
            "eventtab" => "SPEVENT",
            "bikeridertab" => "SPBIKER",
            _ => "SPRINFO",
        };
        if let Some(p) = Dlg::open(&mut self.ui, pack, PAGE) {
            if let Screen::Single { page, .. } = &mut self.screen {
                *page = p;
            }
            self.typing = false;
            match pack {
                "SPEVENT" => self.fill_event_page(),
                "SPRINFO" => self.fill_race_info(),
                _ => self.fill_bike_page(),
            }
        }
    }

    /// The game's folder.
    pub fn game(&self) -> Option<&Path> {
        Some(self.ui.game())
    }

    /// While the Bike & Rider page shows: the ride to turn in its window,
    /// and whether Auto Rotate is on.
    pub fn preview(&self) -> Option<(Ride, bool)> {
        match &self.screen {
            Screen::Single { page, .. } if page.index("DDLBikes").is_some() => {
                let turning = page.get("ChkAutoRotate").is_none_or(|c| c.on);
                Some((self.ride()?, turning))
            }
            Screen::Extra(s) => s.preview().map(|p| (p.0, p.1)),
            _ => None,
        }
    }

    /// Where the turning bike goes on the screen.
    pub fn preview_window(&self) -> [f32; 4] {
        match &self.screen {
            Screen::Extra(s) => s.preview().map_or(crate::preview::VIEWPORT, |p| p.2),
            _ => crate::preview::VIEWPORT,
        }
    }

    /// The player's choices on the Bike & Rider page.
    fn ride(&self) -> Option<Ride> {
        let mut ride = self.roster.default_ride()?;
        if let Some(b) = self.roster.bikes.get(self.bike) {
            ride.bike = b.clone();
        }
        if let Some(o) = self.roster.outfits.get(self.outfit) {
            ride.outfit = o.file.clone();
        }
        let mut plate: u32 = self.plate.parse().unwrap_or(0);
        if plate < 100 {
            plate += 100;
        }
        ride.plate = plate.clamp(101, 999);
        Some(ride)
    }

    /// The Bike & Rider page (`0x44fe50`, `0x450630`): the bike and outfit
    /// lists on the player's picks, the engine size, and the plate number.
    fn fill_bike_page(&mut self) {
        let bikes: Vec<String> = self.roster.bikes.iter().map(|b| b.name.clone()).collect();
        let outfits: Vec<String> = self.roster.outfits.iter().map(|o| o.name.clone()).collect();
        let cc = self.roster.bikes.get(self.bike).map_or(250, |b| b.cc);
        let size = ENGINE_SIZES
            .iter()
            .position(|e| e.0 >= cc)
            .unwrap_or(ENGINE_SIZES.len() - 1);
        let Screen::Single { page, .. } = &mut self.screen else {
            return;
        };
        page.set_items("DDLBikes", bikes, self.bike);
        page.set_items("DDLRiders", outfits, self.outfit);
        let sizes = ENGINE_SIZES.iter().map(|e| e.1.to_string()).collect();
        page.set_items("DDLEngineSize", sizes, size);
        page.enable("DDLEngineSize", false);
        page.enable("TxtEngineSize", false);
        page.set_on("ChkAutoRotate", true);
        page.set_text("EditPlateNumber", self.plate.clone());
    }

    /// The Bike & Rider page's arrows and boxes.
    fn bike_clicked(&mut self, name: &str) {
        let step = |at: usize, count: usize, by: isize| {
            (at as isize + by).rem_euclid(count.max(1) as isize) as usize
        };
        let (bikes, outfits) = (self.roster.bikes.len(), self.roster.outfits.len());
        match name {
            "bikeleft" => {
                self.bike = step(self.bike, bikes, -1);
                self.bike_picked = true;
            }
            "bikeright" => {
                self.bike = step(self.bike, bikes, 1);
                self.bike_picked = true;
            }
            "riderleft" => self.outfit = step(self.outfit, outfits, -1),
            "riderright" => self.outfit = step(self.outfit, outfits, 1),
            "editplatenumber" => {
                self.typing = true;
                return;
            }
            _ => return,
        }
        if self.typing {
            self.commit_plate();
        }
        self.keep_ride();
        self.fill_bike_page();
    }

    /// Keys typed into the plate number box: up to three digits.
    pub fn type_char(&mut self, c: char) {
        if self.extra_up() {
            let mut cx = Cx {
                ui: &mut self.ui,
                audio: None,
                user: &mut self.user,
                now: self.now,
            };
            match &mut self.screen {
                Screen::Extra(s) => {
                    let go = s.type_char(&mut cx, c);
                    self.go(go);
                }
                Screen::Main(d) => {
                    let events = self
                        .boxes
                        .dialog()
                        .map(|b| b.type_char(c))
                        .unwrap_or_default();
                    for e in events {
                        self.boxes.event(&mut cx, d, e);
                    }
                }
                _ => {}
            }
            return;
        }
        if !self.typing {
            return;
        }
        match c {
            '0'..='9' if self.plate.len() < 3 => self.plate.push(c),
            '\u{8}' => {
                self.plate.pop();
            }
            '\r' | '\n' | '\t' => self.commit_plate(),
            _ => return,
        }
        let plate = self.plate.clone();
        if let Screen::Single { page, .. } = &mut self.screen {
            page.set_text("EditPlateNumber", plate);
        }
    }

    /// Leaving the plate box (`0x44fe50`, message 10): numbers run from 101
    /// to 999, and one under 100 gains 100.
    fn commit_plate(&mut self) {
        self.typing = false;
        let mut n: u32 = self.plate.parse().unwrap_or(0);
        if n < 100 {
            n += 100;
        }
        self.plate = n.clamp(101, 999).to_string();
        self.keep_ride();
    }

    /// The profile's bike, outfit and plate on the Bike & Rider page, by
    /// name (`0x44fe50`); a name no longer listed keeps the current pick.
    fn load_ride(&mut self) {
        let p = &self.user.profile;
        if let Some(i) = self.roster.bikes.iter().position(|b| b.name == p.bike) {
            self.bike = i;
            self.bike_picked = true;
        }
        if let Some(i) = self.roster.outfits.iter().position(|o| o.name == p.outfit) {
            self.outfit = i;
        }
        if (101..=999).contains(&p.plate) {
            self.plate = p.plate.to_string();
        }
    }

    /// The page's picks into the profile, saved (`0x4513f0`), so the next
    /// time the player signs in they ride the same.
    fn keep_ride(&mut self) {
        let bike = self.roster.bikes.get(self.bike).filter(|_| self.bike_picked).map(|b| b.name.clone());
        let outfit = self.roster.outfits.get(self.outfit).map(|o| o.name.clone());
        let plate = self.plate.parse().ok().filter(|n| (101..=999).contains(n));
        let p = &mut self.user.profile;
        let before = (p.bike.clone(), p.outfit.clone(), p.plate);
        if let Some(b) = bike {
            p.bike = b;
        }
        if let Some(o) = outfit {
            p.outfit = o;
        }
        if let Some(n) = plate {
            p.plate = n;
        }
        if (p.bike.clone(), p.outfit.clone(), p.plate) != before {
            self.user.save();
        }
    }

    /// The Race Info page (`0x4521b0`): for a race, each rider's name,
    /// place, and best lap with the total time after it ("incomplete" if
    /// they did not finish), in finishing order. The replay button waits
    /// for a replay to view.
    fn fill_race_info(&mut self) {
        let heads = if self.results.stunt {
            [self.ui.lang(2356), String::new(), self.ui.lang(2359)]
        } else {
            [
                self.ui.lang(2356),
                self.ui.lang(2357),
                format!("{} ({})", self.ui.lang(2358), self.ui.lang(5048)),
            ]
        };
        let Screen::Single { page, .. } = &mut self.screen else {
            return;
        };
        for (i, head) in heads.into_iter().enumerate() {
            page.set_text(&format!("ButStats{}", i + 1), head);
        }
        for column in 0..4 {
            let items = self
                .results
                .lines
                .iter()
                .map(|r| r[column].clone())
                .collect();
            page.set_items(&format!("LstStats{}", column + 1), items, 0);
        }
        page.enable("ButViewReplay", self.results.replay.is_some());
    }

    fn open_single(&mut self) {
        self.load_ride();
        let Some(mut base) = Dlg::open(&mut self.ui, "SPBASE", (0.0, 0.0)) else {
            return;
        };
        // The tabs' names come from the code (`0x44e660`).
        for (tab, id) in [
            ("EventTab", 5091),
            ("BikeRiderTab", 5092),
            ("RaceInfoTab", 5093),
        ] {
            base.set_text(tab, self.ui.lang(id));
            base.set_font(tab, 18.0);
        }
        base.set_on("EventTab", true);
        let Some(page) = Dlg::open(&mut self.ui, "SPEVENT", PAGE) else {
            return;
        };
        self.screen = Screen::Single { base, page };
        self.fill_event_page();
    }

    /// The event page's arrows.
    fn page_clicked(&mut self, name: &str) {
        let Screen::Single { page, .. } = &mut self.screen else {
            return;
        };
        if page.index("DDLBikes").is_some() {
            return self.bike_clicked(name);
        }
        if page.index("LapsListBox").is_none() {
            return;
        }
        let step = |page: &mut Dlg, list: &str, by: i32| {
            let count = page.get(list).map_or(0, |c| c.items.len()) as i32;
            let at = (page.selected(list) as i32 + by).clamp(0, (count - 1).max(0));
            page.select(list, at as usize);
        };
        match name {
            "ghostleft" => step(page, "DDLGhostRaces", -1),
            "ghostright" => step(page, "DDLGhostRaces", 1),
            "lapsleftarrow" => step(page, "LapsListBox", -1),
            "lapsrightarrow" => step(page, "LapsListBox", 1),
            "opponentsleftarrow" => step(page, "OpponentsListBox", -1),
            "opponentsrightarrow" => step(page, "OpponentsListBox", 1),
            "racesleftarrow" => step(page, "RacesListBox", -1),
            "racesrightarrow" => step(page, "RacesListBox", 1),
            "trackleft" | "trackright" => {
                let count = page.get("DDLTextBox").map_or(0, |c| c.items.len());
                if count > 0 {
                    let at = page.selected("DDLTextBox");
                    let at = if name == "trackleft" {
                        (at + count - 1) % count
                    } else {
                        (at + 1) % count
                    };
                    page.select("DDLTextBox", at);
                }
            }
            _ => {}
        }
        self.read_event_page();
        // A new track has its own ghosts.
        if self.mode == GHOST && name.starts_with("track") {
            self.fill_event_page();
        }
    }

    fn selected(&mut self, name: &str, line: usize) {
        match name.to_ascii_lowercase().as_str() {
            "ddlbikes" => {
                self.bike = line;
                self.bike_picked = true;
                self.keep_ride();
                self.fill_bike_page();
            }
            "ddlriders" => {
                self.outfit = line;
                self.keep_ride();
            }
            "eventtypedropdown" => {
                let event = EventKind::LIST[line.min(EventKind::LIST.len() - 1)].0;
                if event != self.event {
                    self.event = event;
                    self.track = 0;
                }
                self.fill_event_page();
            }
            "racemodedropdown" => {
                self.read_event_page();
                self.fill_event_page();
            }
            "ddltextbox" => {
                self.read_event_page();
                if self.mode == GHOST {
                    self.fill_event_page();
                }
            }
            _ => self.read_event_page(),
        }
    }

    /// Takes the page's choices.
    fn read_event_page(&mut self) {
        let Screen::Single { page, .. } = &self.screen else {
            return;
        };
        if page.index("LapsListBox").is_none() {
            return;
        }
        let modes: Vec<u32> = MODES
            .iter()
            .map(|m| m.0)
            .filter(|&m| m != GHOST || self.event != EventKind::Stunt)
            .collect();
        self.mode = modes[page.selected("RaceModeDropDown").min(modes.len() - 1)];
        let item = |name: &str| {
            page.get(name)
                .and_then(|c| c.item())
                .and_then(|s| s.parse().ok())
        };
        if let Some(laps) = item("LapsListBox") {
            self.laps = laps;
        }
        if let Some(n) = item("OpponentsListBox") {
            self.opponents = n;
        }
        self.track = page.selected("DDLTextBox");
        self.ghost = page.selected("DDLGhostRaces");
    }

    /// The ghost race list (`0x44fa20`): "Record a new ghost race", then
    /// each `Record\*.gho` made on this track and course (`0x44fb30`).
    fn ghost_list(&mut self, entry: Option<&TrackEntry>) -> Vec<String> {
        let mut lines = vec![self.ui.lang(5319)];
        self.ghosts.clear();
        let Some(entry) = entry else {
            return lines;
        };
        let stem = entry
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let dir = crate::replay::record_dir(&self.user.store.dir);
        let mut files: Vec<PathBuf> = crate::platform::fs::read_dir(dir)
            .map(|d| {
                d.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gho")))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        for f in files {
            let Some(tape) = crate::platform::fs::read(&f)
                .ok()
                .and_then(|b| mm2_formats::vcr::Tape::parse(&b).ok())
            else {
                continue;
            };
            if tape.track().eq_ignore_ascii_case(stem)
                && tape.length > 0.0
                && tape.course() == entry.course
            {
                lines.push(format!("{} - {}", self.ui.lang(5320), tape.description));
                self.ghosts.push(f);
            }
        }
        lines
    }

    /// Fills the event page for the chosen event (`0x44f2c0`), and shows
    /// the boxes its race mode uses (`0x44ee90`).
    fn fill_event_page(&mut self) {
        let event = self.event;
        let tracks = self.tracks(event).to_vec();
        let lang = |id| self.ui.lang(id);
        let events: Vec<String> = EventKind::LIST.iter().map(|e| lang(e.1)).collect();
        let modes: Vec<(u32, String)> = MODES
            .iter()
            .filter(|m| m.0 != GHOST || event != EventKind::Stunt)
            .map(|m| (m.0, lang(m.1)))
            .collect();
        let laps_label = lang(if event == EventKind::Stunt {
            2351
        } else {
            2345
        });
        let Screen::Single { page, .. } = &mut self.screen else {
            return;
        };
        if page.index("LapsListBox").is_none() {
            return;
        }
        let at = EventKind::LIST
            .iter()
            .position(|e| e.0 == event)
            .unwrap_or(0);
        page.set_items("EventTypeDropDown", events, at);
        let mode_at = modes.iter().position(|m| m.0 == self.mode).unwrap_or(1);
        self.mode = modes[mode_at].0;
        page.set_items(
            "RaceModeDropDown",
            modes.into_iter().map(|m| m.1).collect(),
            mode_at,
        );
        page.set_items(
            "OpponentsListBox",
            (0..=10).map(|n| n.to_string()).collect(),
            self.opponents.min(10) as usize,
        );
        let (from, to) = if event == EventKind::Stunt {
            (2, 30)
        } else {
            (event.fewest_laps(), 20)
        };
        self.laps = self.laps.clamp(from, to);
        page.set_items(
            "LapsListBox",
            (from..=to).map(|n| n.to_string()).collect(),
            (self.laps - from) as usize,
        );
        page.set_text("LapsLabel", laps_label);
        page.set_items(
            "RacesListBox",
            [2, 4, 6].iter().map(|n| n.to_string()).collect(),
            0,
        );
        self.track = self.track.min(tracks.len().saturating_sub(1));
        page.set_items(
            "DDLTextBox",
            tracks.iter().map(|t| t.name.clone()).collect(),
            self.track,
        );
        if self.mode == GHOST {
            let lines = self.ghost_list(tracks.get(self.track));
            self.ghost = self.ghost.min(lines.len() - 1);
            if let Screen::Single { page, .. } = &mut self.screen {
                page.set_items("DDLGhostRaces", lines, self.ghost);
            }
        }
        let Screen::Single { page, .. } = &mut self.screen else {
            return;
        };
        // The boxes each race mode uses.
        let mode = self.mode;
        page.show("OpponentsControlBox", mode != GHOST);
        page.show("ChkRecordRace", mode != GHOST);
        page.show("GhostBox", mode == GHOST);
        page.show_group(1011, mode != GHOST);
        page.show("LapsControlBox", mode != 0 && mode != GHOST);
        page.show("racesControlBox", mode == 2);
        // Random gates are for Baja races.
        page.show("WaypointBox", event == EventKind::Baja && mode != GHOST);
        page.show("ChkTreeCollision", true);
        page.set_on("RadLODMedium", true);
    }

    /// The tracks of an event, in the order the game lists them (by file
    /// name), each course of a Baja or enduro track its own line.
    fn tracks(&mut self, event: EventKind) -> &[TrackEntry] {
        let game = self.ui.game().to_path_buf();
        // In the browser the event's files may still be on their way.
        if !self.tracks.contains_key(&event) && !crate::platform::fs::ensure_folder(game.join(event.folder())) {
            self.awaiting = Some(event);
            return &[];
        }
        self.tracks
            .entry(event)
            .or_insert_with(|| list_tracks(&game.join(event.folder()), event))
    }

    fn start(&mut self) -> Next {
        self.read_event_page();
        if let Some(b) = self.roster.bikes.get(self.bike).filter(|_| self.bike_picked) {
            let classes = crate::screens::options::engine_classes(self.ui.game());
            let mut garage = self.user.profile.garage.clone();
            crate::screens::options::ride_bike(&mut garage, &classes, b.cc);
            if garage != self.user.profile.garage {
                self.user.profile.garage = garage;
                if let Err(e) = self.user.store.save(&self.user.profile) {
                    eprintln!("could not save the profile: {e}");
                }
            }
        }
        let (event, track) = (self.event, self.track);
        let Some(entry) = self.tracks(event).get(track).cloned() else {
            return Next::Stay;
        };
        if let Some(d) = Dlg::open(&mut self.ui, self.event.loading(), (0.0, 0.0)) {
            self.screen = Screen::Loading(d);
        }
        Next::Race(Setup {
            event: self.event,
            track: entry.path,
            course: entry.course,
            laps: if self.mode == 0 { 99 } else { self.laps },
            opponents: if self.mode == GHOST {
                0
            } else {
                self.opponents
            },
            mode: self.mode,
            name: entry.name,
            ride: self.ride(),
            level: 1,
            names: vec![self.user.profile.name.clone()],
            rides: Vec::new(),
            ghost: match self.mode {
                GHOST => self
                    .ghost
                    .checked_sub(1)
                    .and_then(|g| self.ghosts.get(g))
                    .cloned(),
                _ => None,
            },
            net: None,
        })
    }

    pub fn spread(&self) -> u8 {
        if !crate::platform::FILL_MENUS {
            return 0;
        }
        match &self.screen {
            Screen::Main(_) => 1,
            _ => 2,
        }
    }

    pub fn draw(&mut self, gfx: &mut Gfx, now: f32, progress: f32) {
        // Load the pull-downs' pictures a few at a time as the menu sits.
        if !self.warmed {
            if let Screen::Main(d) = &self.screen {
                self.warm = d.fx_frames();
                self.warm.reverse();
                self.warmed = true;
            }
        }
        for _ in 0..3 {
            let Some((pack, name)) = self.warm.pop() else { break };
            self.ui.image(gfx, &pack, &name);
        }
        gfx.sprites.spread = self.spread();
        self.draw_screen(gfx, now, progress);
        gfx.sprites.spread = 0;
    }

    fn draw_screen(&mut self, gfx: &mut Gfx, now: f32, progress: f32) {
        let dt = (now - self.now).max(0.0);
        self.now = now;
        self.events_tick(dt);
        if let Some(event) = self.awaiting {
            let folder = self.ui.game().join(event.folder());
            if crate::platform::fs::ensure_folder(folder) {
                self.awaiting = None;
                if event == self.event && matches!(self.screen, Screen::Single { .. }) {
                    self.fill_event_page();
                }
            }
        }
        if let Screen::Extra(s) = &mut self.screen {
            let mut cx = Cx {
                ui: &mut self.ui,
                audio: None,
                user: &mut self.user,
                now,
            };
            let go = s.tick(&mut cx);
            let next = self.go(go);
            if !matches!(next, Next::Stay) {
                self.pending = Some(next);
            }
        }
        let event = self.event;
        let track = self.track;
        let preview = match &self.screen {
            Screen::Single { page, .. } if page.index("PictureBox").is_some() => {
                self.tracks(event).get(track).cloned()
            }
            _ => None,
        };
        let ui = &mut self.ui;
        match &mut self.screen {
            Screen::Main(d) => {
                d.tick(now);
                // The clips go over the Single pull-down and under the
                // screen's frame (`0x44c1d0` puts the video control just
                // before ScreenOverCtl).
                d.draw_background(ui, gfx);
                let n = d.ctls_len();
                let over = d.index("ScreenOverCtl").unwrap_or(n);
                d.draw_some(ui, gfx, now, 0..over);
                if let Some(m) = &mut self.events {
                    let (x, y) = d.origin;
                    m.draw(gfx, now, [x + 355.0, y + 113.0, 144.0, 104.0]);
                }
                d.draw_some(ui, gfx, now, over..n);
                d.draw_list(ui, gfx);
                if COMING_SOON && d.is_showing("ButTCP") {
                    coming_soon(d, ui, gfx);
                }
                if self.boxes.dims() {
                    // Half as bright: 0.75 of black blended on the linear
                    // target about halves the stored colours.
                    ui.fill(gfx, [0.0, 0.0, 640.0, 480.0], [0.0, 0.0, 0.0, 0.75]);
                }
                if let Some(b) = self.boxes.dialog() {
                    b.tick(now);
                    b.draw(ui, gfx, now);
                }
            }
            Screen::Extra(s) => s.draw(ui, gfx, now),
            Screen::Single { base, page } => {
                base.tick(now);
                base.draw(ui, gfx, now);
                page.tick(now);
                page.draw_under(ui, gfx, now);
                if let (Some(t), Some(r)) = (preview, page.rect_of("PictureBox")) {
                    let key = format!("{}#{}", t.path.display(), t.preview);
                    if let Some(tex) = ui.picture(gfx, &key, || preview_image(&t)) {
                        let (w, h) = gfx.sprites.size(tex);
                        gfx.sprites.image(
                            tex,
                            (r[0] + (r[2] - w as f32) / 2.0).round(),
                            (r[1] + (r[3] - h as f32) / 2.0).round(),
                        );
                    }
                }
                page.draw_list(ui, gfx);
            }
            Screen::Loading(d) => {
                d.draw(ui, gfx, now);
                // The bar fills as the track loads. The picture has a glow line baked into
                // the trough, the colour of the fill, so the part not yet filled is darkened
                // and the fill can be seen to grow.
                if let Some(i) = d.index("StaBarRect") {
                    let r = d.rect(i);
                    if let Some(t) = ui.image(gfx, &d.pack, "BlueBar") {
                        let p = progress.clamp(0.0, 1.0);
                        if p < 1.0 {
                            gfx.sprites.stretched_tinted(
                                t,
                                [p, 0.0, 1.0, 1.0],
                                [r[0] + r[2] * p, r[1], r[2] * (1.0 - p), r[3]],
                                [0.5, 0.45, 0.27, 1.0],
                            );
                        }
                        gfx.sprites.stretched(t, [0.0, 0.0, p, 1.0], [r[0], r[1], r[2] * p, r[3]]);
                    }
                }
            }
        }
        // The ghost lap question, over whatever screen the race went back to.
        if let Some((d, _)) = &mut self.ghost_ask {
            d.tick(now);
            d.draw(ui, gfx, now);
        }
        // The pointer's tip, from the dialog on top, if the player keeps
        // the Game page's "Show Tool Tips" on.
        let mut layers: Vec<&mut Dlg> = match (&mut self.ghost_ask, &mut self.screen) {
            (Some((d, _)), _) => vec![d],
            (None, Screen::Main(d)) => match self.boxes.dialog() {
                Some(b) => vec![b],
                None => vec![d],
            },
            (None, Screen::Extra(s)) => s.layers(),
            (None, Screen::Single { base, page }) => vec![base, page],
            (None, Screen::Loading(_)) => Vec::new(),
        };
        let top = layers.iter().rposition(|d| d.has_tip());
        let on = self.user.profile.game.tooltips;
        for (k, d) in layers.iter_mut().enumerate() {
            d.draw_tip(ui, gfx, now, on && Some(k) == top);
        }
    }
}

/// Multiplayer is not in the browser build yet: its connection buttons are
/// greyed and the Multiplayer pull-down's screen says so.
const COMING_SOON: bool = cfg!(target_arch = "wasm32");

/// Dims the Multiplayer pull-down's screen (the connection icons) and writes
/// "COMING SOON" across it.
fn coming_soon(d: &Dlg, ui: &mut Ui, gfx: &mut Gfx) {
    let mut area: Option<[f32; 4]> = None;
    for name in ["ButTCP", "ButIPX", "ButZone", "ButCable", "ButModem"] {
        let Some(r) = d.rect_of(name) else {
            continue;
        };
        area = Some(match area {
            None => r,
            Some(a) => {
                let (x0, y0) = (a[0].min(r[0]), a[1].min(r[1]));
                let (x1, y1) = ((a[0] + a[2]).max(r[0] + r[2]), (a[1] + a[3]).max(r[1] + r[3]));
                [x0, y0, x1 - x0, y1 - y0]
            }
        });
    }
    let Some(a) = area else {
        return;
    };
    let pad = 10.0;
    let screen = [a[0] - pad, a[1] - pad, a[2] + 2.0 * pad, a[3] + 2.0 * pad];
    ui.fill(gfx, screen, [0.02, 0.04, 0.12, 0.78]);
    let Some(t) = ui.text(gfx, "COMING SOON", 22.0, true) else {
        return;
    };
    let (w, h) = gfx.sprites.size(t);
    let (w, h) = (w as f32, h as f32);
    let x = (screen[0] + (screen[2] - w) / 2.0).round();
    let y = (screen[1] + (screen[3] - h) / 2.0).round();
    // A gold band behind the lettering, like the menu's chain trim.
    let band = [screen[0], y - 6.0, screen[2], h + 12.0];
    ui.fill(gfx, band, [0.0, 0.0, 0.0, 0.55]);
    ui.fill(gfx, [band[0], band[1], band[2], 1.0], [0.79, 0.66, 0.18, 1.0]);
    ui.fill(gfx, [band[0], band[1] + band[3] - 1.0, band[2], 1.0], [0.79, 0.66, 0.18, 1.0]);
    gfx.sprites.quad(t, [0.0, 0.0, 1.0, 1.0], [x, y, w, h], [0.94, 0.85, 0.44, 1.0]);
}

/// The main menu's buttons and the pull-downs they open.
const PULL_DOWNS: [(&str, &str); 3] = [
    ("menusingle", "MenuSingleAnimation"),
    ("menumulti", "MenuMultiAnimation"),
    ("menuuserprofile", "MenuUserAnimation"),
];

pub fn list_tracks(folder: &Path, event: EventKind) -> Vec<TrackEntry> {
    let mut files: Vec<PathBuf> = crate::platform::fs::read_dir(folder)
        .map(|d| {
            d.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("env")))
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|p| p.file_name().map(|n| n.to_ascii_uppercase()));
    let mut out = Vec::new();
    for path in files {
        let Ok(bytes) = crate::read_archive(&path) else {
            continue;
        };
        let Ok(ar) = rs2::Archive::parse(&bytes) else {
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let ini = ar
            .entries()
            .iter()
            .find(|e| e.name.to_ascii_lowercase().ends_with(".scn"))
            .map(|e| scn::Ini::parse(&String::from_utf8_lossy(ar.data(e))));
        let info = ini.as_ref().and_then(|i| i.section("SceneInfo"));
        let name = info
            .and_then(|s| s.get("SceneName"))
            .filter(|n| !n.is_empty())
            .unwrap_or(&stem)
            .to_string();
        if event.courses() {
            let courses = ar
                .entries()
                .iter()
                .filter(|e| e.name.to_ascii_lowercase().ends_with(".tdf"))
                .count();
            for c in 0..courses {
                let course = info
                    .and_then(|s| s.get(&format!("WaypointRaceName{}", c + 1)))
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{name} {}", c + 1));
                out.push(TrackEntry {
                    path: path.clone(),
                    course: c,
                    name: course,
                    preview: format!("{stem}{:02}.tga", c + 1),
                });
            }
        } else {
            out.push(TrackEntry {
                path: path.clone(),
                course: 0,
                name,
                preview: format!("{stem}.tga"),
            });
        }
    }
    out
}

pub fn preview_image(t: &TrackEntry) -> Option<tga::Image> {
    let bytes = crate::read_archive(&t.path).ok()?;
    let ar = rs2::Archive::parse(&bytes).ok()?;
    let entry = ar
        .entries()
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(&t.preview))?;
    tga::parse(ar.data(entry)).ok()
}
