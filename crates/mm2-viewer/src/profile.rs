//! The player's profile and the game's own settings, as `Mcm2.exe` keeps
//! them (`uiinfo.cpp`, `0x527690`): each profile in its folder
//! `profile/<name>/` with its settings in `<name>.prf` and its key map in
//! `control.ctl`, and the machine's settings (the game's registry key:
//! the last profile, the start-up pickers, the player ID) in
//! `settings.ini`.
//!
//! The original's `.prf` is the profile object written raw; ours holds the
//! same values as INI text, so it can be read and moved between machines.
//! Everything lives in the user's data folder (`MM2_USER_DIR`, else
//! `~/Library/Application Support/MM2`, `%APPDATA%\MM2` or
//! `$XDG_DATA_HOME/mm2`), never in the game's folder.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The longest profile name the game takes.
pub const NAME_LENGTH: usize = 15;

/// The letters a profile name may have, besides letters and digits
/// (`0x56fe6c`).
pub const NAME_SYMBOLS: &str = "_-()$#@";

/// The Game page's settings (profile `+0x6ac`, `0x527d50`).
#[derive(Debug, Clone, PartialEq)]
pub struct GameOptions {
    pub gauges: bool,
    pub race_stats: bool,
    /// The chat box: 0 normal, 1 always on, 2 always off.
    pub chat_mode: u32,
    pub overview: bool,
    pub rider_names: bool,
    pub rider_position: bool,
    pub podiums: bool,
    pub ui_anims: bool,
    pub intro_movie: bool,
    pub player_indicator: bool,
    pub tooltips: bool,
    pub wreck_reset: bool,
}

impl Default for GameOptions {
    fn default() -> Self {
        GameOptions {
            gauges: true,
            race_stats: true,
            chat_mode: 0,
            overview: true,
            rider_names: true,
            rider_position: true,
            podiums: true,
            ui_anims: true,
            intro_movie: true,
            player_indicator: true,
            tooltips: true,
            wreck_reset: true,
        }
    }
}

/// The Sound page's settings (profile `+0xa20`, `0x527db0`).
#[derive(Debug, Clone, PartialEq)]
pub struct SoundOptions {
    pub ui_sounds: bool,
    pub effects: bool,
    pub hardware: bool,
    pub eax: bool,
    pub waypoint_chime: bool,
    /// 0 to 100.
    pub ui_volume: u32,
    pub game_volume: u32,
    pub sixteen_bit: bool,
}

impl Default for SoundOptions {
    fn default() -> Self {
        SoundOptions {
            ui_sounds: true,
            effects: true,
            hardware: false,
            eax: false,
            waypoint_chime: true,
            ui_volume: 100,
            game_volume: 100,
            sixteen_bit: false,
        }
    }
}

impl SoundOptions {
    /// The menus' sounds' volume, 0 to 1: the game gives DirectSound
    /// `(volume * 5 - 500) * 5 - 200` hundredths of a decibel
    /// (`0x4b7420`), silence without UI sounds.
    pub fn ui_gain(&self) -> f32 {
        if !self.ui_sounds {
            return 0.0;
        }
        let centibels = ((self.ui_volume as f32 * 5.0 - 500.0) * 5.0 - 200.0) / 100.0;
        10f32.powf(centibels / 20.0)
    }
}

/// The Graphics page's settings (profile `+0xa4c`, `0x527e00`).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphicsOptions {
    /// The display mode, as "W x H".
    pub resolution: String,
    pub shadows: bool,
    pub particles: bool,
    pub sky: bool,
    /// Overall quality, 0 to 9.
    pub terrain: u32,
}

impl Default for GraphicsOptions {
    fn default() -> Self {
        GraphicsOptions {
            resolution: "640 X 480".into(),
            shadows: true,
            particles: true,
            sky: true,
            terrain: 6,
        }
    }
}

/// The Controls page's settings (profile `+0xa68`, `0x527e30`): eight
/// sensitivities of 0 to 100 and four switches.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlOptions {
    pub steering: u32,
    pub cross_up: u32,
    pub pitch: u32,
    pub press_throttle: u32,
    pub press_brakes: u32,
    pub release_throttle: u32,
    pub release_brakes: u32,
    pub feedback: u32,
    pub force_feedback: bool,
    pub gas_gyro: bool,
    pub brake_gyro: bool,
    pub analog_axes: bool,
    /// The device whose keys the page shows (string 460 and on).
    pub device: u32,
}

impl Default for ControlOptions {
    fn default() -> Self {
        ControlOptions {
            steering: 50,
            cross_up: 50,
            pitch: 50,
            press_throttle: 50,
            press_brakes: 50,
            release_throttle: 50,
            release_brakes: 50,
            feedback: 50,
            force_feedback: true,
            gas_gyro: false,
            brake_gyro: true,
            analog_axes: true,
            // Keyboard: what the original shows with no joystick.
            device: 6,
        }
    }
}

/// The Garage page's settings (profile `+0xfd8`, `0x5282c0`).
#[derive(Debug, Clone, PartialEq)]
pub struct Garage {
    /// The bike's engine size, which picks its class of curves.
    pub cc: u32,
    /// Front and rear compression, decompression, then front and rear
    /// damping, 0 to 1.
    pub suspension: [f32; 6],
    /// The curve each engine class uses: 0 to 2 the stock ones, 3 to 5
    /// the player's.
    pub curve: [usize; 5],
    /// The player's three curves for each class, horsepower per band.
    pub custom: [[[i32; 11]; 3]; 5],
    /// The curve the bike rides on (+0x24 on): its revs from, to and the
    /// step, and the horsepower per band. A new profile has none and the
    /// bike rides the engine's built-in curve, until the Bike & Rider
    /// page puts a bike of another class under it (0x450f00).
    pub ride_rpm: [i32; 3],
    pub ride_power: [i32; 11],
}

impl Default for Garage {
    fn default() -> Self {
        Garage {
            cc: 250,
            suspension: [0.5; 6],
            curve: [0; 5],
            custom: [[[10; 11]; 3]; 5],
            ride_rpm: [0; 3],
            ride_power: [0; 11],
        }
    }
}

/// The key map: per device (0 keyboard, 1 to 7 the others), each of
/// the 14 actions' input as the game codes it (kind 0 a key's scan code,
/// 1 the mouse, 2 a joystick; negative codes are axes).
#[derive(Debug, Clone, PartialEq)]
pub struct Controls {
    pub current: u32,
    pub keys: Vec<[(i32, i32); ACTIONS]>,
}

/// The actions the controls page lists (strings 402 to 415).
pub const ACTIONS: usize = 14;

impl Controls {
    /// The map in a `control.ctl` (`UI/PRESET.CTL` is the factory one).
    pub fn parse(text: &str) -> Controls {
        let ini = Ini::parse(text);
        let current = ini
            .section("Setup")
            .and_then(|s| s.get("CurrentInputDevice"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(1);
        let mut keys = Vec::new();
        for d in 0..8 {
            let Some(sec) = ini.section(&format!("Controller{d}")) else {
                break;
            };
            let mut map = [(-1, 0); ACTIONS];
            for (a, slot) in map.iter_mut().enumerate() {
                if let Some((k, c)) = sec.get(&format!("Key{a}")).and_then(|v| v.split_once(',')) {
                    *slot = (
                        k.trim().parse().unwrap_or(-1),
                        c.trim().parse().unwrap_or(0),
                    );
                }
            }
            keys.push(map);
        }
        Controls { current, keys }
    }

    pub fn text(&self) -> String {
        let mut out = format!("[Setup]\r\nCurrentInputDevice={}\r\n", self.current);
        for (d, map) in self.keys.iter().enumerate() {
            let _ = write!(out, "\r\n[Controller{d}]\r\n");
            for (a, (k, c)) in map.iter().enumerate() {
                let _ = write!(out, "Key{a}={k},{c}\r\n");
            }
        }
        out
    }

    /// The factory map, from the game's `UI/PRESET.CTL`.
    pub fn preset(game: &Path) -> Controls {
        let text = crate::platform::fs::read(game.join("UI").join("PRESET.CTL"))
            .map(|b| b.iter().map(|&c| c as char).collect::<String>())
            .unwrap_or_default();
        let mut c = Controls::parse(&text);
        if c.keys.is_empty() {
            c.keys.push([(-1, 0); ACTIONS]);
        }
        c
    }

    /// An action's input on a device.
    pub fn get(&self, device: usize, action: usize) -> (i32, i32) {
        self.keys
            .get(device)
            .map_or((-1, 0), |m| m.get(action).copied().unwrap_or((-1, 0)))
    }

    pub fn set(&mut self, device: usize, action: usize, input: (i32, i32)) {
        while self.keys.len() <= device {
            self.keys.push([(-1, 0); ACTIONS]);
        }
        self.keys[device][action] = input;
    }
}

/// One player's profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub game: GameOptions,
    pub sound: SoundOptions,
    pub graphics: GraphicsOptions,
    pub controls: ControlOptions,
    pub garage: Garage,
    /// The ten chat messages, each at the digit of its Control key.
    pub messages: [String; 10],
    /// The Bike & Rider page's picks: the bike's and the outfit's names
    /// and the plate number.
    pub bike: String,
    pub outfit: String,
    pub plate: u32,
    pub keys: Controls,
}

impl Profile {
    /// A new profile with the game's defaults (`0x527a70`); `messages`
    /// are strings 5143 to 5152 and `keys` the factory map.
    pub fn new(name: &str, messages: [String; 10], keys: Controls) -> Profile {
        Profile {
            name: name.to_string(),
            game: GameOptions::default(),
            sound: SoundOptions::default(),
            graphics: GraphicsOptions::default(),
            controls: ControlOptions::default(),
            garage: Garage::default(),
            messages,
            bike: String::new(),
            outfit: String::new(),
            plate: 101,
            keys,
        }
    }

    /// The settings back to the game's defaults, keeping the name and the
    /// rider (the Restore dialog's "Reset to Defaults").
    pub fn defaults(&mut self, messages: [String; 10], keys: Controls) {
        let keep = (self.name.clone(), self.bike.clone(), self.outfit.clone());
        *self = Profile::new(&keep.0, messages, keys);
        self.bike = keep.1;
        self.outfit = keep.2;
    }

    /// The profile as `<name>.prf` holds it.
    pub fn text(&self) -> String {
        let mut o = String::new();
        let b = |v: bool| v as u32;
        let g = &self.game;
        let _ = write!(
            o,
            "[Profile]\r\nName={}\r\nBike={}\r\nOutfit={}\r\nPlate={}\r\n",
            self.name, self.bike, self.outfit, self.plate
        );
        let _ = write!(
            o,
            "\r\n[Game]\r\nGauges={}\r\nRaceStats={}\r\nChatMode={}\r\nOverview={}\r\n\
RiderNames={}\r\nRiderPosition={}\r\nPodiums={}\r\nUIAnims={}\r\nIntroMovie={}\r\n\
PlayerIndicator={}\r\nToolTips={}\r\nWreckResetOnTrack={}\r\n",
            b(g.gauges),
            b(g.race_stats),
            g.chat_mode,
            b(g.overview),
            b(g.rider_names),
            b(g.rider_position),
            b(g.podiums),
            b(g.ui_anims),
            b(g.intro_movie),
            b(g.player_indicator),
            b(g.tooltips),
            b(g.wreck_reset)
        );
        let s = &self.sound;
        let _ = write!(
            o,
            "\r\n[Sound]\r\nUISounds={}\r\nEffects={}\r\nHardware={}\r\nEAX={}\r\n\
WaypointChime={}\r\nUIVolume={}\r\nGameVolume={}\r\nSixteenBit={}\r\n",
            b(s.ui_sounds),
            b(s.effects),
            b(s.hardware),
            b(s.eax),
            b(s.waypoint_chime),
            s.ui_volume,
            s.game_volume,
            b(s.sixteen_bit)
        );
        let v = &self.graphics;
        let _ = write!(
            o,
            "\r\n[Graphics]\r\nResolution={}\r\nShadows={}\r\nParticles={}\r\nSky={}\r\n\
Terrain={}\r\n",
            v.resolution,
            b(v.shadows),
            b(v.particles),
            b(v.sky),
            v.terrain
        );
        let c = &self.controls;
        let _ = write!(
            o,
            "\r\n[Controls]\r\nSteering={}\r\nCrossUp={}\r\nPitch={}\r\nPressThrottle={}\r\n\
PressBrakes={}\r\nReleaseThrottle={}\r\nReleaseBrakes={}\r\nFeedback={}\r\n\
ForceFeedback={}\r\nGasGyro={}\r\nBrakeGyro={}\r\nAnalogAxes={}\r\nDevice={}\r\n",
            c.steering,
            c.cross_up,
            c.pitch,
            c.press_throttle,
            c.press_brakes,
            c.release_throttle,
            c.release_brakes,
            c.feedback,
            b(c.force_feedback),
            b(c.gas_gyro),
            b(c.brake_gyro),
            b(c.analog_axes),
            c.device
        );
        let gar = &self.garage;
        let _ = write!(o, "\r\n[Garage]\r\nCC={}\r\n", gar.cc);
        let sus: Vec<String> = gar.suspension.iter().map(|v| format!("{v}")).collect();
        let _ = write!(o, "Suspension={}\r\n", sus.join(","));
        let curves: Vec<String> = gar.curve.iter().map(|v| v.to_string()).collect();
        let _ = write!(o, "Curves={}\r\n", curves.join(","));
        for (cls, set) in gar.custom.iter().enumerate() {
            for (n, bands) in set.iter().enumerate() {
                let bands: Vec<String> = bands.iter().map(|v| v.to_string()).collect();
                let _ = write!(o, "Custom{}_{}={}\r\n", cls + 1, n + 1, bands.join(","));
            }
        }
        if gar.ride_rpm[1] > 0 {
            let rpm: Vec<String> = gar.ride_rpm.iter().map(|v| v.to_string()).collect();
            let power: Vec<String> = gar.ride_power.iter().map(|v| v.to_string()).collect();
            let _ = write!(o, "RideRPM={}\r\nRidePower={}\r\n", rpm.join(","), power.join(","));
        }
        let _ = write!(o, "\r\n[Messages]\r\n");
        for (i, m) in self.messages.iter().enumerate() {
            let _ = write!(o, "Message{i}={m}\r\n");
        }
        o
    }

    /// A profile from its `<name>.prf` text, with defaults for what it
    /// lacks.
    pub fn parse(text: &str, messages: [String; 10], keys: Controls) -> Profile {
        let ini = Ini::parse(text);
        let get = |sec: &str, key: &str| ini.section(sec).and_then(|s| s.get(key));
        let num = |sec: &str, key: &str, d: u32| {
            get(sec, key)
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(d)
        };
        let flag = |sec: &str, key: &str, d: bool| num(sec, key, d as u32) != 0;
        let mut p = Profile::new(get("Profile", "Name").unwrap_or(""), messages, keys);
        p.bike = get("Profile", "Bike").unwrap_or("").to_string();
        p.outfit = get("Profile", "Outfit").unwrap_or("").to_string();
        p.plate = num("Profile", "Plate", 101);
        let d = GameOptions::default();
        p.game = GameOptions {
            gauges: flag("Game", "Gauges", d.gauges),
            race_stats: flag("Game", "RaceStats", d.race_stats),
            chat_mode: num("Game", "ChatMode", 0).min(2),
            overview: flag("Game", "Overview", d.overview),
            rider_names: flag("Game", "RiderNames", d.rider_names),
            rider_position: flag("Game", "RiderPosition", d.rider_position),
            podiums: flag("Game", "Podiums", d.podiums),
            ui_anims: flag("Game", "UIAnims", d.ui_anims),
            intro_movie: flag("Game", "IntroMovie", d.intro_movie),
            player_indicator: flag("Game", "PlayerIndicator", d.player_indicator),
            tooltips: flag("Game", "ToolTips", d.tooltips),
            wreck_reset: flag("Game", "WreckResetOnTrack", d.wreck_reset),
        };
        let d = SoundOptions::default();
        p.sound = SoundOptions {
            ui_sounds: flag("Sound", "UISounds", d.ui_sounds),
            effects: flag("Sound", "Effects", d.effects),
            hardware: flag("Sound", "Hardware", d.hardware),
            eax: flag("Sound", "EAX", d.eax),
            waypoint_chime: flag("Sound", "WaypointChime", d.waypoint_chime),
            ui_volume: num("Sound", "UIVolume", d.ui_volume).min(100),
            game_volume: num("Sound", "GameVolume", d.game_volume).min(100),
            sixteen_bit: flag("Sound", "SixteenBit", d.sixteen_bit),
        };
        let d = GraphicsOptions::default();
        p.graphics = GraphicsOptions {
            resolution: get("Graphics", "Resolution")
                .map(str::to_string)
                .unwrap_or(d.resolution),
            shadows: flag("Graphics", "Shadows", d.shadows),
            particles: flag("Graphics", "Particles", d.particles),
            sky: flag("Graphics", "Sky", d.sky),
            terrain: num("Graphics", "Terrain", d.terrain).min(9),
        };
        let d = ControlOptions::default();
        p.controls = ControlOptions {
            steering: num("Controls", "Steering", d.steering),
            cross_up: num("Controls", "CrossUp", d.cross_up),
            pitch: num("Controls", "Pitch", d.pitch),
            press_throttle: num("Controls", "PressThrottle", d.press_throttle),
            press_brakes: num("Controls", "PressBrakes", d.press_brakes),
            release_throttle: num("Controls", "ReleaseThrottle", d.release_throttle),
            release_brakes: num("Controls", "ReleaseBrakes", d.release_brakes),
            feedback: num("Controls", "Feedback", d.feedback),
            force_feedback: flag("Controls", "ForceFeedback", d.force_feedback),
            gas_gyro: flag("Controls", "GasGyro", d.gas_gyro),
            brake_gyro: flag("Controls", "BrakeGyro", d.brake_gyro),
            analog_axes: flag("Controls", "AnalogAxes", d.analog_axes),
            device: num("Controls", "Device", d.device).min(7),
        };
        p.garage.cc = num("Garage", "CC", 250);
        let list = |key: &str| -> Vec<String> {
            get("Garage", key)
                .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default()
        };
        for (slot, v) in p.garage.suspension.iter_mut().zip(list("Suspension")) {
            *slot = v.parse::<f32>().unwrap_or(0.5).clamp(0.0, 1.0);
        }
        for (slot, v) in p.garage.curve.iter_mut().zip(list("Curves")) {
            *slot = v.parse::<usize>().unwrap_or(0).min(5);
        }
        for cls in 0..5 {
            for n in 0..3 {
                let bands = list(&format!("Custom{}_{}", cls + 1, n + 1));
                for (slot, v) in p.garage.custom[cls][n].iter_mut().zip(bands) {
                    *slot = v.parse().unwrap_or(10);
                }
            }
        }
        for (slot, v) in p.garage.ride_rpm.iter_mut().zip(list("RideRPM")) {
            *slot = v.parse().unwrap_or(0);
        }
        for (slot, v) in p.garage.ride_power.iter_mut().zip(list("RidePower")) {
            *slot = v.parse().unwrap_or(0);
        }
        for (i, m) in p.messages.iter_mut().enumerate() {
            if let Some(v) = get("Messages", &format!("Message{i}")) {
                *m = v.to_string();
            }
        }
        p
    }
}

/// Where the profiles and settings are kept.
#[derive(Debug, Clone)]
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    /// The user's data folder (see the module's notes).
    pub fn open() -> Store {
        let dir = std::env::var_os("MM2_USER_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(user_dir);
        Store { dir }
    }

    fn profiles(&self) -> PathBuf {
        self.dir.join("profile")
    }

    /// The profiles there are, sorted as the game's list sorts them
    /// (ignoring case).
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = crate::platform::fs::read_dir(self.profiles())
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .filter(|e| crate::platform::fs::is_dir(e.path()))
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        names.sort_by_key(|n| n.to_ascii_lowercase());
        names
    }

    pub fn exists(&self, name: &str) -> bool {
        self.list().iter().any(|n| n.eq_ignore_ascii_case(name))
    }

    /// Reads a profile (`0x528820`); `None` when it has no `.prf`.
    pub fn load(&self, name: &str, messages: [String; 10], preset: Controls) -> Option<Profile> {
        let folder = self.profiles().join(name);
        let text = crate::platform::fs::read_to_string(folder.join(format!("{name}.prf"))).ok()?;
        let keys = crate::platform::fs::read_to_string(folder.join("control.ctl"))
            .map(|t| Controls::parse(&t))
            .ok()
            .filter(|c| !c.keys.is_empty())
            .unwrap_or(preset);
        let mut p = Profile::parse(&text, messages, keys);
        p.name = name.to_string();
        Some(p)
    }

    /// Writes a profile and its key map (`0x528bb0`), and remembers it
    /// as the last one used.
    pub fn save(&self, p: &Profile) -> std::io::Result<()> {
        if p.name.is_empty() {
            return Ok(());
        }
        let folder = self.profiles().join(&p.name);
        crate::platform::fs::create_dir_all(&folder)?;
        crate::platform::fs::write(folder.join(format!("{}.prf", p.name)), p.text())?;
        crate::platform::fs::write(folder.join("control.ctl"), p.keys.text())?;
        let mut s = self.settings();
        s.set("MRUProfile", &p.name);
        self.save_settings(&s)
    }

    /// Makes a new profile's folder (`0x452990`); false when one of that
    /// name exists.
    pub fn create(&self, p: &Profile) -> std::io::Result<bool> {
        if self.exists(&p.name) {
            return Ok(false);
        }
        self.save(p)?;
        Ok(true)
    }

    /// Deletes a profile's folder.
    pub fn remove(&self, name: &str) -> std::io::Result<()> {
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return Ok(());
        }
        crate::platform::fs::remove_dir_all(self.profiles().join(name))
    }

    /// The machine's settings (the game's registry key).
    pub fn settings(&self) -> Settings {
        let text = crate::platform::fs::read_to_string(self.dir.join("settings.ini")).unwrap_or_default();
        Settings::parse(&text)
    }

    pub fn save_settings(&self, s: &Settings) -> std::io::Result<()> {
        crate::platform::fs::create_dir_all(&self.dir)?;
        crate::platform::fs::write(self.dir.join("settings.ini"), s.text())
    }
}

/// The folder saves go in unless `MM2_USER_DIR` names another.
pub fn user_dir() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        if let Some(home) = env("HOME") {
            return home.join("Library/Application Support/MM2");
        }
    } else if cfg!(windows) {
        if let Some(app) = env("APPDATA") {
            return app.join("MM2");
        }
    } else if let Some(data) = env("XDG_DATA_HOME") {
        return data.join("mm2");
    } else if let Some(home) = env("HOME") {
        return home.join(".local/share/mm2");
    }
    PathBuf::from("mm2-user")
}

/// The game's registry values, as `Name=Value` lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings(pub Vec<(String, String)>);

impl Settings {
    fn parse(text: &str) -> Settings {
        Settings(
            text.lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .collect(),
        )
    }

    fn text(&self) -> String {
        self.0.iter().map(|(k, v)| format!("{k}={v}\r\n")).collect()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn number(&self, key: &str, default: i64) -> i64 {
        self.get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    pub fn set(&mut self, key: &str, value: &str) {
        match self.0.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some((_, v)) => *v = value.to_string(),
            None => self.0.push((key.to_string(), value.to_string())),
        }
    }
}

/// INI text as the profile files hold it: sections of `Key=Value`,
/// values kept whole (a chat message may hold a `;`).
struct Ini(Vec<(String, Vec<(String, String)>)>);

struct Section<'a>(&'a [(String, String)]);

impl<'a> Section<'a> {
    fn get(&self, key: &str) -> Option<&'a str> {
        self.0
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

impl Ini {
    fn parse(text: &str) -> Ini {
        let mut out: Vec<(String, Vec<(String, String)>)> = Vec::new();
        for line in text.lines() {
            let t = line.trim();
            if let Some(name) = t.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                out.push((name.trim().to_string(), Vec::new()));
            } else if let Some((k, v)) = line.split_once('=') {
                if let Some((_, entries)) = out.last_mut() {
                    entries.push((k.trim().to_string(), v.trim_end_matches('\r').to_string()));
                }
            }
        }
        Ini(out)
    }

    fn section(&self, name: &str) -> Option<Section<'_>> {
        self.0
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, e)| Section(e))
    }
}

/// Whether `name` is one the new profile box takes (`0x452e00`).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= NAME_LENGTH
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || NAME_SYMBOLS.contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages() -> [String; 10] {
        std::array::from_fn(|i| format!("message {i}"))
    }

    #[test]
    fn round_trip() {
        let mut p = Profile::new("Rider_1", messages(), Controls::parse(""));
        p.game.tooltips = false;
        p.game.chat_mode = 2;
        p.sound.ui_volume = 37;
        p.graphics.terrain = 3;
        p.controls.pitch = 88;
        p.garage.cc = 125;
        p.garage.curve[1] = 4;
        p.garage.custom[1][1][3] = 33;
        p.garage.suspension[2] = 0.25;
        p.messages[3] = "Eat my dust".into();
        p.keys.set(0, 6, (0, 44));
        let q = Profile::parse(&p.text(), messages(), p.keys.clone());
        assert_eq!(p, q);
    }

    #[test]
    fn key_map() {
        let c = Controls::parse(
            "[Setup]\r\nCurrentInputDevice=1\r\n\r\n[Controller0]\r\nKey0=2,-3\r\nKey6=0,44\r\n",
        );
        assert_eq!(c.current, 1);
        assert_eq!(c.get(0, 0), (2, -3));
        assert_eq!(c.get(0, 6), (0, 44));
        assert_eq!(c.get(0, 7), (-1, 0));
        assert_eq!(Controls::parse(&c.text()), c);
    }

    #[test]
    fn store() {
        let dir = std::env::temp_dir().join(format!("mm2-profile-test-{}", std::process::id()));
        let store = Store { dir: dir.clone() };
        let p = Profile::new("Ann", messages(), Controls::parse(""));
        assert!(store.create(&p).unwrap());
        assert!(!store.create(&p).unwrap());
        assert_eq!(store.list(), ["Ann"]);
        assert_eq!(store.settings().get("MRUProfile"), Some("Ann"));
        let q = store.load("Ann", messages(), Controls::parse("")).unwrap();
        assert_eq!(q.name, "Ann");
        store.remove("Ann").unwrap();
        assert!(store.list().is_empty());
        let _ = crate::platform::fs::remove_dir_all(dir);
    }

    #[test]
    fn names_and_volume() {
        assert!(valid_name("Moto_X(2)"));
        assert!(!valid_name("no spaces"));
        assert!(!valid_name("sixteen_letters_"));
        let s = SoundOptions::default();
        assert!((s.ui_gain() - 10f32.powf(-0.1)).abs() < 1e-4);
    }
}
