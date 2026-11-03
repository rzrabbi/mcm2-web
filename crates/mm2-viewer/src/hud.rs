//! The in-race display as the game's `TrackOverlay.cpp` draws it:
//! the speedometer at the bottom right, the race panel at the top left and
//! the radar at the top right, all in the 640×480 screen's units.

use crate::race::Race;
use crate::ui::{Gfx, Ui};
use glam::Vec3;

/// ft/s to mph.
const MPH: f32 = 0.68182;

/// Where `instr1.tga` goes (`0x51ddf0`: the rect 511,351 to 639,479), and
/// its needle's centre and length.
const SPEEDO: (f32, f32) = (511.0, 351.0);
const NEEDLE_CENTRE: (f32, f32) = (64.0, 85.0);
const NEEDLE: f32 = 45.0;

/// `BajaScore.tga` at the top left (`0x51eb30`), and its lines' top left
/// corners in the panel.
const PANEL: (f32, f32) = (0.0, 0.0);
const LINES: [(f32, f32); 6] = [
    (14.0, 20.0),
    (14.0, 36.0),
    (14.0, 53.0),
    (14.0, 69.0),
    (14.0, 87.0),
    (14.0, 105.0),
];
/// The panel's text height (the game's 12 pixel font).
const TEXT: f32 = 12.0;

/// `radar128.tga` at the top right, its centre and rim (`0x520e90`).
const RADAR: (f32, f32) = (512.0, 1.0);
const RADAR_CENTRE: (f32, f32) = (572.0, 65.0);
const RADAR_RIM: f32 = 55.0;
/// The rim is 180 ft × zoom away; the game starts at 2.
const RADAR_FEET: f32 = 180.0;
/// The course line on the radar (0xedea5e) and the gate to make next
/// (0xffff5e).
const RADAR_COURSE: [f32; 4] = [0xed as f32 / 255.0, 0xea as f32 / 255.0, 0x5e as f32 / 255.0, 1.0];
const RADAR_GATE: [f32; 4] = [1.0, 1.0, 0x5e as f32 / 255.0, 1.0];

/// The part of the stretch `a`–`b` (radar pixels from the centre) inside
/// the rim (0x521b40), if any: both ends outside draws nothing.
fn clip_to_rim(a: [f32; 2], b: [f32; 2]) -> Option<([f32; 2], [f32; 2])> {
    let inside = |p: [f32; 2]| p[0] * p[0] + p[1] * p[1] < RADAR_RIM * RADAR_RIM;
    let (ia, ib) = (inside(a), inside(b));
    if ia && ib {
        return Some((a, b));
    }
    if !ia && !ib {
        return None;
    }
    let (from, to) = if ia { (a, b) } else { (b, a) };
    // from + t (to − from) on the rim, t in 0..1.
    let d = [to[0] - from[0], to[1] - from[1]];
    let (qa, qb) = (d[0] * d[0] + d[1] * d[1], 2.0 * (from[0] * d[0] + from[1] * d[1]));
    let qc = from[0] * from[0] + from[1] * from[1] - RADAR_RIM * RADAR_RIM;
    let t = (-qb + (qb * qb - 4.0 * qa * qc).max(0.0).sqrt()) / (2.0 * qa);
    let edge = [from[0] + d[0] * t, from[1] + d[1] * t];
    Some(if ia { (from, edge) } else { (edge, from) })
}

/// The text queue at the bottom left (`0x520970`, `0x520aa0`): its box,
/// text height, and how long each message stays (`0x497740`).
const MESSAGE: [f32; 4] = [10.0, 460.0, 246.0, 20.0];
const MESSAGE_TEXT: f32 = 17.0;
const MESSAGE_TIME: f32 = 3.25;
/// The riders' labels' text height.
const LABEL_TEXT: f32 = 14.0;
/// A label hangs this far over its rider's head (0x55d9fc), and fades in
/// or out over this long when a hill comes between it and the camera
/// (0x51e400's +0x144).
const LABEL_OVER_HEAD: f32 = 1.62;
const LABEL_FADE: f32 = 0.25;

/// A rider's label's fading (0x51e5b0): whether the camera saw it at the
/// last look, whether it is fading in or out, and how far through, s.
#[derive(Debug, Clone, Copy, Default)]
struct Label {
    hidden: bool,
    fading_in: bool,
    fading_out: bool,
    t: f32,
    alpha: f32,
}

impl Label {
    fn new() -> Self {
        Label {
            alpha: 1.0,
            ..Default::default()
        }
    }

    /// 0x51e760: after a look, a label the camera has just come to see
    /// fades in, and one it has lost fades out.
    fn saw(&mut self, seen: bool) {
        if seen && self.hidden {
            (self.fading_in, self.fading_out) = (true, false);
        } else if !seen && !self.hidden {
            (self.fading_in, self.fading_out) = (false, true);
        }
        self.hidden = !seen;
    }

    /// 0x51e5b0, each frame.
    fn step(&mut self, dt: f32) {
        if self.fading_in {
            self.t += dt;
            if self.t >= LABEL_FADE {
                (self.alpha, self.fading_in) = (1.0, false);
            }
        } else if self.fading_out {
            self.t -= dt;
            if self.t <= 0.0 {
                (self.t, self.alpha, self.fading_out) = (0.0, 0.0, false);
            }
        }
        if self.fading_in || self.fading_out {
            self.alpha = ((self.t / LABEL_FADE * 255.0 - 0.5).round() / 255.0).clamp(0.0, 1.0);
        }
    }
}

/// What the race shows: the Game and Graphics pages' switches, which the
/// F5 to F10 keys and Ctrl+N flip during a race (0x4e54c0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Show {
    /// The speedometer (Instruments, F7).
    pub gauges: bool,
    /// The race panel (Stats, F5).
    pub stats: bool,
    /// The radar (Radar Overlay, F6).
    pub radar: bool,
    /// Labels over the other riders: their names, their places, or both.
    pub names: bool,
    pub positions: bool,
    /// The sky (F8), the dirt (F9) and the shadows (F10).
    pub sky: bool,
    pub particles: bool,
    pub shadows: bool,
}

impl Default for Show {
    fn default() -> Self {
        Show::from_options(&Default::default(), &Default::default())
    }
}

impl Show {
    pub fn from_profile(p: &crate::profile::Profile) -> Self {
        Show::from_options(&p.game, &p.graphics)
    }

    fn from_options(g: &crate::profile::GameOptions, v: &crate::profile::GraphicsOptions) -> Self {
        Show {
            gauges: g.gauges,
            stats: g.race_stats,
            radar: g.overview,
            names: g.rider_names,
            positions: g.rider_position,
            sky: v.sky,
            particles: v.particles,
            shadows: v.shadows,
        }
    }

    /// Back into the profile, which keeps them.
    pub fn store(&self, p: &mut crate::profile::Profile) {
        let (g, v) = (&mut p.game, &mut p.graphics);
        g.gauges = self.gauges;
        g.race_stats = self.stats;
        g.overview = self.radar;
        g.rider_names = self.names;
        g.rider_position = self.positions;
        v.sky = self.sky;
        v.particles = self.particles;
        v.shadows = self.shadows;
    }

    /// Ctrl+N: both, names, places, neither, and round again; the string
    /// the game says and whether it is on.
    pub fn cycle_labels(&mut self) -> (u32, bool) {
        match (self.names, self.positions) {
            (true, true) => {
                self.positions = false;
                (5138, true)
            }
            (true, false) => {
                (self.names, self.positions) = (false, true);
                (5139, true)
            }
            (false, false) => {
                (self.names, self.positions) = (true, true);
                (5140, true)
            }
            (false, true) => {
                self.positions = false;
                (5140, false)
            }
        }
    }
}

pub struct Hud {
    pub show: Show,
    /// The camera's projection and view, for the riders' labels.
    pub view_proj: glam::Mat4,
    /// Each rider's head, game coordinates, and each label's fading.
    pub heads: Vec<Option<Vec3>>,
    labels: Vec<Label>,
    /// Frames counted for the labels' looks, every fourth frame each.
    looks: usize,
    /// The speed the dial shows, mph; in the air it winds down.
    shown: f32,
    pub zoom: f32,
    /// The message showing and how long it has been up.
    message: Option<(String, f32)>,
    /// The player's finish has been announced.
    told: bool,
}

impl Default for Hud {
    fn default() -> Self {
        Hud {
            show: Show::default(),
            view_proj: glam::Mat4::IDENTITY,
            heads: Vec::new(),
            labels: Vec::new(),
            looks: 0,
            shown: 0.0,
            zoom: 2.0,
            message: None,
            told: false,
        }
    }
}

impl Hud {
    /// Shows a message in place of the one showing (`0x520b90`).
    pub fn say(&mut self, text: &str) {
        if !text.is_empty() {
            self.message = Some((text.to_string(), 0.0));
        }
    }

    /// Each rider's label looks for the camera from over the head, every
    /// fourth frame, the riders taking turns (0x51e680, 0x51e4b0's +0x130):
    /// hidden when the ground lies between them (0x50c600). `eye` is the
    /// camera, game coordinates.
    pub fn look(&mut self, terrain: &mm2_formats::trn::Terrain, eye: Vec3) {
        self.labels.resize(self.heads.len(), Label::new());
        let turn = self.looks % 4;
        self.looks = self.looks.wrapping_add(1);
        for (i, (label, head)) in self.labels.iter_mut().zip(&self.heads).enumerate() {
            let Some(head) = head else {
                continue;
            };
            if i % 4 == turn {
                label.saw(!ground_between(terrain, eye, *head + Vec3::Y * LABEL_OVER_HEAD));
            }
        }
    }

    /// Steps the radar's zoom (1 to 5).
    pub fn zoom_by(&mut self, step: f32) {
        self.zoom = (self.zoom + step).clamp(1.0, 5.0);
    }

    /// "Stats ON" and the like (0x4e5410).
    pub fn say_switch(&mut self, ui: &Ui, what: u32, on: bool) {
        let state = ui.lang(if on { 5127 } else { 5128 });
        self.say(&format!("{} {}", ui.lang(what), state));
    }

    pub fn draw(&mut self, ui: &mut Ui, gfx: &mut Gfx, race: &Race, dt: f32) {
        let me = &race.racers[race.player];
        self.draw_labels(ui, gfx, race, dt);
        if self.show.gauges {
            self.draw_gauges(ui, gfx, race, dt);
        }
        if self.show.stats {
            self.draw_panel(ui, gfx, race);
        }
        // The finish (`0x497740`): "You finished 2nd", or the
        // congratulations for a win.
        if me.finished.is_some() && !self.told {
            self.told = true;
            let place = race.position().clamp(1, 12) as u32;
            self.say(&ui.lang(5248 + place));
        }
        self.draw_message(ui, gfx, dt);
        if self.show.radar {
            self.draw_radar(ui, gfx, race);
        }
    }

    /// Each other rider's label (0x523a40): its place, its name, or
    /// "place : name". 0x51e760 draws it at its size on the screen, its
    /// top left corner 1.62 ft over the rider's head, while that point is
    /// in view.
    fn draw_labels(&mut self, ui: &mut Ui, gfx: &mut Gfx, race: &Race, dt: f32) {
        for label in &mut self.labels {
            label.step(dt);
        }
        if !self.show.names && !self.show.positions {
            return;
        }
        let order = race.standings();
        for (i, r) in race.racers.iter().enumerate() {
            if i == race.player || Some(i) == race.ghost {
                continue;
            }
            let alpha = self.labels.get(i).map_or(1.0, |l| l.alpha);
            if alpha <= 0.0 {
                continue;
            }
            let head = self.heads.get(i).copied().flatten().unwrap_or(r.bike.position() + Vec3::Y * 4.5);
            let at = crate::renderer::from_game(head + Vec3::Y * LABEL_OVER_HEAD);
            let clip = self.view_proj * at.extend(1.0);
            if clip.w < 1.0 {
                continue;
            }
            let (x, y) = (clip.x / clip.w, clip.y / clip.w);
            if x.abs() > 1.0 || y.abs() > 1.0 || clip.z > clip.w {
                continue;
            }
            let place = order.iter().position(|&o| o == i).map_or(0, |p| p + 1);
            let place = if place < 1 { "--".to_string() } else { place.to_string() };
            let name = crate::rider_name(race, ui, i);
            let text = match (self.show.positions, self.show.names) {
                (true, true) => format!("{place} : {name}"),
                (true, false) => place,
                _ => name,
            };
            let Some(t) = ui.cell_text(gfx, &text, LABEL_TEXT, true) else {
                continue;
            };
            let (w, h) = gfx.sprites.size(t);
            let (w, h) = (w as f32, h as f32);
            let (sx, sy) = (((x + 1.0) * 320.0).trunc(), ((1.0 - y) * 240.0).trunc());
            gfx.sprites.in_view = true;
            for (d, colour) in [(1.0, [0.0, 0.0, 0.0, alpha]), (0.0, [1.0, 1.0, 1.0, alpha])] {
                gfx.sprites.quad(t, [0.0, 0.0, 1.0, 1.0], [sx + d, sy + d, w, h], colour);
            }
            gfx.sprites.in_view = false;
        }
    }

    fn draw_gauges(&mut self, ui: &mut Ui, gfx: &mut Gfx, race: &Race, dt: f32) {
        let bike = &race.racers[race.player].bike;

        // Speedometer.
        if bike.airborne() {
            // 0.5 mph a frame at the game's 30 frames a second.
            self.shown = (self.shown - 0.5 * dt * 30.0).max(0.0);
        } else {
            self.shown = bike.speed().abs() * MPH;
        }
        if let Some(t) = ui.overlay(gfx, "instr1.tga") {
            gfx.sprites.image(t, SPEEDO.0, SPEEDO.1);
        }
        let v = self.shown.clamp(0.0, 100.0);
        let angle = if v < 50.0 {
            230.0 + 130.0 * v / 50.0
        } else {
            130.0 * (v - 50.0) / 50.0
        }
        .to_radians();
        let c = [SPEEDO.0 + NEEDLE_CENTRE.0, SPEEDO.1 + NEEDLE_CENTRE.1];
        let tip = [c[0] + angle.sin() * NEEDLE, c[1] - angle.cos() * NEEDLE];
        ui.line(gfx, c, tip, 1.5, [1.0, 0.0, 0.0, 1.0]);
    }

    fn draw_panel(&mut self, ui: &mut Ui, gfx: &mut Gfx, race: &Race) {
        let me = &race.racers[race.player];
        // Race panel.
        if let Some(t) = ui.overlay(gfx, "BajaScore.tga") {
            gfx.sprites.image(t, PANEL.0, PANEL.1);
        }
        let clock = race.clock();
        let started = race.gate_down();
        let unset = "-:--.--".to_string();
        let lap = if race.laps >= 99 {
            format!("{} {}", ui.lang(2102), me.laps + 1)
        } else {
            format!(
                "{} {} / {}",
                ui.lang(2102),
                (me.laps + 1).min(race.laps),
                race.laps
            )
        };
        let mut lines = [
            format!(
                "{} {} / {}",
                ui.lang(2101),
                race.position(),
                race.racers.len() - race.ghost.is_some() as usize
            ),
            lap,
            format!(
                "{} {}",
                ui.lang(2103),
                if started {
                    clock_text(me.lap_time(clock))
                } else {
                    unset.clone()
                }
            ),
            format!(
                "{} {}",
                ui.lang(2104),
                me.best_lap().map_or(unset.clone(), clock_text)
            ),
            // Time behind the leader; the leader's is blank.
            format!(
                "{} {}",
                ui.lang(2105),
                if me.timing.behind_leader > 0.0 {
                    clock_text(me.timing.behind_leader)
                } else {
                    unset.clone()
                }
            ),
            format!(
                "{} {}",
                ui.lang(5048),
                if started {
                    clock_text(me.finished.unwrap_or(clock))
                } else {
                    unset.clone()
                }
            ),
        ];
        // The Stunt Quarry's panel (0x51f070): the time left, unless the
        // session has no end, then the riders by points, `%d) %s : %.0f`.
        if race.stunt_event {
            let order = race.standings();
            let names: Vec<String> = (0..race.racers.len())
                .map(|i| crate::rider_name(race, ui, i))
                .collect();
            let mut stunt: Vec<String> = Vec::new();
            if let Some(left) = race.time_left() {
                let s = left as u32;
                stunt.push(format!("{}{:02}:{:02}", ui.lang(2371), s / 60, s % 60));
            }
            let room = lines.len() - stunt.len();
            let me = order.iter().position(|&i| i == race.player).unwrap_or(0);
            for (place, &i) in order.iter().enumerate().take(room) {
                // The player keeps the last line when further down.
                let (place, i) = if place + 1 == room && me >= room {
                    (me, race.player)
                } else {
                    (place, i)
                };
                let name = names.get(i).cloned().unwrap_or_default();
                stunt.push(format!(
                    "{}) {name} : {:.0}",
                    place + 1,
                    race.racers[i].stunt_points
                ));
            }
            stunt.resize(lines.len(), String::new());
            lines = std::array::from_fn(|k| stunt[k].clone());
        }
        // Moto-Tag's panel (0x520090): the time left, the riders by points
        // in stunt mode (otherwise the player's place and times as IT), and
        // who is IT, or a loose ball.
        if let Some(t) = &race.tag {
            let mut tag: Vec<String> = Vec::new();
            let s = race.time_left().unwrap_or(0.0) as u32;
            tag.push(format!("{}{:02}:{:02}", ui.lang(2371), s / 60, s % 60));
            if t.stunts {
                let order = race.standings();
                for (place, &i) in order.iter().enumerate().take(4) {
                    tag.push(format!(
                        "{}) {} : {:.0}",
                        place + 1,
                        crate::rider_name(race, ui, i),
                        race.racers[i].stunt_points
                    ));
                }
            } else {
                tag.push(format!(
                    "{} {} / {}",
                    ui.lang(2101),
                    race.position(),
                    race.racers.len()
                ));
                tag.push(format!(
                    "{} {}",
                    ui.lang(2361),
                    clock_text(t.time_it(race.player))
                ));
                tag.push(format!(
                    "{} {}",
                    ui.lang(5163),
                    clock_text(t.longest_it(race.player))
                ));
            }
            tag.resize(lines.len() - 1, String::new());
            tag.push(match t.it {
                Some(i) => format!("{} {}", crate::rider_name(race, ui, i), ui.lang(5304)),
                None if t.ball.is_some() => ui.lang(5303),
                None => String::new(),
            });
            lines = std::array::from_fn(|k| tag[k].clone());
        }
        // A gate race's panel (0x51fbb0) ends on the gate being ridden for
        // instead of the race time.
        if !race.gates.is_empty() && race.tag.is_none() {
            let count = race.gates.len();
            let gate = if me.finished.is_some() {
                count
            } else {
                me.timing.gate + 1
            };
            lines[5] = format!("{} {gate} / {count}", ui.lang(5037));
        }
        for (text, (x, y)) in lines.iter().zip(LINES) {
            if let Some(t) = ui.cell_text(gfx, text, TEXT, false) {
                let (w, h) = gfx.sprites.size(t);
                gfx.sprites.quad(
                    t,
                    [0.0, 0.0, 1.0, 1.0],
                    [PANEL.0 + x, PANEL.1 + y - 1.0, w as f32, h as f32],
                    [1.0; 4],
                );
            }
        }

    }

    fn draw_radar(&mut self, ui: &mut Ui, gfx: &mut Gfx, race: &Race) {
        let bike = &race.racers[race.player].bike;
        // Radar, heading up.
        if let Some(t) = ui.overlay(gfx, "radar128.tga") {
            gfx.sprites.image(t, RADAR.0, RADAR.1);
        }
        let at = bike.position();
        let forward = bike.forward();
        let right = Vec3::new(forward.z, 0.0, -forward.x);
        let scale = RADAR_RIM / (RADAR_FEET * self.zoom);
        for (i, r) in race.racers.iter().enumerate() {
            if i == race.player {
                continue;
            }
            let rel = r.bike.position() - at;
            let (mut x, mut y) = (rel.dot(right) * scale, -rel.dot(forward) * scale);
            let d = (x * x + y * y).sqrt();
            let colour = if d > RADAR_RIM {
                (x, y) = (x / d * RADAR_RIM, y / d * RADAR_RIM);
                [
                    0x28 as f32 / 255.0,
                    0xaa as f32 / 255.0,
                    0x28 as f32 / 255.0,
                    1.0,
                ]
            } else {
                [
                    0x50 as f32 / 255.0,
                    0xe6 as f32 / 255.0,
                    0x50 as f32 / 255.0,
                    1.0,
                ]
            };
            // A 3 pixel dash, two rows high.
            let (cx, cy) = (RADAR_CENTRE.0 + x, RADAR_CENTRE.1 + y);
            ui.fill(gfx, [cx - 1.5, cy - 1.0, 3.0, 2.0], colour);
        }
        // Whole pixels from the centre (0x5219b0).
        let spot = |p: Vec3| {
            let rel = p - at;
            [(rel.dot(right) * scale).trunc(), (-rel.dot(forward) * scale).trunc()]
        };
        // The original's lines are one screen pixel wide at any size.
        let width = 1.0 / gfx.sprites.scale.max(1.0);
        let line = |ui: &mut Ui, gfx: &mut Gfx, a: [f32; 2], b: [f32; 2], colour: [f32; 4]| {
            if let Some((a, b)) = clip_to_rim(a, b) {
                let at = |p: [f32; 2]| [RADAR_CENTRE.0 + p[0], RADAR_CENTRE.1 + p[1]];
                ui.line(gfx, at(a), at(b), width, colour);
            }
        };
        if !race.gates.is_empty() {
            // Baja and Enduro (0x522170): the gate the player heads for in
            // yellow and the one after it in white, each post to post, when
            // its first post is on the radar.
            let next = race.racers[race.player].timing.gate;
            for (k, colour) in [(0, RADAR_GATE), (1, [1.0; 4])] {
                let g = &race.gates[(next + k) % race.gates.len()];
                let (a, b) = (spot(g.centre - g.half_width), spot(g.centre + g.half_width));
                if (a[0] * a[0] + a[1] * a[1]).sqrt() < RADAR_RIM {
                    line(ui, gfx, a, b, colour);
                }
            }
        } else if let (Some(course), false) = (&race.course, race.stunt_event || race.tag.is_some()) {
            // Nationals and Supercross (0x521d70): both sides of the
            // course, cut at the rim.
            for (a, b) in course.side_segments() {
                line(ui, gfx, spot(a), spot(b), RADAR_COURSE);
            }
        }
    }
}

impl Hud {
    /// Green text with a black shadow, held for its time.
    fn draw_message(&mut self, ui: &mut Ui, gfx: &mut Gfx, dt: f32) {
        let Some((text, age)) = &mut self.message else {
            return;
        };
        *age += dt;
        if *age > MESSAGE_TIME {
            self.message = None;
            return;
        }
        let Some(t) = ui.cell_text(gfx, text, MESSAGE_TEXT, true) else {
            return;
        };
        let (w, h) = gfx.sprites.size(t);
        let (w, h) = (w as f32, h as f32);
        let y = (MESSAGE[1] + (MESSAGE[3] - h) / 2.0).round();
        for (dx, colour) in [(0.0, [0.0, 0.0, 0.0, 1.0]), (-1.0, [0.0, 1.0, 0.0, 1.0])] {
            gfx.sprites.quad(
                t,
                [0.0, 0.0, 1.0, 1.0],
                [MESSAGE[0] + dx, y + dx, w, h],
                colour,
            );
        }
    }
}

/// The panel's times: `%2d:%05.2f`.
pub fn clock_text(t: f32) -> String {
    let m = (t / 60.0).floor();
    format!("{:2}:{:05.2}", m as u32, t - m * 60.0)
}

/// Whether the ground rises between `from` and `to` (0x50c600's test along
/// the line), checked every 2 ft.
fn ground_between(terrain: &mm2_formats::trn::Terrain, from: Vec3, to: Vec3) -> bool {
    let steps = ((to - from).length() / 2.0).ceil().clamp(1.0, 1000.0) as usize;
    (1..steps).any(|k| {
        let p = from.lerp(to, k as f32 / steps as f32);
        crate::ride::ground(terrain, p.x, p.z) > p.y
    })
}
