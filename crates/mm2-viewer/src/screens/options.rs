//! Options (`OptBase.dtm`, `0x4b8880`): a tabbed screen whose tabs are
//! Game, Graphics, Sound, Controls, Messages and Garage. Also here are
//! Advanced Graphics (`OptAdvG.dtm`, `0x4b6dd0`) and the box that puts the
//! default settings back (`GlobSet.dtm`, `0x4b9470`).
//!
//! As in the original the pages edit a copy of the profile's settings,
//! taken when the screen opens and given back (and saved) when Back
//! closes it; each page shows the copy when it opens and writes its
//! controls back into it when another page replaces it.

use super::keys;
use super::{popup, Cx, Go, MsgBox, Screen};
use crate::audio::Voice;
use crate::profile::{Controls, Profile, ACTIONS};
use crate::ui::{Dlg, Event, Gfx, Ui};
use mm2_formats::rs2;
use std::path::Path;

/// The tabs, their text in `LANG.DLL`, and their pages.
const PAGES: [(&str, u32, &str); 6] = [
    ("GameSettings", 5085, "OPTGAME"),
    ("Graphics", 5086, "OPTGRAPH"),
    ("Sound", 5087, "OPTSOUND"),
    ("Controls", 5088, "OPTCONT"),
    ("Messages", 5089, "OPTMESS"),
    ("Garage", 5090, "OPTGAR"),
];

/// Where the pages go (`0x4b90b0`).
const PAGE_AT: (f32, f32) = (20.0, 35.0);

/// The engine sizes, one class each (`0x572bec`).
const ENGINE_SIZES: [u32; 5] = [125, 250, 400, 500, 600];

/// The display modes the graphics page offers: the 16-bit modes a card
/// of the time listed.
const MODES: [(u32, u32); 7] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1152, 864),
    (1280, 960),
    (1280, 1024),
    (1600, 1200),
];

/// One engine class's power bands, from `UI/UIRES.RES` `presets.pb`
/// (`0x49b460`).
#[derive(Debug, Clone, Default)]
pub struct EngineClass {
    pub rpm_low: i32,
    pub rpm_high: i32,
    pub total: i32,
    pub least: i32,
    /// The highest band limit: the top of the sliders.
    pub most: i32,
    pub limits: [i32; 11],
    pub stock: [[i32; 11]; 3],
}

impl EngineClass {
    fn step(&self) -> i32 {
        (self.rpm_high - self.rpm_low) / 10
    }
}

/// The five classes, or the game's fallback when the file is missing.
pub fn engine_classes(game: &Path) -> [EngineClass; 5] {
    let text = crate::read_archive(&game.join("UI").join("UIRES.RES"))
        .ok()
        .and_then(|b| {
            let ar = rs2::Archive::parse(&b).ok()?;
            let e = ar
                .entries()
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case("presets.pb"))?;
            Some(String::from_utf8_lossy(ar.data(e)).into_owned())
        });
    let Some(text) = text else {
        return std::array::from_fn(|_| EngineClass {
            rpm_low: 5000,
            rpm_high: 10000,
            total: 100,
            least: 0,
            most: 40,
            limits: [40; 11],
            stock: [[10; 11]; 3],
        });
    };
    let ini = mm2_formats::scn::Ini::parse(&text);
    std::array::from_fn(|c| {
        let num = |sec: &str, key: &str, d: i32| {
            ini.section(sec)
                .and_then(|s| s.get(key))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(d)
        };
        let sec = format!("Category_{}", c + 1);
        let low = num(&sec, "RPMLowerLimit", 5000);
        let high = num(&sec, "RPMUpperLimit", 10000);
        let step = (high - low) / 10;
        let band = |sec: &str, b: usize| num(sec, &format!("RPM{:05}", low + b as i32 * step), 10);
        let limits: [i32; 11] = std::array::from_fn(|b| band(&sec, b));
        EngineClass {
            rpm_low: low,
            rpm_high: high,
            total: num(&sec, "HPSum", 400),
            least: num(&sec, "MinRange", 10),
            most: limits.iter().copied().max().unwrap_or(0),
            limits,
            stock: std::array::from_fn(|n| {
                let sec = format!("Category_{}Preset_{}", c + 1, n + 1);
                std::array::from_fn(|b| band(&sec, b))
            }),
        }
    })
}

/// The class of an engine size: those between two sizes count with the
/// lower (`0x4b7f70`).
/// The Bike & Rider page's bike under the player (0x450f00): a bike of
/// another class than the garage's puts that class's chosen curve under
/// the bike, with its revs; the same class leaves the curve as it was.
pub fn ride_bike(garage: &mut crate::profile::Garage, classes: &[EngineClass; 5], cc: u32) {
    let (was, now) = (engine_class(garage.cc), engine_class(cc));
    garage.cc = cc;
    if was == now {
        return;
    }
    let class = &classes[now];
    garage.ride_rpm = [class.rpm_low, class.rpm_high, class.step()];
    let curve = garage.curve[now];
    garage.ride_power = if curve < 3 {
        class.stock[curve]
    } else {
        garage.custom[now][curve - 3]
    };
}

pub fn engine_class(cc: u32) -> usize {
    if cc <= ENGINE_SIZES[0] {
        return 0;
    }
    if cc >= ENGINE_SIZES[4] {
        return 4;
    }
    (1..4)
        .find(|&i| cc > ENGINE_SIZES[i - 1] && cc < ENGINE_SIZES[i + 1])
        .unwrap_or(1)
}

/// The Game page's check boxes and what they set.
fn game_checks(p: &mut Profile) -> [(&'static str, &mut bool); 11] {
    let g = &mut p.game;
    [
        ("RiderPositionCheckBox", &mut g.rider_position),
        ("RaceStatsCheckBox", &mut g.race_stats),
        ("GuagesCheckBox", &mut g.gauges),
        ("ChkUIAnims", &mut g.ui_anims),
        ("OverviewCheckBox", &mut g.overview),
        ("RiderNamesCheckBox", &mut g.rider_names),
        ("ChkPodiums", &mut g.podiums),
        ("ChkIntroMovie", &mut g.intro_movie),
        ("ChkPlayerIndicator", &mut g.player_indicator),
        ("ChkToolTips", &mut g.tooltips),
        ("ChkWreckResetOnTrack", &mut g.wreck_reset),
    ]
}

fn sound_checks(p: &mut Profile) -> [(&'static str, &mut bool); 5] {
    let s = &mut p.sound;
    [
        ("InGameSoundEffectsCheck", &mut s.effects),
        ("SoundHardwareCheckBox", &mut s.hardware),
        ("UserInterfaceSoundsCheckBox", &mut s.ui_sounds),
        ("ChkEax", &mut s.eax),
        ("ChkWaypointChime", &mut s.waypoint_chime),
    ]
}

fn sound_sliders(p: &mut Profile) -> [(&'static str, &mut u32); 2] {
    let s = &mut p.sound;
    [
        ("UserInterfaceVolumeSlider", &mut s.ui_volume),
        ("InGameVolumeSlider", &mut s.game_volume),
    ]
}

fn graphics_checks(p: &mut Profile) -> [(&'static str, &mut bool); 3] {
    let g = &mut p.graphics;
    [
        ("ShadowsCheckBox", &mut g.shadows),
        ("ParticlesCheckBox", &mut g.particles),
        ("SkyCheckBox", &mut g.sky),
    ]
}

fn control_sliders(p: &mut Profile) -> [(&'static str, &mut u32); 8] {
    let c = &mut p.controls;
    [
        ("SteeringSlider", &mut c.steering),
        ("CrossupSlider", &mut c.cross_up),
        ("PitchSlider", &mut c.pitch),
        ("PressThrottleSlider", &mut c.press_throttle),
        ("PressBrakesSlider", &mut c.press_brakes),
        ("ReleaseThrottleSlider", &mut c.release_throttle),
        ("ReleaseBrakesSlider", &mut c.release_brakes),
        ("FeedbackSlider", &mut c.feedback),
    ]
}

fn control_checks(p: &mut Profile) -> [(&'static str, &mut bool); 4] {
    let c = &mut p.controls;
    [
        ("ForceFeedCheck", &mut c.force_feedback),
        ("ChkGasGyro", &mut c.gas_gyro),
        ("ChkBrakeGyro", &mut c.brake_gyro),
        ("ChkAnalogAxes", &mut c.analog_axes),
    ]
}

const SUSPENSION: [&str; 6] = [
    "SldFrontComp",
    "SldFrontDecomp",
    "SldRearComp",
    "SldRearDecomp",
    "SldFrontDamp",
    "SldRearDamp",
];

/// The chat box modes, in the order the box steps through them, with
/// their text.
const CHAT_MODES: [u32; 3] = [5131, 5130, 5132];

/// The advanced screen's quality sliders, after the video memory box.
pub(crate) const DETAILS: [&str; 4] = ["SldEcosystem", "SldObjects", "SldTerrain", "SldVisibility"];

#[allow(clippy::large_enum_variant)]
enum Sub {
    Advanced(Dlg),
    Restore { dlg: Dlg, confirm: Option<MsgBox> },
}

pub struct Options {
    base: Dlg,
    page: Option<(usize, Dlg)>,
    sub: Option<Sub>,
    /// The copy the pages edit.
    p: Profile,
    classes: [EngineClass; 5],
    /// The garage page's bands as shown, and the power they use.
    bands: [i32; 11],
    used: i32,
    /// The controls page's action waiting for a key.
    listening: Option<usize>,
    ratchet: Option<Voice>,
    ratchet_at: f32,
}

impl Options {
    /// The screen, on page `page` (0 the game's; the single player screen
    /// opens it on 3 for the controls and 5 for the garage, `0x44df50`).
    pub fn open(cx: &mut Cx, page: usize) -> Option<Options> {
        let mut base = Dlg::open(cx.ui, "OPTBASE", (0.0, 0.0))?;
        for (tab, id, _) in PAGES {
            base.set_text(tab, cx.ui.lang(id));
        }
        let mut o = Options {
            base,
            page: None,
            sub: None,
            p: cx.user.profile.clone(),
            classes: engine_classes(cx.game()),
            bands: [0; 11],
            used: 0,
            listening: None,
            ratchet: None,
            ratchet_at: f32::MIN,
        };
        o.show_page(cx, page.min(5));
        // Without a profile there is nothing to restore (`0x527300`).
        if cx.user.store.list().is_empty() {
            o.base.show("Restore", false);
        }
        Some(o)
    }

    /// Opens page `page` in place of the one showing (`0x4b90b0`).
    fn show_page(&mut self, cx: &mut Cx, page: usize) {
        if self.page.as_ref().is_some_and(|(i, _)| *i == page) {
            return;
        }
        self.close_page(cx);
        for (i, (tab, _, _)) in PAGES.iter().enumerate() {
            self.base.set_on(tab, i == page);
        }
        let Some(dlg) = Dlg::open(cx.ui, PAGES[page].2, PAGE_AT) else {
            return;
        };
        self.page = Some((page, dlg));
        self.fill(cx);
    }

    fn close_page(&mut self, cx: &mut Cx) {
        if self.page.is_some() {
            self.commit(cx);
        }
        self.page = None;
        self.listening = None;
    }

    fn dlg(&mut self) -> Option<&mut Dlg> {
        self.page.as_mut().map(|(_, d)| d)
    }

    /// The copy into the page's controls (the handlers' open, and slot 31
    /// with 0).
    fn fill(&mut self, cx: &mut Cx) {
        let Some((page, d)) = self.page.as_mut() else {
            return;
        };
        let p = &mut self.p;
        match *page {
            0 => {
                for (name, v) in game_checks(p) {
                    d.set_on(name, *v);
                }
                let mode = p.game.chat_mode.min(2) as usize;
                d.set_text("MultChatMode", cx.ui.lang(CHAT_MODES[mode]));
                // With no product ID from Setup the game shows a blank one.
                let pid = cx
                    .user
                    .settings
                    .get("PID")
                    .filter(|p| !p.is_empty())
                    .unwrap_or("00000-000-0000000-00000")
                    .to_string();
                d.set_text("TxtPID", format!("{}: {pid}", cx.ui.lang(5047)));
            }
            1 => {
                let mut modes: Vec<String> = MODES.iter().map(|(w, h)| format!("{w} X {h}")).collect();
                let mut at = modes
                    .iter()
                    .position(|m| *m == p.graphics.resolution)
                    .unwrap_or(0);
                if !crate::platform::DISPLAY_MODES {
                    (modes, at) = (vec!["Auto".into()], 0);
                }
                d.set_items("DisplayResolutionDropDown", modes, at);
                // 0x4b6ac0: white and centred.
                d.set_color("DisplayResolutionDropDown", 0xffffff);
                d.set_align("DisplayResolutionDropDown", 0x12);
                // In the browser the list shows Auto and is disabled, as
                // there is no mode to choose.
                if !crate::platform::DISPLAY_MODES {
                    d.enable("DisplayResolutionDropDown", false);
                }
                // The slider only sets how far the fog lets you see (the
                // Visibility detail): a line under its box says so, in the
                // style of the box's own -/+ labels.
                if crate::platform::VIEW_DISTANCE_NOTE {
                    d.add_text(
                        "rbwLabel6",
                        "ViewDistanceNote",
                        [7, 81, 214, 34],
                        "In this version the slider sets the view distance: how far you can see before the fog.",
                    );
                    d.set_font("ViewDistanceNote", 11.0);
                }
                d.set_slider("TerrainQualitySliderBar", p.graphics.terrain, None);
                for (name, v) in graphics_checks(p) {
                    d.set_on(name, *v);
                }
            }
            2 => {
                for (name, v) in sound_checks(p) {
                    d.set_on(name, *v);
                }
                for (name, v) in sound_sliders(p) {
                    d.set_slider(name, *v, None);
                }
                d.set_on("Rad16Bit", p.sound.sixteen_bit);
                d.set_on("Rad8Bit", !p.sound.sixteen_bit);
            }
            3 => {
                for (name, v) in control_sliders(p) {
                    d.set_slider(name, *v, None);
                }
                for (name, v) in control_checks(p) {
                    d.set_on(name, *v);
                }
                // The analog axes box is hidden (`0x4b74e0`), and so is
                // force feedback without a joystick that has it, as the
                // original shows it with none.
                d.show("ChkAnalogAxes", false);
                for name in [
                    "ForceFeedCheck",
                    "FeedbackSlider",
                    "WeakerText",
                    "StrongerText",
                ] {
                    d.show(name, false);
                }
                let actions = (0..ACTIONS as u32).map(|i| cx.ui.lang(402 + i)).collect();
                d.set_items("ActionListBox", actions, 0);
                // The pad plugged in by its own name in place of its kind.
                let pad = keys::pad();
                let devices = (0..8)
                    .map(|i| match &pad {
                        Some(p) if p.device == i && !p.name.is_empty() => p.name.clone(),
                        _ => cx.ui.lang(460 + i as u32),
                    })
                    .collect();
                d.set_items("InputDeviceDDL", devices, p.controls.device as usize);
                d.set_selectable("ActionListBox", true);
                d.set_selectable("MapKeyListBox", true);
                Self::fill_keys(cx.ui, d, p);
            }
            4 => {
                for i in 0..10 {
                    let name = format!("Message{i}");
                    d.set_text(&name, p.messages[i].clone());
                    if let Some(c) = d.ctl(&name) {
                        c.max_len = c.max_len.min(127);
                    }
                }
            }
            _ => {
                self.fill_curves(cx.ui);
                self.show_bands();
            }
        }
    }

    /// The controls page's keys on the device shown (`0x4b7af0`).
    fn fill_keys(ui: &Ui, d: &mut Dlg, p: &Profile) {
        let device = p.controls.device as usize;
        let keys = (0..ACTIONS)
            .map(|a| {
                let (kind, code) = p.keys.get(device, a);
                keys::input_name(kind, code, |id| ui.lang(id))
            })
            .collect();
        let at = d.selected("MapKeyListBox");
        d.set_items("MapKeyListBox", keys, at);
    }

    /// The page's controls into the copy (slot 31 with 1, and the bound
    /// controls as the page closes).
    fn commit(&mut self, cx: &mut Cx) {
        let Some((page, d)) = self.page.as_mut() else {
            return;
        };
        let p = &mut self.p;
        match *page {
            0 => {
                for (name, v) in game_checks(p) {
                    *v = d.get(name).is_some_and(|c| c.on);
                }
            }
            1 => {
                if let Some(m) = d
                    .get("DisplayResolutionDropDown")
                    .filter(|_| crate::platform::DISPLAY_MODES)
                    .and_then(|c| c.item())
                {
                    p.graphics.resolution = m.to_string();
                }
                p.graphics.terrain = d.slider("TerrainQualitySliderBar");
                for (name, v) in graphics_checks(p) {
                    *v = d.get(name).is_some_and(|c| c.on);
                }
            }
            2 => self.commit_sound(cx.ui),
            3 => {
                for (name, v) in control_sliders(p) {
                    *v = d.slider(name);
                }
                for (name, v) in control_checks(p) {
                    *v = d.get(name).is_some_and(|c| c.on);
                }
                p.controls.device = d.selected("InputDeviceDDL") as u32;
                p.keys.current = p.controls.device;
            }
            4 => {
                for i in 0..10 {
                    if let Some(t) = d.get(&format!("Message{i}")).and_then(|c| c.text.clone()) {
                        p.messages[i] = t;
                    }
                }
            }
            _ => {
                for (i, name) in SUSPENSION.iter().enumerate() {
                    p.garage.suspension[i] = d.slider_at(name, 100) as f32 * 0.01;
                }
            }
        }
    }

    /// The sound page into the copy, and into the menus' volume at once
    /// (`0x4b7420`).
    fn commit_sound(&mut self, ui: &mut Ui) {
        let Some((2, d)) = self.page.as_mut() else {
            return;
        };
        let p = &mut self.p;
        for (name, v) in sound_checks(p) {
            *v = d.get(name).is_some_and(|c| c.on);
        }
        for (name, v) in sound_sliders(p) {
            *v = d.slider(name).min(100);
        }
        p.sound.sixteen_bit = d.get("Rad16Bit").is_some_and(|c| c.on);
        ui.volume = p.sound.ui_gain();
    }

    // ---- The garage page (`0x4b7be0`) ----

    /// DDLCurves: the class's three stock curves and the player's three
    /// (`0x4b7f70`).
    fn fill_curves(&mut self, ui: &mut Ui) {
        let cc = self.p.garage.cc;
        let cls = engine_class(cc);
        let size = ui.lang(5226).replace("%d", &ENGINE_SIZES[cls].to_string());
        let (stock, custom) = (ui.lang(5227), ui.lang(5228));
        let lines = (0..6)
            .map(|i| {
                if i < 3 {
                    format!("{cc}cc ({size}) {stock} #{}", i + 1)
                } else {
                    format!("{cc}cc ({size}) {custom} #{}", i - 2)
                }
            })
            .collect();
        let curve = self.p.garage.curve[cls];
        if let Some(d) = self.dlg() {
            d.set_items("DDLCurves", lines, curve);
        }
        self.choose_curve(curve);
    }

    fn choose_curve(&mut self, curve: usize) {
        let cls = engine_class(self.p.garage.cc);
        self.p.garage.curve[cls] = curve;
        self.bands = if curve < 3 {
            self.classes[cls].stock[curve]
        } else {
            self.p.garage.custom[cls][curve - 3]
        };
    }

    /// Fills in each rpm band's slider and figure. The stock curves show
    /// greyed out; the player's own are editable (`0x4b8240`).
    fn show_bands(&mut self) {
        let cls = engine_class(self.p.garage.cc);
        let class = self.classes[cls].clone();
        let curve = self.p.garage.curve[cls];
        let suspension = self.p.garage.suspension;
        let Some(d) = self.page.as_mut().map(|(_, d)| d) else {
            return;
        };
        d.show("TxtUnused", curve >= 3);
        d.show("HPUnused", curve >= 3);
        let span = (class.most - class.least).max(0) as u32;
        self.used = 0;
        for b in 0..11 {
            let power = self.bands[b];
            let rpm = (b as i32 * class.step() + class.rpm_low) as f32 * 0.001;
            d.set_text(&format!("Txt{}", b + 1), format!("{rpm:2.1}k"));
            let slider = format!("SldEQ{}", b + 1);
            if let Some(c) = d.ctl(&slider) {
                c.reversed = true;
            }
            d.set_steps(&slider, span + 1);
            d.set_slider(&slider, (power - class.least).max(0) as u32, Some(span));
            d.enable(&slider, curve >= 3);
            self.used += power;
            d.set_text(&format!("HP{}", b + 1), power.to_string());
        }
        d.set_text("HPUnused", (class.total - self.used).to_string());
        for (i, name) in SUSPENSION.iter().enumerate() {
            d.set_slider(name, (suspension[i] * 100.0) as u32, Some(100));
        }
    }

    /// Called when the player drags one band's slider. The new figure is
    /// clamped to that band's cap and to the unspent power, then stored in
    /// the selected curve (`0x4b8120`).
    fn slide_band(&mut self, band: usize) {
        let cls = engine_class(self.p.garage.cc);
        let class = &self.classes[cls];
        let span = (class.most - class.least).max(0) as u32;
        let Some(d) = self.page.as_ref().map(|(_, d)| d) else {
            return;
        };
        let value = d.slider_at(&format!("SldEQ{}", band + 1), span) as i32 + class.least;
        let most = value.min(class.limits[band]);
        let power = (self.bands[band] + (class.total - self.used)).min(most);
        self.bands[band] = power;
        let curve = self.p.garage.curve[cls];
        if curve >= 3 {
            self.p.garage.custom[cls][curve - 3][band] = power;
        }
        // The suspension is read back before the bands redraw it.
        if let Some((_, d)) = self.page.as_ref() {
            for (i, name) in SUSPENSION.iter().enumerate() {
                self.p.garage.suspension[i] = d.slider_at(name, 100) as f32 * 0.01;
            }
        }
        self.show_bands();
    }

    // ---- The controls page (`0x4b74e0`) ----

    /// An action picked: "press any key" in its place until one is
    /// (`0x4b74e0` event 2).
    fn listen(&mut self, ui: &Ui, line: usize) {
        if line >= ACTIONS {
            return;
        }
        self.listening = Some(line);
        if let Some(d) = self.dlg() {
            let mut keys = d
                .get("MapKeyListBox")
                .map(|c| c.items.clone())
                .unwrap_or_default();
            if let Some(k) = keys.get_mut(line) {
                *k = ui.lang(450);
            }
            d.set_items("MapKeyListBox", keys, line);
            d.mark("MapKeyListBox", Some(line));
            d.mark("ActionListBox", Some(line));
        }
    }

    /// Binds the key just let go to the waiting action, unbinding it from
    /// whichever action held it before. Escape cancels and leaves the
    /// binding alone (`0x4b9c00`).
    fn take_key(&mut self, ui: &Ui, code: u32) {
        if code == 0x01 {
            self.listening = None;
            self.refill_keys(ui);
        } else {
            self.take_input(ui, (0, code as i32));
        }
    }

    /// Binds a key or joystick input to the waiting action. The game's
    /// page took only keys for the steering and lean (`0x4b9b70`) and only
    /// buttons from a joystick; here a stick pushed binds its axis to the
    /// steering or lean (both sides at once, as the factory map has
    /// them), and Accelerate also takes a stick's throttle lever (the Z
    /// axis), which the game's map can hold (0x449e20).
    fn take_input(&mut self, ui: &Ui, input: (i32, i32)) {
        let Some(action) = self.listening else {
            return;
        };
        let pair = match action {
            0 | 1 => Some([0, 1]),
            2 | 3 => Some([2, 3]),
            _ => None,
        };
        match input {
            (2, code) if code < 0 && pair.is_none() && !(action == crate::input::ACCELERATE && code == -7) => {
                return
            }
            _ => {}
        }
        self.listening = None;
        // A joystick's input binds on the joystick's own map: shown the
        // keyboard or mouse, the page turns to the pad (as the race reads it).
        if input.0 == 2 && self.p.controls.device as usize >= crate::input::KEYBOARD {
            let pad = keys::pad().map_or(3, |p| p.device);
            self.p.controls.device = pad as u32;
            if let Some(d) = self.dlg() {
                d.select("InputDeviceDDL", pad);
            }
        }
        let device = self.p.controls.device as usize;
        if let (Some(pair), (2, code)) = (pair, input) {
            if code < 0 {
                for a in 0..ACTIONS {
                    if !pair.contains(&a) && self.p.keys.get(device, a) == input {
                        self.p.keys.set(device, a, (-1, 0));
                    }
                }
                for a in pair {
                    self.p.keys.set(device, a, input);
                }
                self.refill_keys(ui);
                return;
            }
        }
        for a in 0..ACTIONS {
            if a != action && self.p.keys.get(device, a) == input {
                self.p.keys.set(device, a, (-1, 0));
            }
        }
        self.p.keys.set(device, action, input);
        self.refill_keys(ui);
    }

    fn refill_keys(&mut self, ui: &Ui) {
        if let Some((_, d)) = self.page.as_mut() {
            Self::fill_keys(ui, d, &self.p);
            d.mark("MapKeyListBox", None);
            d.mark("ActionListBox", None);
        }
    }

    /// The ratchet sound while a slider moves (`0x4b6ac0`).
    fn ratchet(&mut self, cx: &mut Cx, on: bool) {
        let Some(audio) = cx.audio else {
            return;
        };
        // Not again while it is still clicking.
        if on && self.ratchet.is_some() && cx.now - self.ratchet_at < 0.15 {
            return;
        }
        self.ratchet_at = cx.now;
        if let Some(v) = self.ratchet.take() {
            audio.stop(v);
        }
        if on && cx.ui.volume > 0.0 {
            let pack = self
                .page
                .as_ref()
                .map_or("OPTBASE".to_string(), |(_, d)| d.pack.clone());
            if let Some(s) = cx.ui.sound(&pack, "Ratchet03") {
                self.ratchet = Some(audio.play(&s, cx.ui.volume, false));
            }
        }
    }

    fn page_event(&mut self, cx: &mut Cx, e: Event) {
        let Some(page) = self.page.as_ref().map(|(i, _)| *i) else {
            return;
        };
        match (page, e) {
            (0, Event::Clicked(n)) if n.eq_ignore_ascii_case("MultChatMode") => {
                self.p.game.chat_mode = (self.p.game.chat_mode + 1) % 3;
                let text = cx.ui.lang(CHAT_MODES[self.p.game.chat_mode as usize]);
                if let Some(d) = self.dlg() {
                    d.set_text("MultChatMode", text);
                }
            }
            (1, Event::Changed(n)) => {
                self.ratchet(cx, true);
                if n.eq_ignore_ascii_case("TerrainQualitySliderBar") {
                    // The overall quality sets all four details at once.
                    let q = self
                        .dlg()
                        .map_or(0, |d| d.slider("TerrainQualitySliderBar"));
                    self.p.graphics.terrain = q;
                    for key in DETAILS {
                        cx.user.settings.set(key, &q.to_string());
                    }
                }
            }
            (2, Event::Clicked(_)) => self.commit_sound(cx.ui),
            (2, Event::Changed(_)) => {
                self.commit_sound(cx.ui);
                self.ratchet(cx, true);
            }
            (3, Event::Selected(n, line))
                if n.eq_ignore_ascii_case("ActionListBox")
                    || n.eq_ignore_ascii_case("MapKeyListBox") =>
            {
                if self.listening.is_none() {
                    self.listen(cx.ui, line);
                }
            }
            (3, Event::Selected(n, line)) if n.eq_ignore_ascii_case("InputDeviceDDL") => {
                self.p.controls.device = line as u32;
                let ui = &*cx.ui;
                if let Some((_, d)) = self.page.as_mut() {
                    Self::fill_keys(ui, d, &self.p);
                }
            }
            (3, Event::Changed(n)) if !n.eq_ignore_ascii_case("ListboxScroll") => {
                self.ratchet(cx, true)
            }
            (5, Event::Clicked(n)) => {
                let n = n.to_ascii_lowercase();
                if n == "curveleft" || n == "curveright" {
                    let cls = engine_class(self.p.garage.cc);
                    let by = if n == "curveleft" { 5 } else { 1 };
                    let curve = (self.p.garage.curve[cls] + by) % 6;
                    if let Some(d) = self.dlg() {
                        d.select("DDLCurves", curve);
                    }
                    self.choose_curve(curve);
                    self.show_bands();
                }
            }
            (5, Event::Selected(n, line)) if n.eq_ignore_ascii_case("DDLCurves") => {
                self.choose_curve(line.min(5));
                self.show_bands();
            }
            (5, Event::Changed(n)) | (5, Event::Released(n)) => {
                self.ratchet(cx, true);
                let band = n
                    .get(..5)
                    .filter(|p| p.eq_ignore_ascii_case("SldEQ"))
                    .and_then(|_| n[5..].parse::<usize>().ok());
                if let Some(b) = band.filter(|b| (1..=11).contains(b)) {
                    self.slide_band(b - 1);
                }
            }
            (_, Event::Released(_)) => self.ratchet(cx, false),
            _ => {}
        }
    }

    // ---- Advanced (`0x4b6dd0`) ----

    fn open_advanced(&mut self, cx: &mut Cx) {
        if self.page.as_ref().is_some_and(|(i, _)| *i == 1) {
            self.commit(cx);
        }
        let Some(mut d) = Dlg::open(cx.ui, "OPTADVG", (0.0, 0.0)) else {
            return;
        };
        let s = &cx.user.settings;
        d.set_on("ChkForceVideoMem", s.number("ForceVideoMemory", 0) != 0);
        for key in DETAILS {
            d.set_slider(
                key,
                s.number(key, self.p.graphics.terrain as i64).clamp(0, 9) as u32,
                None,
            );
        }
        // This renderer is never the software one.
        d.set_text("Txt3DCard", "Hardware (wgpu)");
        d.set_on("ChkShowPick3D", s.number("UseLastVideoCard", 1) == 0);
        let method = s.number("TerrainDetailTextureMethod", -1);
        let radio = match method {
            0 => "RadTerrNone",
            1 => "RadTerrDual",
            _ => "RadTerrSingle",
        };
        d.set_on(radio, true);
        d.set_on("ChkFilterInput", s.number("JoystickFilter", 0) != 0);
        d.set_on(
            "ChkShowPickController",
            s.number("UseLastController", 0) == 0,
        );
        self.sub = Some(Sub::Advanced(d));
    }

    /// Closing the advanced screen keeps its choices, and the overall
    /// quality becomes the four details' mean.
    fn close_advanced(&mut self, cx: &mut Cx) {
        let Some(Sub::Advanced(d)) = self.sub.take() else {
            return;
        };
        let s = &mut cx.user.settings;
        let on = |n: &str| d.get(n).is_some_and(|c| c.on);
        s.set(
            "ForceVideoMemory",
            &(on("ChkForceVideoMem") as u32).to_string(),
        );
        let mut sum = 0;
        for key in DETAILS {
            let v = d.slider(key);
            sum += v;
            s.set(key, &v.to_string());
        }
        s.set(
            "UseLastVideoCard",
            &(!on("ChkShowPick3D") as u32).to_string(),
        );
        let method = if on("RadTerrSingle") {
            2
        } else {
            on("RadTerrDual") as u32
        };
        s.set("TerrainDetailTextureMethod", &method.to_string());
        s.set("JoystickFilter", &(on("ChkFilterInput") as u32).to_string());
        s.set(
            "UseLastController",
            &(!on("ChkShowPickController") as u32).to_string(),
        );
        cx.user.save_settings();
        self.p.graphics.terrain = sum / 4;
        if let Some((1, _)) = &self.page {
            self.fill(cx);
        }
    }

    // ---- Restore (`0x4b9470`) ----

    fn open_restore(&mut self, cx: &mut Cx) {
        let Some(mut d) = popup(cx.ui, "GLOBSET") else {
            return;
        };
        // The other players' profiles (`0x4b97a0`).
        let others: Vec<String> = cx
            .user
            .store
            .list()
            .into_iter()
            .filter(|n| *n != self.p.name)
            .collect();
        if others.is_empty() {
            d.enable("ButCopyUser", false);
        }
        d.set_items("LstProfiles", others, 0);
        d.set_selectable("LstProfiles", true);
        d.mark("LstProfiles", Some(0));
        d.show("LstProfiles", false);
        d.show("ScrollProfiles", false);
        d.set_on("ButFactory", true);
        self.sub = Some(Sub::Restore {
            dlg: d,
            confirm: None,
        });
    }

    fn restore_event(&mut self, cx: &mut Cx, e: Event) {
        let Some(Sub::Restore { dlg, confirm }) = &mut self.sub else {
            return;
        };
        if confirm.is_some() {
            let Event::Clicked(n) = e else { return };
            if n.eq_ignore_ascii_case("ButLeft") {
                // Confirmed (0x60): the factory settings, or another
                // player's, under this player's name (`0x4b9a40`).
                let factory = dlg.get("ButFactory").is_some_and(|c| c.on);
                let other = dlg
                    .get("LstProfiles")
                    .and_then(|c| c.mark.and_then(|m| c.items.get(m).cloned()));
                let messages = super::default_messages(cx.ui);
                let preset = Controls::preset(cx.game());
                if factory {
                    self.p.defaults(messages, preset);
                } else if let Some(o) = other {
                    if let Some(mut q) = cx.user.store.load(&o, messages, preset) {
                        q.name = self.p.name.clone();
                        q.bike = self.p.bike.clone();
                        q.outfit = self.p.outfit.clone();
                        q.plate = self.p.plate;
                        self.p = q;
                    }
                }
                self.sub = None;
                self.fill(cx);
                if let Some((2, _)) = &self.page {
                    self.commit_sound(cx.ui);
                }
            } else if n.eq_ignore_ascii_case("ButRight") {
                self.sub = None;
            }
            return;
        }
        let Event::Clicked(n) = e else { return };
        match n.to_ascii_lowercase().as_str() {
            "butfactory" => {
                dlg.show("LstProfiles", false);
                dlg.show("ScrollProfiles", false);
            }
            "butcopyuser" => {
                dlg.show("LstProfiles", true);
                dlg.show("ScrollProfiles", true);
            }
            "butcancel" => self.sub = None,
            "butload" => {
                let (t, p) = (cx.ui.lang(5321), cx.ui.lang(5322));
                if let Some(mut b) = MsgBox::ask(cx.ui, &t, &p, [5097, 0, 5084]) {
                    b.dlg.set_wrap("TxtPrompt", true);
                    *confirm = Some(b);
                }
            }
            _ => {}
        }
    }
}

impl Screen for Options {
    fn layers(&mut self) -> Vec<&mut Dlg> {
        match &mut self.sub {
            Some(Sub::Advanced(d)) => vec![d],
            Some(Sub::Restore { dlg, confirm }) => {
                let mut v = vec![&mut self.base];
                if let Some((_, p)) = &mut self.page {
                    v.push(p);
                }
                v.push(dlg);
                if let Some(b) = confirm {
                    v.push(&mut b.dlg);
                }
                v
            }
            None => {
                let mut v = vec![&mut self.base];
                if let Some((_, p)) = &mut self.page {
                    v.push(p);
                }
                v
            }
        }
    }

    fn modal(&self) -> bool {
        self.sub.is_some()
    }

    fn event(&mut self, cx: &mut Cx, e: Event) -> Go {
        match &self.sub {
            Some(Sub::Advanced(_)) => {
                if let Event::Clicked(n) = &e {
                    if n.eq_ignore_ascii_case("Back") {
                        self.close_advanced(cx);
                    }
                }
                return Go::Stay;
            }
            Some(Sub::Restore { .. }) => {
                self.restore_event(cx, e);
                return Go::Stay;
            }
            None => {}
        }
        if let Event::Clicked(n) = &e {
            let lower = n.to_ascii_lowercase();
            if let Some(i) = PAGES.iter().position(|(t, _, _)| t.eq_ignore_ascii_case(n)) {
                self.show_page(cx, i);
                return Go::Stay;
            }
            match lower.as_str() {
                "back" => {
                    // Leaving the screen hands the edited copy back (`0x4b8880` close).
                    self.close_page(cx);
                    cx.user.profile = self.p.clone();
                    cx.ui.volume = self.p.sound.ui_gain();
                    cx.user.save();
                    self.ratchet(cx, false);
                    return Go::Back;
                }
                "advanced" => {
                    self.open_advanced(cx);
                    return Go::Stay;
                }
                "restore" => {
                    self.open_restore(cx);
                    return Go::Stay;
                }
                // The help file is WinHelp, which no system opens now.
                "help" => return Go::Stay,
                _ => {}
            }
        }
        self.page_event(cx, e);
        Go::Stay
    }

    fn escape(&mut self, cx: &mut Cx) -> Go {
        match &mut self.sub {
            Some(Sub::Advanced(_)) => self.close_advanced(cx),
            Some(Sub::Restore { confirm, .. }) => {
                if confirm.is_some() {
                    *confirm = None;
                } else {
                    self.sub = None;
                }
            }
            None if self.listening.is_some() => self.take_key(cx.ui, 0x01),
            None => return self.event(cx, Event::Clicked("Back".into())),
        }
        Go::Stay
    }

    fn key(&mut self, cx: &mut Cx, code: u32) -> Go {
        if self.sub.is_none() && self.listening.is_some() {
            self.take_key(cx.ui, code);
        }
        Go::Stay
    }

    fn wants_pad(&self) -> bool {
        self.sub.is_none() && self.listening.is_some()
    }

    fn pad(&mut self, cx: &mut Cx, input: (i32, i32)) -> Go {
        if self.sub.is_none() && self.listening.is_some() {
            self.take_input(cx.ui, input);
        }
        Go::Stay
    }

    fn draw(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32) {
        for d in self.layers() {
            d.tick(now);
            d.draw(ui, gfx, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        assert_eq!(engine_class(125), 0);
        assert_eq!(engine_class(250), 1);
        assert_eq!(engine_class(300), 1);
        assert_eq!(engine_class(400), 2);
        assert_eq!(engine_class(450), 2);
        assert_eq!(engine_class(500), 3);
        assert_eq!(engine_class(650), 4);
    }

    #[test]
    fn presets() {
        let Some(game) = std::env::var_os("MM2_DATA") else {
            return;
        };
        let c = engine_classes(Path::new(&game));
        assert_eq!(
            (c[1].rpm_low, c[1].rpm_high, c[1].total),
            (5000, 10000, 380)
        );
        assert_eq!(c[1].least, 20);
        assert_eq!(c[1].most, 45);
        assert_eq!(c[1].stock[0][0], 24);
        assert_eq!(c[0].limits[10], 20);
    }
}
