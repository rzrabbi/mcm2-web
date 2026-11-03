//! Race sounds as the game's `Audio.cpp` (RaceSound) plays them.
//!
//! The engine note is not pitch-shifted: recorded samples from
//! `RES/ENGINE.RES` (listed in `Audio_16.ini`, or `Audio_08.ini` for 8-bit
//! sound) play whole, one after another, in a small state machine
//! (0x4e8720, 0x4e9aa0). Opening the throttle picks a torque band by speed,
//! which runs on into the power band, which alternates with sputters;
//! closing it plays the decel, which runs on into the idle loop. A change
//! of state rewrites the stream from the write cursor, so it is heard at
//! once. Each bike uses its own engine class's samples; the four bikes
//! nearest the camera have a voice.
//!
//! Every engine, landing, wreck and fall is a DirectSound 3D buffer: the
//! listener is the camera, moving with the followed bike, and each sound
//! falls off as min/distance between its minimum and maximum distance, is
//! silent beyond the maximum (`DSBCAPS_MUTE3DATMAXDISTANCE`, 0x4c1c90) and
//! is Doppler-shifted (0x4e81a0: distance factor 0.3048 m/ft, rolloff and
//! Doppler factor 1). Crashes, landings, the crowd and its reactions come
//! from `RES/AUDIO.RES`.

use crate::audio::{Audio, Voice};
use crate::frontend::EventKind;
use crate::race::Race;
use glam::Vec3;
use mm2_formats::{rs2, scn, wav::Sound};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// The engine sample lists of `Audio_16.ini`, in the order the game reads
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum List {
    Idle,
    LowTorque,
    MidTorque,
    HighTorque,
    PowerBand,
    Decel,
    Sputter,
    Rev,
}

const LISTS: [(List, &str); 8] = [
    (List::Idle, "Idle"),
    (List::LowTorque, "LowTorque"),
    (List::MidTorque, "MidTorque"),
    (List::HighTorque, "HighTorque"),
    (List::PowerBand, "PowerBand"),
    (List::Decel, "Decel"),
    (List::Sputter, "Sputter"),
    (List::Rev, "Rev"),
];

/// The ini sections, by class: below 250 cc, two-strokes from 250 cc, and
/// the four-strokes (bike +0x738, +0x737).
const CLASSES: [&str; 3] = ["125", "250", "400"];

/// ft/s to mph, as the game converts (0x55ca00).
const MPH: f32 = 0.68182;
/// Engines heard at once.
const VOICES: usize = 4;
/// Bikes farther than this, side to side or front to back, all count as
/// this far when the voices are handed out (0x4e91d0).
const FAR: f32 = 500.0;
/// In the air this long, the engine note changes (0x555868).
const AIR: f32 = 0.2;
/// The speed of sound in ft/s: DirectSound's 343 m/s over the game's
/// distance factor of 0.3048 m/ft.
const SOUND_SPEED: f32 = 343.0 / 0.3048;
/// Each buffer's minimum and maximum distance, ft (0x4e81a0): the engines,
/// wrecks and falls 50 to 500, the hard landing 10 to 500.
const ENGINE_RANGE: (f32, f32) = (50.0, 500.0);
const LANDING_RANGE: (f32, f32) = (10.0, 500.0);
/// The crowd's reactions wait this long between plays, s (0x555bf8,
/// 0x555800).
const OH_WAIT: f32 = 30.0;
const APPLAUSE_WAIT: f32 = 15.0;
/// A stunt worth this many points rings the bonus (0x555bfc).
const BONUS_POINTS: f32 = 10000.0;

/// The engine class of a bike of `cc` (0x4e7160).
pub fn class(cc: u32, four_stroke: bool) -> usize {
    if cc < 250 {
        0
    } else if four_stroke {
        2
    } else {
        1
    }
}

/// The player's sound options that the race reads.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// The whole race's volume, 0 to 1 (0 with In-Game Sound Effects off).
    pub gain: f32,
    /// Waypoint Chime.
    pub chime: bool,
    /// 16-bit sound: `Audio_16.ini`'s samples, else `Audio_08.ini`'s.
    pub sixteen_bit: bool,
}

impl Default for Options {
    /// A new profile's: full volume, the chime, 8-bit sound.
    fn default() -> Self {
        Options::from_profile(&crate::profile::SoundOptions::default())
    }
}

impl Options {
    /// From the player's Sound page: the In-Game Volume slider gives the
    /// primary buffer `volume × 25 − 2500` hundredths of a decibel, and
    /// nothing at 0 (0x4e81a0).
    pub fn from_profile(s: &crate::profile::SoundOptions) -> Options {
        let hundredths = s.game_volume.min(100) as f32 * 25.0 - 2500.0;
        let gain = if !s.effects || hundredths <= -2500.0 {
            0.0
        } else {
            10f32.powf(hundredths / 2000.0)
        };
        Options {
            gain,
            chime: s.waypoint_chime,
            // The browser always has the clean 16-bit samples: nothing
            // here is short of memory, and 8-bit adds audible hiss.
            sixteen_bit: s.sixteen_bit || cfg!(target_arch = "wasm32"),
        }
    }
}

/// Where the race is heard from: the camera, its right-hand side, and the
/// followed bike's velocity (0x4e84a0 gives the listener the camera's
/// place and the followed bike's velocity).
#[derive(Debug, Clone, Copy)]
pub struct Listener {
    pub eye: Vec3,
    pub right: Vec3,
    pub velocity: Vec3,
}

impl Listener {
    /// How a 3D buffer at `at`, moving at `velocity`, with minimum and
    /// maximum distances `range`, is heard: its volume, pan and Doppler
    /// pitch. The volume is min/distance between the two (rolloff 1), and
    /// nothing beyond the maximum: every 3D buffer is made with
    /// `DSBCAPS_MUTE3DATMAXDISTANCE` (0x4c1c90's flags `| 0x20010`). The
    /// pan follows the side the sound is on **[likely]**: DirectSound's
    /// software 3D pans by the sound's direction, and how hard is not in
    /// the game.
    pub fn hear(&self, at: Vec3, velocity: Vec3, range: (f32, f32)) -> (f32, f32, f32) {
        let to = at - self.eye;
        let distance = to.length();
        let volume = if distance > range.1.max(range.0) {
            0.0
        } else {
            range.0 / distance.max(range.0.max(1e-3))
        };
        let toward = to.normalize_or_zero();
        let pan = toward.dot(self.right);
        // Both closing speeds along the line between them.
        let pitch = (SOUND_SPEED + self.velocity.dot(toward))
            / (SOUND_SPEED + velocity.dot(toward)).max(1.0);
        (volume.min(1.0), pan, pitch)
    }
}

/// One engine class (`[125]`, `[250]` or `[400]`).
struct Bank {
    lists: Vec<Vec<Arc<Sound>>>,
    /// The tops of the low, mid and high torque bands, mph.
    bands: [i32; 3],
}

impl Bank {
    /// `divisor` thins every list of more than one sample, to no fewer
    /// than one (0x4e7160, +0x468).
    fn load(res: &Path, sixteen_bit: bool, class: &str, divisor: usize) -> Option<Bank> {
        let bytes = crate::read_archive(&res.join("ENGINE.RES"))
            .map_err(|e| crate::audio::warn(&e.to_string()))
            .ok()?;
        let ar = rs2::Archive::parse(&bytes).ok()?;
        let file = |name: &str| {
            ar.entries()
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(name))
                .map(|e| ar.data(e))
        };
        let ini_name = if sixteen_bit {
            "Audio_16.ini"
        } else {
            "Audio_08.ini"
        };
        let ini = scn::Ini::parse(&String::from_utf8_lossy(file(ini_name)?));
        let section = ini.section(class)?;
        let lists = LISTS
            .iter()
            .map(|(_, name)| {
                let mut total: usize = section
                    .get(&format!("Total{name}"))
                    .and_then(|t| t.trim().parse().ok())
                    .unwrap_or(0);
                if total > 1 {
                    total = (total / divisor.max(1)).max(1);
                }
                (1..=total.min(10))
                    .filter_map(|i| section.get(&format!("{name}_{i}")))
                    .filter_map(|f| mm2_formats::wav::parse(file(f)?).ok())
                    .map(Arc::new)
                    .collect()
            })
            .collect();
        let band = |key: &str, default: i32| {
            section
                .get(key)
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(default)
        };
        Some(Bank {
            lists,
            bands: [
                band("LowTorqueMaxSpeed", 20),
                band("MidTorqueMaxSpeed", 55),
                band("HighTorqueMaxSpeed", 100),
            ],
        })
    }

    fn list(&self, l: List) -> &[Arc<Sound>] {
        let i = LISTS.iter().position(|(k, _)| *k == l).unwrap_or(0);
        &self.lists[i]
    }
}

/// What an engine is playing (the game's states 1–6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Idle,
    /// A low, mid or high torque sample.
    Band,
    Power,
    Sputter,
    Decel,
    Rev,
}

impl State {
    /// What follows when the sample ends; `None` loops it.
    fn next(self) -> Option<State> {
        match self {
            State::Idle => None,
            State::Band | State::Sputter => Some(State::Power),
            State::Power => Some(State::Sputter),
            State::Decel | State::Rev => Some(State::Idle),
        }
    }

    /// The list a state plays from (a band's own list is picked by speed).
    fn list(self) -> List {
        match self {
            State::Idle => List::Idle,
            State::Band => List::HighTorque,
            State::Power => List::PowerBand,
            State::Sputter => List::Sputter,
            State::Decel => List::Decel,
            State::Rev => List::Rev,
        }
    }
}

/// Whether the sample queued after `state`'s loops: only the idle does.
fn t_loops(state: State) -> bool {
    state.next() == Some(State::Idle)
}

fn seconds(s: &Sound) -> f32 {
    s.frames() as f32 / s.rate.max(1) as f32
}

/// One bike's engine and crash sounds (a 0x54-byte record at +0x98).
#[derive(Default)]
struct Engine {
    /// The engine class, an index into `CLASSES`.
    class: usize,
    state: State,
    /// The sample playing, and how far into it. Without a voice the
    /// stream stands still (0x4e9aa0 runs only on a buffer).
    sound: Option<Arc<Sound>>,
    clock: f32,
    /// The sample queued to follow it.
    then: Option<Arc<Sound>>,
    voice: Option<Voice>,
    air: f32,
    airborne: bool,
    /// The throttle was let go on the ground, or in the air.
    released: bool,
    released_in_air: bool,
    racing: bool,
    crashed: bool,
    /// The wreck and fall last played for this bike, 1 to 6 (0: none, and
    /// so is the first: the game keeps the index and tests it for 0).
    wreck: usize,
    fall: usize,
}

/// A one-shot 3D sound still playing, where it was started.
struct Placed {
    voice: Voice,
    at: Vec3,
    range: (f32, f32),
}

/// What a scene sound hangs on, found each frame by the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Site {
    /// A fixed place: a static model or a `[StaticSoundN]`.
    Fixed(Vec3),
    /// An animated object and one of its frames (its root when none).
    Object(usize, Option<usize>),
    /// A scripted vehicle, by index into the track's traffic.
    Car(usize),
}

/// A scene's sound emitter (SoundEmitter, 0x402080–0x402a10): it plays
/// while the camera is within 1.2 times its maximum distance, side to side
/// and front to back (0x4024d0), or once, the first time the camera comes
/// within its one-shot distance. Each time it comes in range it rolls its
/// trigger chance once (0x4027b0). It is a 3D buffer with the scene's
/// minimum and maximum distances, moving with what it hangs on (0x402870).
struct Emitter {
    spec: scn::SoundSpec,
    site: Site,
    sound: Option<Arc<Sound>>,
    voice: Option<Voice>,
    /// Where it was last frame, for its velocity.
    last: Option<Vec3>,
    /// It has rolled since it came in range (+0x1a0 bit 4), and a one-shot
    /// has played (bit 0).
    rolled: bool,
    fired: bool,
}

/// How far out a scene sound keeps playing, in maximum distances
/// (0x5555e8).
const EMITTER_REACH: f32 = 1.2;

pub struct RaceSounds {
    /// By class; only the classes in the race are loaded.
    banks: [Option<Bank>; 3],
    effects: HashMap<String, Arc<Sound>>,
    engines: Vec<Engine>,
    /// The class of any racer not given one: the player's.
    default_class: usize,
    options: Options,
    event: EventKind,
    /// The scene has a crowd (`[Sounds] CrowdPresent`): its loop and its
    /// reactions are loaded.
    crowd_present: bool,
    crowd: Option<Voice>,
    /// The crowd loop's level, hundredths of a dB (Ctrl+C: 0, −500,
    /// −10000).
    crowd_level: f32,
    /// The hard landing is one buffer, placed on the followed bike.
    landing: Option<Voice>,
    /// The six wrecks and six falls are one buffer each, shared by every
    /// bike: one still playing is not started again.
    wrecks: [Option<Voice>; 6],
    falls: [Option<Voice>; 6],
    placed: Vec<Placed>,
    seed: u32,
    /// The player's gates and laps passed, and place, for the chime and
    /// the crowd (+0x1294, +0x1290, +0x129c).
    gates: Option<(u32, u32)>,
    place: Option<usize>,
    /// Seconds since the crowd last said "oh", applauded a pass, and
    /// applauded a stunt (+0x12a4, +0x12a8, +0x12ac).
    since_oh: f32,
    since_pass: f32,
    since_stunt: f32,
    /// The last stunt score seen, for the bonus.
    popup: Option<(f32, f32, f32)>,
    /// When Moto-Tag's IT last changed hands, for its call.
    tagged: Option<f32>,
    /// The scene's ambient sounds.
    emitters: Vec<Emitter>,
    /// Only the engines sound: no landings, crashes, chimes or crowd (for
    /// recording the engine note on its own, as the game's engine buffer
    /// is captured to compare with).
    pub engines_only: bool,
}

impl RaceSounds {
    /// The sounds from the game's `res` folder for a race of `classes`
    /// (each racer's engine class, the player's at `player`), the event,
    /// and whether the scene has a crowd.
    pub fn load(
        res: &Path,
        audio: &Audio,
        classes: &[usize],
        player: usize,
        event: EventKind,
        crowd_present: bool,
        options: Options,
    ) -> RaceSounds {
        // With 16-bit sound and more than one class in the race, the other
        // classes keep half their samples (a quarter with three, when the
        // player's keeps half), the game's way of saving memory (0x4e7160
        // with its fifth argument 6).
        let present: Vec<usize> = (0..3).filter(|c| classes.contains(c)).collect();
        let mine = classes.get(player).copied().unwrap_or(1);
        let divisor = |c: usize| match (options.sixteen_bit, present.len()) {
            _ if cfg!(target_arch = "wasm32") => 1,
            (true, 2) if c != mine => 2,
            (true, 3) if c != mine => 4,
            (true, 3) => 2,
            _ => 1,
        };
        let mut banks: [Option<Bank>; 3] = Default::default();
        for &c in &present {
            banks[c] = Bank::load(res, options.sixteen_bit, CLASSES[c], divisor(c));
            if banks[c].is_none() {
                crate::audio::warn(&format!("no [{}] engine sounds in RES/ENGINE.RES", CLASSES[c]));
            }
        }
        let mut effects = HashMap::new();
        if let Ok(bytes) = crate::read_archive(&res.join("AUDIO.RES")) {
            if let Ok(ar) = rs2::Archive::parse(&bytes) {
                for e in ar.entries() {
                    if let Ok(s) = mm2_formats::wav::parse(ar.data(e)) {
                        effects.insert(e.name.to_ascii_lowercase(), Arc::new(s));
                    }
                }
            }
        }
        let engines = classes
            .iter()
            .map(|&class| Engine {
                class,
                ..Default::default()
            })
            .collect();
        let mut sounds = RaceSounds {
            banks,
            effects,
            engines,
            default_class: mine,
            options,
            event,
            crowd_present,
            crowd: None,
            crowd_level: 0.0,
            landing: None,
            wrecks: [None; 6],
            falls: [None; 6],
            placed: Vec::new(),
            seed: 0x2545_f491,
            gates: None,
            place: None,
            since_oh: 0.0,
            since_pass: 0.0,
            since_stunt: 0.0,
            popup: None,
            tagged: None,
            emitters: Vec::new(),
            engines_only: false,
        };
        // The crowd loops from the start, flat (0x4e81a0).
        if crowd_present {
            sounds.crowd = sounds.play(audio, "crowdloop.wav", true);
        }
        sounds
    }

    /// The game's `rand()`, near enough: the picks only need to vary.
    fn random(&mut self, n: usize) -> usize {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed as usize % n.max(1)
    }

    /// A random sample from one of a class's engine lists.
    fn pick(&mut self, class: usize, list: List) -> Option<Arc<Sound>> {
        let n = self.banks[class].as_ref().map_or(0, |b| b.list(list).len());
        let k = self.random(n);
        self.banks[class]
            .as_ref()
            .and_then(|b| b.list(list).get(k).cloned())
    }

    /// Ctrl+S: the race's sounds off or on again (0x4e9250); off, every
    /// voice falls silent.
    pub fn set_on(&mut self, audio: &Audio, on: bool, gain: f32) {
        self.options.gain = if on { gain } else { 0.0 };
        if !on {
            audio.stop_all();
            self.crowd = None;
            self.landing = None;
        } else if self.crowd_present && self.crowd.is_none() {
            self.crowd = self.play(audio, "crowdloop.wav", true);
        }
        self.place_crowd(audio);
    }

    /// Ctrl+C in a stadium: the crowd on, reduced (−5 dB) and off, round
    /// again (0x4e9250); the string the game says.
    pub fn step_crowd(&mut self, audio: &Audio) -> Option<u32> {
        if !self.crowd_present {
            return None;
        }
        let (level, said) = match self.crowd_level {
            l if l >= 0.0 => (-500.0, 5312),
            l if l > -10000.0 => (-10000.0, 5314),
            _ => (0.0, 5313),
        };
        self.crowd_level = level;
        self.place_crowd(audio);
        Some(said)
    }

    fn place_crowd(&self, audio: &Audio) {
        if let Some(v) = self.crowd {
            let level = if self.crowd_level <= -10000.0 { 0.0 } else { 10f32.powf(self.crowd_level / 2000.0) };
            audio.place(v, self.options.gain * level, 0.0, 1.0);
        }
    }

    /// Plays one of the race's flat (2D) sounds at full volume.
    fn play(&self, audio: &Audio, name: &str, looping: bool) -> Option<Voice> {
        self.effects
            .get(name)
            .filter(|_| self.options.gain > 0.0 && !self.engines_only)
            .map(|s| audio.play(s, self.options.gain, looping))
    }

    /// One of the crowd's reactions, which only a scene with a crowd has.
    fn cheer(&self, audio: &Audio, name: &str) {
        if self.crowd_present {
            self.play(audio, name, false);
        }
    }

    /// Starts a 3D one-shot at `at`, as heard from `l`.
    fn play_at(
        &mut self,
        audio: &Audio,
        name: &str,
        at: Vec3,
        range: (f32, f32),
        l: &Listener,
    ) -> Option<Voice> {
        let s = self
            .effects
            .get(name)
            .filter(|_| self.options.gain > 0.0 && !self.engines_only)?;
        let (volume, pan, pitch) = l.hear(at, Vec3::ZERO, range);
        let v = audio.play(s, volume * self.options.gain, false);
        audio.place(v, volume * self.options.gain, pan, pitch);
        self.placed.push(Placed {
            voice: v,
            at,
            range,
        });
        Some(v)
    }

    /// A wreck or fall for a bike at `at` (0x4e9050, 0x4e9110): one of the
    /// six not already playing, ten tries at random, or none.
    fn crash_sound(&mut self, audio: &Audio, fall: bool, at: Vec3, l: &Listener) -> usize {
        for _ in 0..10 {
            let k = self.random(6);
            let slot = if fall { self.falls[k] } else { self.wrecks[k] };
            if slot.is_some_and(|v| audio.playing(v)) {
                continue;
            }
            let name = if fall {
                format!("fall{:02}.wav", k + 1)
            } else {
                format!("wreck{:02}.wav", k + 1)
            };
            let v = self.play_at(audio, &name, at, ENGINE_RANGE, l);
            if fall {
                self.falls[k] = v;
            } else {
                self.wrecks[k] = v;
            }
            return k;
        }
        0
    }

    /// Follows the race as heard from `l` (0x4e84a0, 0x4e8720).
    pub fn update(&mut self, audio: &Audio, race: &Race, l: &Listener, dt: f32) {
        let class = self.default_class;
        self.engines.resize_with(race.racers.len(), || Engine {
            class,
            ..Default::default()
        });
        self.since_oh += dt;
        self.since_pass += dt;
        self.since_stunt += dt;
        let player = race.player;

        // The four nearest the camera get a voice, however far (0x4e8720).
        let mut near: Vec<(usize, f32)> = race
            .racers
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let to = r.bike.position() - l.eye;
                let far = to.x.abs() > FAR || to.z.abs() > FAR;
                (
                    i,
                    if far {
                        FAR
                    } else {
                        Vec3::new(to.x, 0.0, to.z).length()
                    },
                )
            })
            .collect();
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        near.truncate(VOICES);

        for i in 0..race.racers.len() {
            let r = &race.racers[i];
            let bike = &r.bike;
            let mut e = std::mem::take(&mut self.engines[i]);
            let was_airborne = e.airborne;
            e.airborne = bike.airborne();
            let heard = near.iter().any(|n| n.0 == i);

            // The engine's state machine (0x4e8720), for every bike.
            let gas = !bike.crashed() && r.controls.throttle > 0.1;
            let racing = race.gate_down();
            let mph = bike.velocity().length() * MPH;
            let mut change = None;
            if !bike.airborne() {
                e.air = 0.0;
                e.released_in_air = false;
                // Touching down, the followed bike thumps (LandHard01),
                // unless the last thump still plays.
                if i == player && was_airborne && !self.landing.is_some_and(|v| audio.playing(v)) {
                    self.landing =
                        self.play_at(audio, "landhard01.wav", bike.position(), LANDING_RANGE, l);
                }
                if !gas {
                    e.released = true;
                }
                if !racing {
                    // Revving at the gate: once per blip.
                    if gas && (e.released || e.racing) {
                        e.released = false;
                        change = Some((State::Rev, List::Rev));
                    }
                } else if !gas {
                    if matches!(e.state, State::Band | State::Power | State::Sputter) {
                        change = Some((State::Decel, List::Decel));
                    }
                } else if e.state == State::Rev {
                    change = Some((State::Band, List::HighTorque));
                } else if matches!(e.state, State::Idle | State::Decel) {
                    let bands = self.banks[e.class]
                        .as_ref()
                        .map_or([20, 55, 100], |b| b.bands);
                    let mph = mph as i32;
                    change = Some(if mph < bands[0] {
                        (State::Band, List::LowTorque)
                    } else if mph < bands[1] {
                        (State::Band, List::MidTorque)
                    } else if mph >= bands[2] {
                        (State::Power, List::PowerBand)
                    } else {
                        (State::Band, List::HighTorque)
                    });
                }
            } else {
                e.air += dt;
                if e.air >= AIR {
                    if !gas {
                        e.released_in_air = true;
                    }
                    if !matches!(e.state, State::Decel | State::Rev | State::Idle) {
                        change = Some((State::Decel, List::Decel));
                    }
                    // Blipping the throttle in the air.
                    if e.released_in_air && gas {
                        e.released_in_air = false;
                        change = Some((State::Rev, List::Rev));
                    }
                }
            }
            e.racing = racing;
            if let Some((state, list)) = change {
                e.state = state;
                e.sound = self.pick(e.class, list);
                e.clock = 0.0;
                e.then = state.next().and_then(|n| self.pick(e.class, n.list()));
                if let (Some(v), Some(s)) = (e.voice, &e.sound) {
                    audio.switch(v, s, e.then.is_none());
                    if let Some(t) = &e.then {
                        audio.then(v, t, t_loops(state));
                    }
                }
            }

            // A crashed bike: the crowd's reaction as it goes down, a
            // wreck each time the bike lands, and the rider's grunts while
            // its body touches the ground (0x4e8720's second loop).
            if bike.crashed() {
                // Thrown back off the world's edge, the player hears the
                // launch (0x4e8720, crash 0xc).
                if !e.crashed && i == player && bike.launched() {
                    self.play(audio, "launch.wav", false);
                }
                if !e.crashed && i == player && self.event == EventKind::Supercross {
                    let place = race.position();
                    if place == 1 {
                        self.cheer(audio, "boo_01.wav");
                    } else if self.since_oh > OH_WAIT {
                        self.cheer(audio, "oh_01.wav");
                        self.since_oh = 0.0;
                    }
                }
                let free = |slot: Option<Voice>| !slot.is_some_and(|v| audio.playing(v));
                if was_airborne
                    && !bike.airborne()
                    && (e.wreck == 0 || free(self.wrecks[e.wreck - 1]))
                {
                    e.wreck = self.crash_sound(audio, false, bike.position(), l);
                }
                if bike.rider_touching() && (e.fall == 0 || free(self.falls[e.fall - 1])) {
                    e.fall = self.crash_sound(audio, true, bike.position(), l);
                }
            } else {
                e.wreck = 0;
                e.fall = 0;
            }
            e.crashed = bike.crashed();

            if !heard {
                if let Some(v) = e.voice.take() {
                    audio.stop(v);
                }
                self.engines[i] = e;
                continue;
            }
            if e.sound.is_none() {
                e.sound = self.pick(e.class, e.state.list());
                e.then = e.state.next().and_then(|n| self.pick(e.class, n.list()));
                e.clock = 0.0;
            }
            // A voice the mixer let go (the page stalled for longer than the
            // sample queued behind the one playing) is started again where
            // the stream has got to, or the bike would stay silent.
            if e.voice.is_some_and(|v| !audio.playing(v)) {
                e.voice = None;
            }
            if e.voice.is_none() && self.options.gain > 0.0 {
                // A new voice takes up the stream where it stood.
                if let Some(s) = &e.sound {
                    let v = audio.play_from(s, e.clock, 0.0, e.then.is_none());
                    if let Some(t) = &e.then {
                        audio.then(v, t, t_loops(e.state));
                    }
                    e.voice = Some(v);
                }
            }
            // Each sample runs on into the next (0x4e9aa0).
            e.clock += dt;
            while let (Some(s), Some(next)) = (&e.sound, e.state.next()) {
                let length = seconds(s);
                if e.clock < length || length <= 0.0 {
                    break;
                }
                e.clock -= length;
                e.state = next;
                e.sound = e.then.take();
                e.then = next.next().and_then(|n| self.pick(e.class, n.list()));
                if let (Some(v), Some(t)) = (e.voice, &e.then) {
                    audio.then(v, t, t_loops(next));
                }
            }
            if e.state == State::Idle {
                if let Some(s) = &e.sound {
                    let length = seconds(s);
                    if length > 0.0 {
                        e.clock %= length;
                    }
                }
            }
            if let Some(v) = e.voice {
                let (volume, pan, pitch) = l.hear(bike.position(), bike.velocity(), ENGINE_RANGE);
                audio.place(v, volume * self.options.gain, pan, pitch);
            }
            self.engines[i] = e;
        }

        // The hard landing follows the bike; the wrecks and falls stay
        // where they started while the camera moves.
        if let Some(v) = self.landing {
            let bike = &race.racers[player].bike;
            let (volume, pan, pitch) = l.hear(bike.position(), Vec3::ZERO, LANDING_RANGE);
            audio.place(v, volume * self.options.gain, pan, pitch);
        }
        self.placed.retain(|p| audio.playing(p.voice));
        for p in &self.placed {
            if Some(p.voice) == self.landing {
                continue;
            }
            let (volume, pan, pitch) = l.hear(p.at, Vec3::ZERO, p.range);
            audio.place(p.voice, volume * self.options.gain, pan, pitch);
        }

        // The player's gates and laps ring the waypoint chime, when it is
        // on.
        let me = &race.racers[player];
        let gates = (me.timing.gates, me.timing.crossings);
        if let Some(was) = self.gates {
            if self.options.chime && (gates.0 > was.0 || gates.1 > was.1) {
                self.play(audio, "waypoint.wav", false);
            }
        }
        self.gates = Some(gates);

        // Passing a rider: applause, always for taking the lead
        // (Applause03), otherwise at most every 15 s (Applause01; the
        // game's pick of Applause02 needs rand() to give 0).
        let place = race.position();
        if let Some(was) = self.place {
            if place < was {
                if place == 1 {
                    self.cheer(audio, "applause03.wav");
                } else if self.since_pass > APPLAUSE_WAIT {
                    self.cheer(audio, "applause01.wav");
                    self.since_pass = 0.0;
                }
            }
        }
        self.place = Some(place);

        // A stunt landed (0x4ea560): the bonus for 10,000 points in the
        // Stunt Quarry or Moto-Tag, and in a stadium the crowd applauds,
        // at most every 15 s.
        if race.popup != self.popup {
            if let Some((_, base, m)) = race.popup {
                if base * m >= BONUS_POINTS
                    && matches!(self.event, EventKind::Stunt | EventKind::Tag)
                {
                    self.play(audio, "bonus.wav", false);
                }
                if self.event == EventKind::Supercross && self.since_stunt > APPLAUSE_WAIT {
                    self.cheer(audio, "applause01.wav");
                    self.since_stunt = 0.0;
                }
            }
            self.popup = race.popup;
        }

        // Moto-Tag calls out each new IT (`It.wav`, 0x4ea5f0).
        let tagged = race.tag.as_ref().and_then(|t| t.passed);
        if tagged != self.tagged && tagged.is_some() {
            self.play(audio, "it.wav", false);
        }
        self.tagged = tagged;
    }

    /// Gives the race the scene's sounds and what each hangs on.
    pub fn set_scene(&mut self, sounds: impl IntoIterator<Item = (scn::SoundSpec, Site)>) {
        self.emitters = sounds
            .into_iter()
            .map(|(spec, site)| Emitter {
                sound: self.effects.get(&spec.file.to_ascii_lowercase()).cloned(),
                spec,
                site,
                voice: None,
                last: None,
                rolled: false,
                fired: false,
            })
            .collect();
    }

    /// Plays the scene's sounds as heard from `l`; `at` says where a site
    /// is now (0x403100 each frame).
    pub fn ambient(
        &mut self,
        audio: &Audio,
        l: &Listener,
        at: impl Fn(Site) -> Option<Vec3>,
        dt: f32,
    ) {
        for k in 0..self.emitters.len() {
            let Some(here) = at(self.emitters[k].site) else {
                continue;
            };
            let e = &mut self.emitters[k];
            let velocity = match e.last {
                Some(last) if dt > 0.0 => (here - last) / dt,
                _ => Vec3::ZERO,
            };
            e.last = Some(here);
            let to = here - l.eye;
            let (dx, dz) = (to.x.abs(), to.z.abs());
            let distance = (dx * dx + dz * dz).sqrt();
            let active = if e.spec.one_shot {
                !e.fired && distance <= e.spec.one_shot_distance
            } else {
                distance < e.spec.max_distance * EMITTER_REACH
            };
            if !active {
                // Out of range: stopped, and it rolls again when back.
                if let Some(v) = e.voice.take() {
                    audio.stop(v);
                }
                e.rolled = false;
                continue;
            }
            if !e.rolled {
                e.rolled = true;
                let roll = self.random(32768) as f32 / 32768.0 * 100.0;
                let e = &mut self.emitters[k];
                if roll <= e.spec.trigger_percent && self.options.gain > 0.0 {
                    if e.spec.one_shot {
                        e.fired = true;
                    }
                    if let Some(s) = &e.sound {
                        e.voice = Some(audio.play(s, 0.0, e.spec.looping));
                    }
                }
            }
            let e = &mut self.emitters[k];
            if let Some(v) = e.voice {
                let range = (e.spec.min_distance, e.spec.max_distance);
                let (volume, pan, pitch) = l.hear(here, velocity, range);
                audio.place(v, volume * self.options.gain, pan, pitch);
            }
        }
    }

    /// The podium: the engines stop and the crowd cheers (0x45e490's
    /// `CrowdLoop.wav`).
    pub fn podium(&mut self, audio: &Audio) {
        for e in &mut self.engines {
            if let Some(v) = e.voice.take() {
                audio.stop(v);
            }
        }
        if self.crowd.is_none() {
            self.crowd = self
                .effects
                .get("crowdloop.wav")
                .map(|s| audio.play(s, self.options.gain, true));
        }
    }

    /// Silences everything (the race is over).
    pub fn stop(&mut self, audio: &Audio) {
        for e in &mut self.engines {
            if let Some(v) = e.voice.take() {
                audio.stop(v);
            }
        }
        if let Some(v) = self.crowd.take() {
            audio.stop(v);
        }
        for p in self.placed.drain(..) {
            audio.stop(p.voice);
        }
        for e in &mut self.emitters {
            if let Some(v) = e.voice.take() {
                audio.stop(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With the game's files beside the engine, every list has samples.
    #[test]
    fn engine_lists_load() {
        let res = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../GAME/RES");
        if !crate::platform::fs::is_file(res.join("ENGINE.RES")) {
            return;
        }
        for sixteen in [true, false] {
            let bank = Bank::load(&res, sixteen, "250", 1).expect("the [250] engine");
            for (list, name) in LISTS {
                let samples = bank.list(list);
                assert!(!samples.is_empty(), "{name}");
                assert!(samples.iter().all(|s| s.frames() > 0), "{name}");
            }
            // The 250s change band later than the 125s (55 mph).
            assert_eq!(bank.bands, [20, 65, 100]);
        }
        // Thinned to half, a list of 8 keeps 4 and a list of 1 keeps 1.
        let half = Bank::load(&res, true, "250", 2).unwrap();
        assert_eq!(half.list(List::LowTorque).len(), 4);
        assert_eq!(half.list(List::Idle).len(), 1);
    }

    #[test]
    fn classes_follow_the_engine() {
        assert_eq!(class(125, false), 0);
        assert_eq!(class(250, false), 1);
        assert_eq!(class(500, false), 1);
        assert_eq!(class(400, true), 2);
        assert_eq!(class(600, true), 2);
    }

    #[test]
    fn falls_off_and_shifts_like_directsound() {
        let l = Listener {
            eye: Vec3::ZERO,
            right: Vec3::X,
            velocity: Vec3::ZERO,
        };
        // Full inside the minimum distance, min/d out to the maximum, and
        // silent beyond it.
        assert_eq!(
            l.hear(Vec3::new(0.0, 0.0, 20.0), Vec3::ZERO, ENGINE_RANGE)
                .0,
            1.0
        );
        assert!(
            (l.hear(Vec3::new(0.0, 0.0, 100.0), Vec3::ZERO, ENGINE_RANGE)
                .0
                - 0.5)
                .abs()
                < 1e-6
        );
        assert!(
            (l.hear(Vec3::new(0.0, 0.0, 500.0), Vec3::ZERO, ENGINE_RANGE)
                .0
                - 0.1)
                .abs()
                < 1e-6
        );
        assert_eq!(
            l.hear(Vec3::new(0.0, 0.0, 900.0), Vec3::ZERO, ENGINE_RANGE)
                .0,
            0.0
        );
        // On the right, panned right.
        assert!(
            l.hear(Vec3::new(30.0, 0.0, 0.0), Vec3::ZERO, ENGINE_RANGE)
                .1
                > 0.99
        );
        // Coming at 88 ft/s (60 mph), about 8 % higher; going, lower.
        let at = Vec3::new(0.0, 0.0, 100.0);
        let coming = l.hear(at, Vec3::new(0.0, 0.0, -88.0), ENGINE_RANGE).2;
        let going = l.hear(at, Vec3::new(0.0, 0.0, 88.0), ENGINE_RANGE).2;
        assert!(
            (coming - 1.085).abs() < 0.01 && going < 1.0,
            "{coming} {going}"
        );
        // Riding along with it, no shift.
        let along = Listener {
            velocity: Vec3::new(0.0, 0.0, 88.0),
            ..l
        };
        assert!((along.hear(at, Vec3::new(0.0, 0.0, 88.0), ENGINE_RANGE).2 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn game_volume_in_hundredths_of_a_decibel() {
        let mut s = crate::profile::SoundOptions::default();
        assert!((Options::from_profile(&s).gain - 1.0).abs() < 1e-6);
        s.game_volume = 60;
        // -1000 hundredths: -10 dB.
        assert!((Options::from_profile(&s).gain - 10f32.powf(-0.5)).abs() < 1e-4);
        s.game_volume = 0;
        assert_eq!(Options::from_profile(&s).gain, 0.0);
        s.game_volume = 100;
        s.effects = false;
        assert_eq!(Options::from_profile(&s).gain, 0.0);
    }
}
