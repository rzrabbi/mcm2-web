//! Multiplayer (`NetProcs.cpp`): the Host or Join box a connection on the
//! main menu's Multiplayer pull-down opens (`UI/HOSTJOIN.DAT`,
//! `0x4b2bd0`), the session name box (`0x4b36a0`), and the game's lobby
//! (`UI/MPBASE.DAT`, `0x4f9400`) with its players, Ready buttons, chat and
//! four pages: Event (`MPEVENT`, `0x4fb130`), Bike & Rider (`MPBIKER`,
//! `0x4fc670`), Race Info (`MPRINFO`, the single player page's
//! `0x4521b0`) and Options (`MPOPT`, `0x4fe1a0`).
//!
//! The game played over DirectPlay; the port plays over its own network
//! code (`net`): the Host or Join box lists the sessions found on the
//! local network, the host's Event and Options pages go to everyone, and
//! the host starts the race once every player is ready.

use super::{options::Options, popup, Cx, Go, Screen};
use crate::frontend::{
    list_tracks, preview_image, EventKind, NetRace, RaceResults, Setup, TrackEntry, PRACTICE,
};
use crate::net::{self, GameInfo, Heard, Msg, Player, RideInfo};
use crate::roster::{Ride, Roster};
use crate::ui::{Dlg, Event, Gfx, Ui};
use std::collections::HashMap;

/// Where the lobby's pages go (`0x4fa7c0`).
const PAGE: (f32, f32) = (17.0, 35.0);

/// The Bike & Rider page's window for the turning bike, inside its
/// corner marks.
const WINDOW: [f32; 4] = [27.0, 105.0, 285.0, 141.0];

/// The lobby's tabs and their names (`0x4f9400` open).
const TABS: [(&str, &str, u32); 4] = [
    ("EventTab", "MPEVENT", 5091),
    ("BikeRiderTab", "MPBIKER", 5092),
    ("RaceInfoTab", "MPRINFO", 5093),
    ("OptionsTab", "MPOPT", 5224),
];

/// The Event page's events, in its list's order (`0x4fb130`).
const EVENTS: [(EventKind, u32); 6] = [
    (EventKind::Baja, 5101),
    (EventKind::Stunt, 5102),
    (EventKind::Enduro, 5104),
    (EventKind::Nationals, 5103),
    (EventKind::Supercross, 5106),
    (EventKind::Tag, 5105),
];

/// Moto-Tag is played on its own arenas and the Baja and enduro tracks
/// (`0x4fbc30`).
const TAG_TRACKS: [EventKind; 3] = [EventKind::Tag, EventKind::Baja, EventKind::Enduro];

/// The race modes (strings 5107 to 5109): practice, single event,
/// tournament.
const MODES: [u32; 3] = [5107, 5108, 5109];
const TOURNAMENT: u32 = 2;

/// The engine sizes of the Bike & Rider page.
const ENGINES: [&str; 5] = [
    "125cc 2-stroke",
    "250cc 2-stroke",
    "400cc 4-stroke",
    "500cc 2-stroke",
    "600cc 4-stroke",
];

/// Grey for a player who is not ready, white for one who is (BGR).
const NOT_READY: i32 = 0x808080;
const READY: i32 = 0xffffff;

/// The lobby's choices, kept across its pages and races.
struct Game {
    event: EventKind,
    mode: u32,
    laps: u32,
    riders: u32,
    races: usize,
    track: usize,
    bike: usize,
    outfit: usize,
    /// Largest engine allowed, from the list.
    largest: usize,
    level: usize,
    trees: bool,
    fast_finishes: bool,
    collisions: bool,
    record: bool,
    max_ai: u32,
    /// Moto-Tag with the ball (`ChkTagBall`, +0x2eb4), and with the IT
    /// rider scoring stunt points (`ChkStuntMode`, +0x2eb8).
    tag_ball: bool,
    tag_stunts: bool,
}

impl Default for Game {
    fn default() -> Self {
        Game {
            event: EventKind::Supercross,
            mode: 1,
            laps: 3,
            riders: 3,
            races: 0,
            track: 0,
            bike: 0,
            outfit: 0,
            largest: ENGINES.len() - 1,
            level: 1,
            trees: true,
            fast_finishes: false,
            collisions: true,
            record: false,
            max_ai: 4,
            tag_ball: false,
            tag_stunts: false,
        }
    }
}

enum View {
    /// Host or Join, over the main menu, with the session name box once
    /// Host is clicked, or first the TCP/IP box asking for the host's
    /// address (`address`). Before it all, over the menu alone, the box
    /// asking which kind of TCP/IP game (`kind`).
    HostJoin {
        menu: Dlg,
        dlg: Dlg,
        kind: Option<Box<Dlg>>,
        name: Option<Box<Dlg>>,
        address: bool,
    },
    Lobby {
        base: Dlg,
        page: Dlg,
        tab: usize,
        sub: Option<Box<dyn Screen>>,
    },
}

pub struct Multi {
    view: View,
    game: Game,
    session: String,
    ready: bool,
    chat: Vec<String>,
    roster: Roster,
    tracks: HashMap<EventKind, Vec<TrackEntry>>,
    results: RaceResults,
    /// Looking for sessions, while the Host or Join box is up.
    finder: Option<net::Finder>,
    /// The session line picked.
    picked: Option<usize>,
    /// The host's game as last sent.
    sent: Option<GameInfo>,
}

/// The Event page's controls a joined player may not change.
const HOST_ONLY: [&str; 18] = [
    "EventTypeDropDown",
    "RaceModeDropDown",
    "OpponentsListBox",
    "OpponentsLeftarrow",
    "OpponentsrightArrow",
    "LapsListBox",
    "LapsLeftArrow",
    "LapRightArrow",
    "RacesListBox",
    "RacesLeftArrow",
    "RacesRightArrow",
    "DDLTextBox",
    "TrackLeft",
    "TrackRight",
    "ChkTagBall",
    "ChkStuntMode",
    "Start",
    "LargestOpponentDropDown",
];

impl Multi {
    /// The Host or Join box (`0x4b2bd0` open): no sessions, so Join is
    /// greyed. First, over the menu with its Multiplayer pull-down still
    /// down, the box asking LAN or Internet (lang 5096, 5118).
    pub fn open(cx: &mut Cx) -> Option<Multi> {
        let mut menu = Dlg::open(cx.ui, "MAINMENU", (0.0, 0.0))?;
        super::profiles::fill(cx, &mut menu);
        menu.pull("MenuMultiAnimation", true, cx.now - 10.0, true);
        let mut dlg = popup(cx.ui, "HOSTJOIN")?;
        dlg.set_items("LstSessions", Vec::new(), 0);
        dlg.enable("ButJoin", false);
        dlg.set_color("ButJoin", NOT_READY);
        let (title, prompt) = (cx.ui.lang(5096), cx.ui.lang(5118));
        let kind =
            super::MsgBox::ask(cx.ui, &title, &prompt, [5095, 5094, 5084]).map(|b| Box::new(b.dlg));
        Some(Multi {
            view: View::HostJoin {
                menu,
                dlg,
                kind,
                address: false,
                name: None,
            },
            game: Game::default(),
            session: String::new(),
            ready: false,
            chat: Vec::new(),
            roster: Roster::load(cx.game()),
            tracks: HashMap::new(),
            results: RaceResults::default(),
            finder: Some(net::Finder::new()),
            picked: None,
            sent: None,
        })
    }

    /// This player as the session knows them.
    fn me(&self, cx: &Cx) -> Player {
        let ride = self.ride(cx.user.profile.plate);
        Player {
            id: 0,
            name: Self::player(cx),
            ready: self.ready,
            ride: ride
                .map(|r| RideInfo {
                    model: r.bike.model,
                    texture: r.bike.texture,
                    outfit: r.outfit,
                    cc: r.bike.cc,
                    plate: r.plate,
                })
                .unwrap_or_default(),
        }
    }

    /// Whether this player joined someone else's session.
    fn guest() -> bool {
        net::with(|s| !s.host).unwrap_or(false)
    }

    /// The host's game, to send.
    fn game_info(&mut self, ui: &Ui) -> GameInfo {
        let event = self.game.event;
        let track = self.tracks(ui, event).get(self.game.track).cloned();
        let g = &self.game;
        GameInfo {
            event: event as u8,
            mode: g.mode as u8,
            laps: g.laps,
            computers: if event == EventKind::Tag { 0 } else { g.riders },
            track: track
                .as_ref()
                .and_then(|t| t.path.file_stem())
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string(),
            course: track.map_or(0, |t| t.course as u8),
            level: g.level as u8,
            tag_ball: g.tag_ball,
            tag_stunts: g.tag_stunts,
            collisions: g.collisions,
        }
    }

    /// Takes the host's game.
    fn apply_game(&mut self, ui: &Ui, info: &GameInfo) {
        let Some(event) = EventKind::from_number(info.event as u32) else {
            return;
        };
        self.game.event = event;
        self.game.mode = info.mode as u32;
        self.game.laps = info.laps;
        self.game.riders = info.computers;
        self.game.level = info.level as usize;
        self.game.tag_ball = info.tag_ball;
        self.game.tag_stunts = info.tag_stunts;
        self.game.collisions = info.collisions;
        let tracks = self.tracks(ui, event);
        if let Some(at) = tracks.iter().position(|t| {
            t.course == info.course as usize
                && t.path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case(&info.track))
        }) {
            self.game.track = at;
        }
    }

    /// The other players' names and Ready buttons.
    fn fill_players(&mut self) {
        let View::Lobby { base, .. } = &mut self.view else {
            return;
        };
        let (me, players) = net::with(|s| (s.me, s.players.clone())).unwrap_or((0, Vec::new()));
        let others: Vec<&Player> = players.iter().filter(|p| p.id != me).collect();
        for i in 2..=8 {
            let p = others.get(i - 2);
            base.set_text(
                &format!("NamePlayer{i}"),
                p.map_or(String::new(), |p| p.name.clone()),
            );
            let rdy = format!("ButRdyPlayer{i}");
            base.enable(&rdy, false);
            base.show(&rdy, p.is_some());
            base.set_color(
                &rdy,
                if p.is_some_and(|p| p.ready) {
                    READY
                } else {
                    NOT_READY
                },
            );
        }
    }

    /// The race the host started: the players in session order, then the
    /// host's computer riders.
    fn net_setup(&mut self, cx: &mut Cx, info: &GameInfo, players: &[Player]) -> Option<Setup> {
        self.apply_game(cx.ui, info);
        let event = self.game.event;
        let track = self.tracks(cx.ui, event).get(self.game.track).cloned()?;
        let me = net::with(|s| s.me).unwrap_or(0);
        let host = net::with(|s| s.host).unwrap_or(true);
        let at = players.iter().position(|p| p.id == me).unwrap_or(0);
        let ride = |p: &Player| {
            let mut r = self.roster.default_ride()?;
            let i = &p.ride;
            if !i.model.is_empty() {
                r.bike.model = i.model.clone();
                r.bike.texture = i.texture.clone();
                r.bike.cc = i.cc;
                r.outfit = i.outfit.clone();
                r.plate = i.plate;
            }
            Some(r)
        };
        let rides: Vec<Option<Ride>> = players.iter().map(ride).collect();
        let g = &self.game;
        Some(Setup {
            event,
            track: track.path,
            course: track.course,
            laps: if g.mode == PRACTICE { 99 } else { g.laps },
            opponents: (players.len() as u32 - 1) + info.computers,
            mode: g.mode,
            name: track.name,
            ride: rides.first().cloned().flatten(),
            level: g.level,
            names: players.iter().map(|p| p.name.clone()).collect(),
            rides,
            ghost: None,
            net: Some(NetRace {
                me: at,
                humans: players.len(),
                host,
                tag_ball: g.tag_ball,
                tag_stunts: g.tag_stunts,
            }),
        })
    }

    fn player(cx: &Cx) -> String {
        let name = cx.user.profile.name.trim();
        if name.is_empty() { "Player" } else { name }.to_string()
    }

    /// The session name box (`0x4b36a0`): the player's name to start with,
    /// up to 11 letters.
    fn name_box(cx: &mut Cx) -> Option<Dlg> {
        let mut d = popup(cx.ui, "MESSBOX1")?;
        d.set_text("TitleText", cx.ui.lang(2321));
        d.set_text("OKButton", cx.ui.lang(5097));
        d.set_text("CancelButton", cx.ui.lang(5084));
        d.set_text("TxtPrompt", "");
        let name: String = Self::player(cx).chars().take(11).collect();
        if let Some(c) = d.ctl("EditBox") {
            c.max_len = 11;
            c.text = Some(name);
        }
        d.edit(Some("EditBox"));
        Some(d)
    }

    /// The TCP/IP Settings box, as DirectPlay asked before looking for
    /// sessions: the host's address or computer name (lang 2200, 2201), or
    /// nothing to look on the local network.
    fn address_box(cx: &mut Cx) -> Option<Dlg> {
        let mut d = popup(cx.ui, "MESSBOX1")?;
        d.set_text("TitleText", cx.ui.lang(2200));
        d.set_text("OKButton", cx.ui.lang(5097));
        d.set_text("CancelButton", cx.ui.lang(5084));
        d.set_text("TxtPrompt", cx.ui.lang(2201));
        if let Some(c) = d.ctl("EditBox") {
            c.max_len = 63;
            c.text = Some(String::new());
        }
        d.edit(Some("EditBox"));
        Some(d)
    }

    /// The lobby (`0x4f9400` open), as the host, on the Event page.
    fn lobby(&mut self, cx: &mut Cx) {
        let Some(mut base) = Dlg::open(cx.ui, "MPBASE", (0.0, 0.0)) else {
            return;
        };
        for (tab, _, id) in TABS {
            base.set_text(tab, cx.ui.lang(id));
            base.set_font(tab, 18.0);
        }
        base.set_text("UserName", Self::player(cx));
        for i in 2..=8 {
            base.set_text(&format!("NamePlayer{i}"), "");
            base.enable(&format!("ButRdyPlayer{i}"), false);
            base.set_color(&format!("ButRdyPlayer{i}"), NOT_READY);
        }
        base.set_color("ButRdyUser", if self.ready { READY } else { NOT_READY });
        if let Some(c) = base.ctl("EditChat") {
            c.max_len = 60;
        }
        base.set_items("ListChat", self.chat.clone(), 0);
        let Some(page) = Dlg::open(cx.ui, TABS[0].1, PAGE) else {
            return;
        };
        self.view = View::Lobby {
            base,
            page,
            tab: 0,
            sub: None,
        };
        self.page(cx, 0);
        self.fill_players();
    }

    fn page(&mut self, cx: &mut Cx, tab: usize) {
        let Some(mut d) = Dlg::open(cx.ui, TABS[tab].1, PAGE) else {
            return;
        };
        match tab {
            0 => self.fill_event(cx.ui, &mut d),
            1 => self.fill_bike(cx, &mut d),
            2 => self.fill_results(cx.ui, &mut d),
            _ => self.fill_options(&mut d),
        }
        // A joined player sees the host's choices.
        if Self::guest() {
            for name in HOST_ONLY {
                d.enable(name, false);
            }
            if tab == 3 {
                for c in [
                    "ChkTreeCollision",
                    "FastFinishesCheckBox",
                    "ChkRiderCollision",
                    "ChkRecordRace",
                    "RadLODEasy",
                    "RadLODMedium",
                    "RadLODHard",
                    "EditMaxAIBikes",
                ] {
                    d.enable(c, false);
                }
            }
        }
        if let View::Lobby { base, .. } = &mut self.view {
            base.enable("Start", !Self::guest());
        }
        if let View::Lobby {
            base,
            page,
            tab: at,
            ..
        } = &mut self.view
        {
            for (i, (name, _, _)) in TABS.iter().enumerate() {
                base.set_on(name, i == tab);
            }
            *page = d;
            *at = tab;
        }
    }

    fn tracks(&mut self, ui: &Ui, event: EventKind) -> Vec<TrackEntry> {
        if event == EventKind::Tag {
            return TAG_TRACKS
                .iter()
                .flat_map(|&e| self.folder(ui, e))
                .collect();
        }
        self.folder(ui, event)
    }

    fn folder(&mut self, ui: &Ui, event: EventKind) -> Vec<TrackEntry> {
        let game = ui.game().to_path_buf();
        // In the browser the event's files may still be on their way.
        if !self.tracks.contains_key(&event) && !crate::platform::fs::ensure_folder(game.join(event.folder())) {
            return Vec::new();
        }
        self.tracks
            .entry(event)
            .or_insert_with(|| list_tracks(&game.join(event.folder()), event))
            .clone()
    }

    /// The Laps list's range: laps, or minutes for the stunt and tag
    /// events, which the list shows as times (`0x4fbc30`, `0x51dcc0`).
    fn laps_range(event: EventKind) -> (u32, u32, bool) {
        match event {
            EventKind::Stunt | EventKind::Tag => (2, 30, true),
            EventKind::Nationals | EventKind::Supercross => (2, 20, false),
            _ => (1, 20, false),
        }
    }

    /// The Event page (`0x4fb130` open, `0x4fbc30`, `0x4fb7a0`).
    fn fill_event(&mut self, ui: &Ui, d: &mut Dlg) {
        let g = &self.game;
        let event = g.event;
        let at = EVENTS.iter().position(|e| e.0 == event).unwrap_or(0);
        d.set_items(
            "EventTypeDropDown",
            EVENTS.iter().map(|e| ui.lang(e.1)).collect(),
            at,
        );
        d.set_items(
            "RaceModeDropDown",
            MODES.iter().map(|&m| ui.lang(m)).collect(),
            g.mode as usize,
        );
        d.set_items(
            "OpponentsListBox",
            (0..=10).map(|n| n.to_string()).collect(),
            g.riders.min(10) as usize,
        );
        let (from, to, timed) = Self::laps_range(event);
        let laps = self.game.laps.clamp(from, to);
        self.game.laps = laps;
        let line = |n: u32| {
            if timed {
                format!("{n:02}:00")
            } else {
                n.to_string()
            }
        };
        d.set_items(
            "LapsListBox",
            (from..=to).map(line).collect(),
            (laps - from) as usize,
        );
        d.set_text("LapsLabel", ui.lang(if timed { 2351 } else { 2345 }));
        d.set_items(
            "RacesListBox",
            ["2", "4", "6"].iter().map(|s| s.to_string()).collect(),
            self.game.races,
        );
        let tracks = self.tracks(ui, event);
        self.game.track = self.game.track.min(tracks.len().saturating_sub(1));
        d.set_items(
            "DDLTextBox",
            tracks.iter().map(|t| t.name.clone()).collect(),
            self.game.track,
        );
        let mode = self.game.mode;
        d.show("LapsControlBox", mode != PRACTICE);
        d.show("RacesControlBox", mode == TOURNAMENT);
        // Moto-Tag has no computer riders (`0x4fbc30`).
        let tag = event == EventKind::Tag;
        d.show("OpponentsControlBox", !tag);
        d.show("ChkTagBall", tag);
        d.show("ChkStuntMode", tag);
        d.set_on("ChkTagBall", self.game.tag_ball);
        d.set_on("ChkStuntMode", self.game.tag_stunts);
    }

    /// Takes the Event page's choices.
    fn read_event(g: &mut Game, d: &Dlg) {
        let item = |name: &str| {
            d.get(name)
                .and_then(|c| c.item())
                .and_then(|s| s.parse().ok())
        };
        let (from, _, _) = Self::laps_range(g.event);
        g.laps = from + d.selected("LapsListBox") as u32;
        g.tag_ball = d.get("ChkTagBall").is_some_and(|c| c.on);
        g.tag_stunts = d.get("ChkStuntMode").is_some_and(|c| c.on);
        if let Some(n) = item("OpponentsListBox") {
            g.riders = n;
        }
        g.races = d.selected("RacesListBox");
        g.track = d.selected("DDLTextBox");
    }

    /// The Bike & Rider page.
    fn fill_bike(&mut self, cx: &mut Cx, d: &mut Dlg) {
        let g = &self.game;
        let bikes = self.roster.bikes.iter().map(|b| b.name.clone()).collect();
        let outfits = self.roster.outfits.iter().map(|o| o.name.clone()).collect();
        d.set_items("DDLBikes", bikes, g.bike);
        d.set_items("DDLRiders", outfits, g.outfit);
        let engines: Vec<String> = ENGINES.iter().map(|e| e.to_string()).collect();
        d.set_items("LargestOpponentDropDown", engines.clone(), g.largest);
        let cc = self.roster.bikes.get(g.bike).map_or(250, |b| b.cc);
        let size = [125, 250, 400, 500, 600]
            .iter()
            .position(|&c| c >= cc)
            .unwrap_or(4);
        d.set_items("DDLEngineSize", engines, size);
        d.enable("DDLEngineSize", false);
        d.enable("TxtEngineSize", false);
        d.enable("ButWrench", false);
        d.set_text("EditPlateNumber", cx.user.profile.plate.to_string());
    }

    /// The Race Info page (`0x4521b0`): the last race's results.
    fn fill_results(&self, ui: &Ui, d: &mut Dlg) {
        let heads = if self.results.stunt {
            [ui.lang(2356), String::new(), ui.lang(2359)]
        } else {
            [
                ui.lang(2356),
                ui.lang(2357),
                format!("{} ({})", ui.lang(2358), ui.lang(5048)),
            ]
        };
        for (i, head) in heads.into_iter().enumerate() {
            d.set_text(&format!("ButStats{}", i + 1), head);
        }
        for column in 0..4 {
            let items = self
                .results
                .lines
                .iter()
                .map(|r| r[column].clone())
                .collect();
            d.set_items(&format!("LstStats{}", column + 1), items, 0);
        }
    }

    /// The Options page (`0x4fe1a0`).
    fn fill_options(&self, d: &mut Dlg) {
        let g = &self.game;
        d.set_on("ChkTreeCollision", g.trees);
        d.set_on("FastFinishesCheckBox", g.fast_finishes);
        d.set_on("ChkRiderCollision", g.collisions);
        d.set_on("ChkRecordRace", g.record);
        for (i, name) in ["RadLODEasy", "RadLODMedium", "RadLODHard"]
            .iter()
            .enumerate()
        {
            d.set_on(name, i == g.level);
        }
        d.set_text("EditMaxAIBikes", g.max_ai.to_string());
    }

    fn read_options(g: &mut Game, d: &Dlg) {
        let on = |n: &str| d.get(n).is_some_and(|c| c.on);
        g.trees = on("ChkTreeCollision");
        g.fast_finishes = on("FastFinishesCheckBox");
        g.collisions = on("ChkRiderCollision");
        g.record = on("ChkRecordRace");
        if let Some(l) = ["RadLODEasy", "RadLODMedium", "RadLODHard"]
            .iter()
            .position(|n| on(n))
        {
            g.level = l;
        }
        if let Some(n) = d
            .get("EditMaxAIBikes")
            .and_then(|c| c.text.as_deref())
            .and_then(|t| t.trim().parse().ok())
        {
            g.max_ai = n;
        }
    }

    fn ride(&self, plate: u32) -> Option<Ride> {
        let mut ride = self.roster.default_ride()?;
        if let Some(b) = self.roster.bikes.get(self.game.bike) {
            ride.bike = b.clone();
        }
        if let Some(o) = self.roster.outfits.get(self.game.outfit) {
            ride.outfit = o.file.clone();
        }
        ride.plate = plate;
        Some(ride)
    }

    /// Start (`0x4f71e0`): the race the Event page set, once the player
    /// is ready.
    fn start(&mut self, cx: &mut Cx) -> Go {
        if !self.ready {
            return Go::Stay;
        }
        // In a session the host starts, once everyone is ready.
        if net::with(|_| ()).is_some() {
            if Self::guest() || !net::with(|s| s.all_ready()).unwrap_or(false) {
                return Go::Stay;
            }
            let info = self.game_info(cx.ui);
            let players = net::with(|s| s.players.clone()).unwrap_or_default();
            net::with(|s| {
                s.say(Msg::Start {
                    game: info.clone(),
                    players: players.clone(),
                })
            });
            return match self.net_setup(cx, &info, &players) {
                Some(setup) => Go::Race(Box::new(setup)),
                None => Go::Stay,
            };
        }
        let g = &self.game;
        let event = g.event;
        let Some(track) = self.tracks(cx.ui, event).get(self.game.track).cloned() else {
            return Go::Stay;
        };
        let g = &self.game;
        Go::Race(Box::new(Setup {
            event,
            track: track.path,
            course: track.course,
            laps: if g.mode == PRACTICE { 99 } else { g.laps },
            opponents: g.riders,
            mode: g.mode,
            name: track.name,
            ride: self.ride(cx.user.profile.plate),
            level: g.level,
            names: vec![Self::player(cx)],
            rides: Vec::new(),
            ghost: None,
            net: None,
        }))
    }

    /// A line of the chat.
    fn say(&mut self, line: String) {
        self.chat.push(line);
        if let View::Lobby { base, .. } = &mut self.view {
            let at = self.chat.len().saturating_sub(1);
            base.set_items("ListChat", self.chat.clone(), at);
            if let Some(c) = base.ctl("ListChat") {
                c.scroll = self.chat.len().saturating_sub(6);
            }
        }
    }

    fn send_ride(&self, cx: &Cx) {
        let ride = self.me(cx).ride;
        net::with(|s| s.say(Msg::Ride(ride)));
    }

    /// What the session heard: players, chat, the host's game, the start.
    fn hear(&mut self, cx: &mut Cx) -> Go {
        let heard = net::with(|s| s.poll()).unwrap_or_default();
        for h in heard {
            match h {
                Heard::Players => self.fill_players(),
                Heard::Chat(line) => self.say(line),
                Heard::Game(g) => {
                    self.apply_game(cx.ui, &g);
                    if let View::Lobby { tab, sub: None, .. } = &self.view {
                        let tab = *tab;
                        if tab == 0 || tab == 3 {
                            self.page(cx, tab);
                        }
                    }
                }
                Heard::Start { game, players } => {
                    if let Some(setup) = self.net_setup(cx, &game, &players) {
                        return Go::Race(Box::new(setup));
                    }
                }
                Heard::Lost => {
                    net::set(None);
                    // "<session> has disconnected".
                    self.say(format!("{} {}", self.session, cx.ui.lang(3030)));
                    self.fill_players();
                }
                _ => {}
            }
        }
        // The host's choices go to everyone as they change.
        if net::with(|s| s.host).unwrap_or(false) {
            let info = self.game_info(cx.ui);
            if self.sent.as_ref() != Some(&info) {
                net::with(|s| s.say(Msg::Game(info.clone())));
                self.sent = Some(info);
            }
        }
        Go::Stay
    }

    /// Send, or Return in the chat box (`0x4f8790`).
    fn send(&mut self, cx: &mut Cx) {
        let View::Lobby { base, .. } = &mut self.view else {
            return;
        };
        let text = base
            .get("EditChat")
            .and_then(|c| c.text.clone())
            .unwrap_or_default();
        base.set_text("EditChat", "");
        base.edit(Some("EditChat"));
        if !text.trim().is_empty() {
            let line = format!("{}: {}", Self::player(cx), text.trim());
            net::with(|s| s.say(Msg::Chat(line.clone())));
            self.say(line);
        }
    }

    fn host_join_event(&mut self, cx: &mut Cx, e: Event) -> Go {
        let View::HostJoin {
            dlg,
            name,
            address,
            kind,
            ..
        } = &mut self.view
        else {
            return Go::Stay;
        };
        let clicked = |n: &str| matches!(&e, Event::Clicked(c) if c.eq_ignore_ascii_case(n));
        if kind.is_some() {
            // LAN looks on the local network; Internet asks for the host.
            if clicked("ButLeft") {
                *kind = None;
            } else if clicked("ButMiddle") {
                *kind = None;
                *name = Self::address_box(cx).map(Box::new);
                *address = name.is_some();
            } else if clicked("ButRight") {
                return Go::Back;
            }
            return Go::Stay;
        }
        if let Some(box_) = name {
            let done = clicked("OKButton")
                || matches!(&e, Event::Released(n) if n.eq_ignore_ascii_case("EditBox"));
            if done && *address {
                let host = box_
                    .get("EditBox")
                    .and_then(|c| c.text.clone())
                    .unwrap_or_default();
                if let (false, Some(f)) = (host.trim().is_empty(), &mut self.finder) {
                    f.hosts.push(host.trim().to_string());
                }
                *name = None;
                *address = false;
            } else if done {
                let session = box_
                    .get("EditBox")
                    .and_then(|c| c.text.clone())
                    .unwrap_or_default();
                if !session.trim().is_empty() {
                    self.session = session.trim().to_string();
                    match net::Session::host(&self.session, self.me(cx)) {
                        Ok(s) => net::set(Some(s)),
                        Err(e) => eprintln!("hosting {}: {e}", self.session),
                    }
                    self.finder = None;
                    self.lobby(cx);
                }
            } else if clicked("CancelButton") {
                *name = None;
                *address = false;
            } else if clicked("EditBox") {
                box_.edit(Some("EditBox"));
            }
            return Go::Stay;
        }
        if let Event::Selected(n, line) = &e {
            if n.eq_ignore_ascii_case("LstSessions") {
                self.picked = Some(*line);
                dlg.enable("ButJoin", true);
                dlg.set_color("ButJoin", READY);
            }
            return Go::Stay;
        }
        if clicked("ButJoin") {
            let found = self
                .picked
                .and_then(|i| self.finder.as_ref()?.found.get(i).cloned());
            if let Some(f) = found {
                match net::Session::join(f.addr, self.me(cx)) {
                    Ok(s) => {
                        net::set(Some(s));
                        self.session = f.name;
                        self.finder = None;
                        self.lobby(cx);
                    }
                    Err(e) => eprintln!("joining {} at {}: {e}", f.name, f.addr),
                }
            }
            return Go::Stay;
        }
        if clicked("ButHost") {
            if let Some(b) = Self::name_box(cx) {
                if let View::HostJoin { name, .. } = &mut self.view {
                    *name = Some(Box::new(b));
                }
            }
        } else if clicked("ButCancel") {
            return Go::Back;
        }
        Go::Stay
    }

    fn lobby_event(&mut self, cx: &mut Cx, e: Event) -> Go {
        let View::Lobby {
            base,
            page,
            tab,
            sub,
        } = &mut self.view
        else {
            return Go::Stay;
        };
        if let Some(s) = sub {
            match s.event(cx, e) {
                Go::Back => {
                    *sub = None;
                    let tab = *tab;
                    self.page(cx, tab);
                }
                Go::Open(o) => *sub = Some(o),
                Go::Quit => return Go::Quit,
                _ => {}
            }
            return Go::Stay;
        }
        let tab = *tab;
        match e {
            Event::Clicked(n) => {
                let lower = n.to_ascii_lowercase();
                if let Some(t) = TABS.iter().position(|t| t.0.eq_ignore_ascii_case(&n)) {
                    if tab == 3 {
                        Self::read_options(&mut self.game, page);
                    }
                    self.page(cx, t);
                    return Go::Stay;
                }
                match lower.as_str() {
                    "back" => {
                        net::set(None);
                        return Go::Back;
                    }
                    "start" => {
                        if tab == 3 {
                            Self::read_options(&mut self.game, page);
                        }
                        return self.start(cx);
                    }
                    "butrdyuser" => {
                        self.ready = !self.ready;
                        base.set_color("ButRdyUser", if self.ready { READY } else { NOT_READY });
                        let r = self.ready;
                        net::with(|s| s.say(Msg::Ready(r)));
                        if self.ready {
                            let line = cx.ui.lang(5331).replace("%s", &Self::player(cx));
                            self.say(line);
                        }
                    }
                    "butchatsend" => self.send(cx),
                    "editchat" => base.edit(Some("EditChat")),
                    "options" | "joystick" => {
                        let at = if lower == "joystick" { 3 } else { 0 };
                        if let Some(o) = Options::open(cx, at) {
                            *sub = Some(Box::new(o));
                        }
                    }
                    "editplatenumber" if tab == 1 => page.edit(Some("EditPlateNumber")),
                    _ if tab == 0 => {
                        let step = |page: &mut Dlg, list: &str, by: i32| {
                            let count = page.get(list).map_or(0, |c| c.items.len()) as i32;
                            let at = page.selected(list) as i32 + by;
                            let at = if list == "DDLTextBox" {
                                at.rem_euclid(count.max(1))
                            } else {
                                at.clamp(0, (count - 1).max(0))
                            };
                            page.select(list, at as usize);
                        };
                        match lower.as_str() {
                            "lapsleftarrow" => step(page, "LapsListBox", -1),
                            "laprightarrow" => step(page, "LapsListBox", 1),
                            "opponentsleftarrow" => step(page, "OpponentsListBox", -1),
                            "opponentsrightarrow" => step(page, "OpponentsListBox", 1),
                            "racesleftarrow" => step(page, "RacesListBox", -1),
                            "racesrightarrow" => step(page, "RacesListBox", 1),
                            "trackleft" => step(page, "DDLTextBox", -1),
                            "trackright" => step(page, "DDLTextBox", 1),
                            _ => return Go::Stay,
                        }
                        Self::read_event(&mut self.game, page);
                    }
                    _ if tab == 1 => {
                        let (list, by) = match lower.as_str() {
                            "bikeleft" => ("DDLBikes", -1),
                            "bikeright" => ("DDLBikes", 1),
                            "riderleft" => ("DDLRiders", -1),
                            "riderright" => ("DDLRiders", 1),
                            _ => return Go::Stay,
                        };
                        let count = page.get(list).map_or(0, |c| c.items.len()).max(1) as i32;
                        let at = (page.selected(list) as i32 + by).rem_euclid(count) as usize;
                        page.select(list, at);
                        self.game.bike = page.selected("DDLBikes");
                        self.game.outfit = page.selected("DDLRiders");
                        self.page(cx, 1);
                        self.send_ride(cx);
                    }
                    _ => {}
                }
            }
            Event::Selected(n, line) => match n.to_ascii_lowercase().as_str() {
                "eventtypedropdown" => {
                    let event = EVENTS[line.min(EVENTS.len() - 1)].0;
                    if event != self.game.event {
                        self.game.event = event;
                        self.game.track = 0;
                    }
                    self.page(cx, 0);
                }
                "racemodedropdown" => {
                    self.game.mode = line as u32;
                    self.page(cx, 0);
                }
                "ddlbikes" => {
                    self.game.bike = line;
                    self.page(cx, 1);
                    self.send_ride(cx);
                }
                "ddlriders" => {
                    self.game.outfit = line;
                    self.send_ride(cx);
                }
                "largestopponentdropdown" => self.game.largest = line,
                _ if tab == 0 => Self::read_event(&mut self.game, page),
                _ => {}
            },
            Event::Released(n) if n.eq_ignore_ascii_case("EditChat") => self.send(cx),
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

impl Screen for Multi {
    fn layers(&mut self) -> Vec<&mut Dlg> {
        match &mut self.view {
            View::HostJoin {
                menu,
                kind: Some(k),
                ..
            } => vec![menu, k],
            View::HostJoin {
                menu, dlg, name, ..
            } => match name {
                Some(n) => vec![menu, dlg, n],
                None => vec![menu, dlg],
            },
            View::Lobby { sub: Some(s), .. } => s.layers(),
            View::Lobby { base, page, .. } => vec![base, page],
        }
    }

    fn modal(&self) -> bool {
        match &self.view {
            View::HostJoin { .. } => true,
            View::Lobby { sub: Some(s), .. } => s.modal(),
            View::Lobby { .. } => false,
        }
    }

    fn event(&mut self, cx: &mut Cx, e: Event) -> Go {
        match self.view {
            View::HostJoin { .. } => self.host_join_event(cx, e),
            View::Lobby { .. } => self.lobby_event(cx, e),
        }
    }

    fn escape(&mut self, cx: &mut Cx) -> Go {
        match &mut self.view {
            View::HostJoin { name: Some(_), .. } => {
                if let View::HostJoin { name, address, .. } = &mut self.view {
                    *name = None;
                    *address = false;
                }
                Go::Stay
            }
            View::HostJoin { .. } => Go::Back,
            View::Lobby {
                sub: Some(s), tab, ..
            } => {
                if let Go::Back = s.escape(cx) {
                    let tab = *tab;
                    if let View::Lobby { sub, .. } = &mut self.view {
                        *sub = None;
                    }
                    self.page(cx, tab);
                }
                Go::Stay
            }
            View::Lobby { base, page, .. } if base.list_open() || page.list_open() => {
                base.close_list();
                page.close_list();
                Go::Stay
            }
            View::Lobby { .. } => {
                net::set(None);
                Go::Back
            }
        }
    }

    fn type_char(&mut self, cx: &mut Cx, c: char) -> Go {
        // The chat box or the page's edit box, whichever has the keyboard.
        let events = match &mut self.view {
            View::HostJoin { name: Some(n), .. } => n.type_char(c),
            View::Lobby { sub: Some(s), .. } => return s.type_char(cx, c),
            View::Lobby { base, page, .. } => {
                let mut ev = page.type_char(c);
                ev.extend(base.type_char(c));
                ev
            }
            _ => Vec::new(),
        };
        for e in events {
            let go = self.event(cx, e);
            if !matches!(go, Go::Stay) {
                return go;
            }
        }
        Go::Stay
    }

    fn key(&mut self, cx: &mut Cx, code: u32) -> Go {
        match &mut self.view {
            View::Lobby { sub: Some(s), .. } => s.key(cx, code),
            _ => Go::Stay,
        }
    }

    fn tick(&mut self, cx: &mut Cx) -> Go {
        if let (View::HostJoin { dlg, .. }, Some(f)) = (&mut self.view, &mut self.finder) {
            if f.tick() {
                let names = f.found.iter().map(|s| s.name.clone()).collect();
                dlg.set_items("LstSessions", names, self.picked.unwrap_or(0));
                dlg.set_selectable("LstSessions", true);
            }
            return Go::Stay;
        }
        if let View::Lobby { sub: Some(s), .. } = &mut self.view {
            let go = s.tick(cx);
            if !matches!(go, Go::Stay) {
                return go;
            }
        }
        self.hear(cx)
    }

    fn race_over(&mut self, cx: &mut Cx, results: Option<&RaceResults>) {
        if let Some(r) = results {
            self.results = r.clone();
        }
        // Everyone readies again for the next race.
        self.ready = false;
        net::with(|s| s.say(Msg::Ready(false)));
        self.lobby(cx);
        self.page(cx, 2);
    }

    fn preview(&self) -> Option<(Ride, bool, [f32; 4])> {
        match &self.view {
            View::Lobby {
                tab: 1, sub: None, ..
            } => Some((self.ride(101)?, true, WINDOW)),
            _ => None,
        }
    }

    fn draw(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32) {
        if let View::Lobby { sub: Some(s), .. } = &mut self.view {
            return s.draw(ui, gfx, now);
        }
        let track = match &self.view {
            View::Lobby { tab: 0, .. } => {
                let (event, at) = (self.game.event, self.game.track);
                self.tracks(ui, event).get(at).cloned()
            }
            _ => None,
        };
        for d in self.layers() {
            d.tick(now);
            d.draw_under(ui, gfx, now);
        }
        if let (View::Lobby { page, .. }, Some(t)) = (&self.view, track) {
            if let Some(r) = page.rect_of("PictureBox") {
                let key = format!("{}#{}", t.path.display(), t.preview);
                if let Some(tex) = ui.picture(gfx, &key, || preview_image(&t)) {
                    let (w, h) = gfx.sprites.size(tex);
                    let s = (r[2] / w as f32).max(r[3] / h as f32);
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
        for d in self.layers() {
            d.draw_list(ui, gfx);
        }
    }
}
