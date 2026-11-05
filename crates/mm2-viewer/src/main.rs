//! `mm2-viewer`: ride or fly around a Motocross Madness 2 track.
//!
//! ```text
//! mm2-viewer <GAME/TERAFORM/.../TRACK.ENV>
//! mm2-viewer                      // the game's menus (GAME found above here)
//! mm2-viewer <GAME folder>
//! mm2-viewer <TRACK.ENV> [--laps 3] [--riders 4]
//! mm2-viewer <TRACK.ENV> --screenshot out.png [--size 1280x720]
//!     [--camera x,y,z,yaw,pitch]   // game coordinates and degrees
//!     [--time seconds]             // how far into the animations
//!     [--ride seconds [--steer -1..1] [--front]]  // race first; the computer rides
//!                                       // for you unless you steer
//! mm2-viewer <TRACK.ENV> --hires <dir>   // with upscaled textures
//! mm2-viewer ... --enhanced[=no-bloom,no-ao,...]  // modern graphics (F3 compares)
//! mm2-viewer ... --fullscreen [--no-vsync]  // or Alt+Enter while playing
//! mm2-viewer ... --show-fps                // or F2: the frame rate in the title and terminal
//! ```
//!
//! Four bikes and riders from `RES/BIKE.RES` race from the start line in
//! the track's `.tdf`: the player on one, the computer on the others,
//! following the course. The window title shows the lap, place and times.
//!
//! Riding: W/Up throttle, S/Down brake, A/D or Left/Right to steer; in
//! the air, throttle and brake tip the bike back and forward (the game's
//! gas and brake gyro options).
//! Xbox and PlayStation pads and flight sticks work too (see `pad`). R restarts the race, P lets
//! the computer ride for you, Tab switches to the free camera. Free camera: hold the right mouse button and move to
//! look, WASD to fly, Q/E or Space/Ctrl to go down/up, Shift to go faster.
//! Esc quits.

// No console window on Windows (it showed behind the game); the launcher
// keeps stderr in last-run.log.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// In the browser stderr goes nowhere: what the game says there goes to the
// page's console, so that a texture or a model that failed to load shows.
macro_rules! eprintln {
    ($($arg:tt)*) => {
        $crate::platform::log(&format!($($arg)*))
    };
}

mod platform;
mod eco;
mod enhance;
mod grass;
mod ruts;
mod dust;
mod rain;
mod smooth;
mod scenery;
mod crowd;
mod cull;
mod materials;
mod graphics;
mod post;
mod finish;
mod podium;
mod audio;
mod camera;
mod countdown;
mod cues;
mod reverb;
mod frontend;
mod gates;
mod ground;
mod hires;
mod hud;
mod input;
#[cfg(feature = "gamepad")]
mod pad;
mod morph;
mod movie;
mod net;
mod objects;
mod particles;
mod popup;
mod preview;
mod procircuit;
mod profile;
mod race;
mod renderer;
mod replay;
mod ride;
mod riding;
mod roster;
mod screens;
mod sky;
mod sound;
mod sprites;
mod tag;
mod traffic;
mod ui;
#[cfg(windows)]
mod winwatch;

use camera::Camera;
use glam::{Mat4, Vec3, Vec4};
use ground::TrackGround;
use hires::HiRes;
use mm2_formats::trn::Terrain;
use mm2_formats::{faoe, rs2, scn, tdf, trn, Container};
use mm2_physics::spec::{self, Spec};
use objects::ObjectMesh;
use race::Race;
use renderer::TerrainRenderer;
use ride::Controls;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use web_time::Instant;
use tdf::TrackDef;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

type BoxError = Box<dyn std::error::Error>;

/// In the browser nothing starts until the page has handed over the game's
/// files; see `web`.
#[cfg(target_arch = "wasm32")]
fn main() {
    console_error_panic_hook::set_once();
}

/// What the page calls: it gives the game's files one by one, then starts.
#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    /// The folder in the file tree that holds the player's copy of the game.
    const GAME: &str = "game";

    /// Adds one of the game's files; `path` is relative to the game folder.
    #[wasm_bindgen]
    pub fn add_file(path: &str, data: &[u8]) {
        let path = std::path::Path::new(GAME).join(path);
        crate::platform::fs::insert(&path, data.to_vec());
    }

    /// Gives back a file the player saved on an earlier visit; `path` is the
    /// whole path it was saved under.
    #[wasm_bindgen]
    pub fn add_saved_file(path: &str, data: &[u8]) {
        crate::platform::fs::restore(std::path::Path::new(path), data.to_vec());
    }

    /// Lists a file whose bytes the page fetches later, when the game
    /// asks for its folder; `path` is relative to the game folder.
    #[wasm_bindgen]
    pub fn add_remote_file(path: &str) {
        crate::platform::fs::insert_remote(&std::path::Path::new(GAME).join(path));
    }

    /// Tells the game that the page could not fetch a folder it asked for
    /// (`mm2Fetch`), so the game asks again on a later frame; `folder` is
    /// the one `mm2Fetch` was given.
    #[wasm_bindgen]
    pub fn fetch_failed(folder: &str) {
        crate::platform::fs::fetch_failed(folder);
    }

    /// Adds a file the game looks for outside its own folder, such as the
    /// system fonts; `path` is the whole path the game reads.
    #[wasm_bindgen]
    pub fn add_system_file(path: &str, data: &[u8]) {
        crate::platform::fs::insert(std::path::Path::new(path), data.to_vec());
    }

    /// The upgrades a list for `--enhanced=` turns on, as JSON, so the
    /// page can show them: the field names are the list's own (`mud_start`
    /// is `mud=`), `time` and `pixels` null for the scene's own.
    #[wasm_bindgen]
    pub fn enhanced_settings(list: &str) -> Result<String, JsValue> {
        let e = crate::graphics::Enhanced::parse(list).map_err(|e| JsValue::from_str(&e))?;
        let weather = match e.weather {
            crate::graphics::Weather::Clear => "clear",
            crate::graphics::Weather::Overcast => "overcast",
            crate::graphics::Weather::Rain => "rain",
        };
        let or_null = |v: Option<String>| v.unwrap_or_else(|| "null".into());
        Ok(format!(
            concat!(
                r#"{{"msaa":{},"lighting":{},"shadows":{},"distance":{},"bloom":{},"ao":{},"sky":{},"grade":{},"#,
                r#""ground":{},"grass":{},"clouds":{},"ruts":{},"dust":{},"bikes":{},"shafts":{},"cinematic":{},"#,
                r#""smooth":{},"scenery":{},"mud":{},"surfaces":{},"mud_start":{},"detail":{},"weather":"{}","#,
                r#""time":{},"pixels":{}}}"#
            ),
            e.msaa, e.lighting, e.shadows, e.distance, e.bloom, e.ao, e.sky, e.grade,
            e.ground, e.grass, e.clouds, e.ruts, e.dust, e.bikes, e.shafts, e.cinematic,
            e.smooth, e.scenery, e.mud, e.surfaces, e.mud_start, e.detail, weather,
            or_null(e.time.map(|t| t.to_string())),
            or_null(e.pixels.map(|p| p.to_string())),
        ))
    }

    /// Sets the Graphics page's Shadows, Particle Systems and Sky boxes and
    /// the overall quality (0 to 9) from the page. They wait until the
    /// player has a profile, then go into it, and into the race if one is on.
    #[wasm_bindgen]
    pub fn set_graphics(shadows: bool, particles: bool, sky: bool, quality: u32) {
        crate::platform::page_graphics::set(crate::platform::page_graphics::Choice {
            shadows,
            particles,
            sky,
            quality: quality.min(9),
        });
    }

    /// What the player's profile holds for those four, as JSON: `live` is
    /// false while there is no profile (the other fields are then absent),
    /// and `pending` is true while the page's choice has not gone in yet.
    #[wasm_bindgen]
    pub fn game_graphics() -> String {
        match crate::platform::page_graphics::shown() {
            (Some(c), pending) => format!(
                r#"{{"live":true,"pending":{pending},"shadows":{},"particles":{},"sky":{},"quality":{}}}"#,
                c.shadows, c.particles, c.sky, c.quality
            ),
            (None, pending) => format!(r#"{{"live":false,"pending":{pending}}}"#),
        }
    }

    /// Opens the game's menus, once the files are in. `enhanced` is what
    /// the desktop takes after `--enhanced=` (empty for all the defaults),
    /// or none for the game's own drawing, as without the flag.
    #[wasm_bindgen]
    pub fn start_game(enhanced: Option<String>) -> Result<(), JsValue> {
        let game = std::path::Path::new(GAME);
        if !crate::platform::fs::is_file(game.join("UI").join("MAINMENU.DAT")) {
            return Err(JsValue::from_str(
                "that folder is not a Motocross Madness 2 install (UI/MAINMENU.DAT is missing)",
            ));
        }
        if let Some(list) = enhanced {
            crate::graphics::enable(crate::graphics::Enhanced::parse(&list).map_err(|e| JsValue::from_str(&e))?);
            eprintln!("enhanced graphics: {:?} (F3 switches to the original look)", crate::graphics::enhanced().unwrap());
        }
        let mut front = crate::frontend::Frontend::new(game)
            .ok_or_else(|| JsValue::from_str("the game's UI/MAINMENU.DAT cannot be read"))?;
        front.intro();
        crate::run(crate::App::new(None, None, Some(front))).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), BoxError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    // The enhanced graphics, all of them or less those named after `=`.
    if let Some(arg) = args.iter().find(|a| *a == "--enhanced" || a.starts_with("--enhanced=")) {
        let list = arg.strip_prefix("--enhanced").unwrap().trim_start_matches('=');
        graphics::enable(graphics::Enhanced::parse(list)?);
        eprintln!("enhanced graphics: {:?} (F3 switches to the original look)", graphics::enhanced().unwrap());
    }
    if let Some(dir) = flag("--hires") {
        let pack = HiRes::open(Path::new(dir)).map_err(|e| format!("{dir}: {e}"))?;
        eprintln!("{dir}: {} high-res textures", pack.len());
        hires::set_pack(pack);
    }
    // A track to ride straight away, or the game folder for the menus.
    let path = args.first().filter(|a| !a.starts_with("--")).map(Path::new);
    let is_track =
        path.is_some_and(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("env")));
    if !is_track {
        let Some(game) = path
            .map(Path::to_path_buf)
            .or_else(find_game)
            .filter(|g| crate::platform::fs::is_file(g.join("UI").join("MAINMENU.DAT")))
        else {
            eprintln!(
                "usage: mm2-viewer [GAME folder] | <TRACK.ENV> [--screenshot out.png] [--size WxH] [--camera x,y,z,yaw,pitch]"
            );
            std::process::exit(2);
        };
        if let Some(out) = flag("--screenshot") {
            let clicks = flag("--clicks").map(String::as_str).unwrap_or("");
            return pollster::block_on(menu_screenshot(&game, clicks, Path::new(out), None));
        }
        let mut front =
            frontend::Frontend::new(&game).ok_or("the game's UI/MAINMENU.DAT cannot be read")?;
        front.intro();
        return run(App::new(None, None, Some(front)));
    }
    let path = path.unwrap();
    // A tape to show at `--at` seconds instead of a race.
    let mut tape = flag("--replay").and_then(|p| replay::Replay::open(Path::new(p)));
    let riders = flag("--riders")
        .and_then(|n| n.parse().ok())
        .or(tape.as_ref().map(|t| t.tape.riders.len()))
        .unwrap_or(RIDERS)
        .clamp(1, 11);
    if let Some(v) = flag("--spawn") {
        let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if let [x, z, h] = n[..] {
            let _ = SPAWN.set((Vec3::new(x, 0.0, z), Vec3::new(h.sin(), 0.0, h.cos())));
        }
    }
    // `--course N`: a Baja or Enduro track's Nth course, else the tape's.
    let course = flag("--course")
        .and_then(|n| n.parse().ok())
        .or(tape.as_ref().map(|t| t.tape.course()))
        .unwrap_or(0);
    let ghost = args.iter().any(|a| a == "--ghost").then_some(1);
    let mut track = pollster::block_on(load_track(path, course, riders, Vec::new(), PLAYER, ghost))?;
    if let Some(out) = flag("--screenshot") {
        let (w, h) = flag("--size")
            .and_then(|s| s.split_once('x'))
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .unwrap_or((1280, 720));
        let mut camera = match flag("--camera") {
            Some(c) => parse_camera(c).ok_or("--camera wants x,y,z,yaw,pitch")?,
            None => renderer::start_camera(&track.terrain),
        };
        let mut time: f32 = flag("--time").and_then(|t| t.parse().ok()).unwrap_or(0.0);
        let laps = flag("--laps")
            .and_then(|n| n.parse().ok())
            .unwrap_or(race::DEFAULT_LAPS);
        // Moto-Tag for this many minutes (`--tag M`), with the ball
        // (`--ball`).
        let tag_flags = flag("--tag")
            .and_then(|m| m.parse::<f32>().ok())
            .map(|m| (m, args.iter().any(|a| a == "--ball")));
        // Start the computer riders from states on a tape of the game's AI
        // (`--ai-check TAPE.csv --from T1,T2,.. --for S --ai-csv OUT`) and
        // ride on with the tape's frame times, to compare.
        if let Some(tape) = flag("--ai-check") {
            let starts: Vec<f32> = flag("--from")
                .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                .unwrap_or_default();
            let seconds = flag("--for").and_then(|t| t.parse().ok()).unwrap_or(2.0);
            let level = flag("--level").and_then(|l| l.parse::<usize>().ok()).unwrap_or(2);
            let out = flag("--ai-csv").cloned().unwrap_or_else(|| "ai_check.csv".into());
            // `--lanes a,b,..`: each rider's lane, ft, in place of the
            // random one.
            let lanes: Vec<f32> = flag("--lanes")
                .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                .unwrap_or_default();
            return ai_check(&track, tape, &starts, seconds, level, &lanes, &out);
        }
        // A network race between two copies on this machine or network:
        // `--net host` waits for one player, `--net join:ADDR` joins.
        if let Some(how) = flag("--net") {
            let seconds: f32 = flag("--ride").and_then(|t| t.parse().ok()).unwrap_or(10.0);
            return net_test(&track, how, seconds, tag_flags);
        }
        // Race for a while before taking the picture.
        let mut race = None;
        let mut ride_dirt = None;
        if let Some(seconds) = flag("--ride").and_then(|t| t.parse::<f32>().ok()) {
            // `--slot N` starts the player in gate slot N (the original's
            // practice uses the second).
            let slot = flag("--slot").and_then(|t| t.parse().ok()).unwrap_or(PLAYER);
            let mut r = make_race_as(&track, laps, slot).ok_or("this track has no bike to ride")?;
            // Scripted: the game's own steps, frame by frame; `--live-steps`
            // steps as live play does instead, to try frame rates.
            let live = args.iter().any(|a| a == "--live-steps");
            r.fixed_step = live;
            r.ai_frame = if live { race::AI_FRAME } else { 0.0 };
            let mut dirt = particles::Particles::new();
            let mut ride_tyres = Vec::new();
            // A Stunt Quarry session of this many minutes.
            if let Some(m) = flag("--stunt").and_then(|m| m.parse::<f32>().ok()) {
                r.stunt_event = true;
                r.minutes = Some(m);
            }
            if let Some((m, ball)) = tag_flags {
                r.tag = Some(tag::Tag::new(r.racers.len(), ball, false));
                r.minutes = Some(m);
            }
            // The computer riders' level, 1 to 3 (`--level N`).
            if let Some(l) = flag("--level").and_then(|l| l.parse::<usize>().ok()) {
                r.level = l.clamp(1, 3) - 1;
            }
            // Nationals stunt money, to see the units add up.
            r.purse = Some(mm2_physics::career::Tables::shipped().stunt_purse(2));
            // The computer rides for the player unless told how to steer.
            let steer = flag("--steer").and_then(|t| t.parse().ok());
            r.autopilot = steer.is_none() && r.course.is_some();
            let controls = Controls {
                throttle: 1.0,
                steer: steer.unwrap_or(0.0),
                ..Default::default()
            };
            // Hold the gas back until this race time (`--gas-from 3`).
            let gas_from = flag("--gas-from").and_then(|t| t.parse::<f32>().ok());
            // Or play a key script (`--inputs FILE`) from the gate drop, or
            // from race time `--inputs-at`, through the factory keyboard.
            let script = match flag("--inputs") {
                Some(f) => Some(input::Script::parse(&crate::platform::fs::read_to_string(f)?)),
                None => None,
            };
            // `--practice`: the original's practice has no gate, so the
            // bike is free from the start and `--inputs-at` counts from it.
            let practice = args.iter().any(|a| a == "--practice");
            if practice {
                r.time = r.lead;
                r.practice = true;
            }
            let script_at = flag("--inputs-at").and_then(|t| t.parse::<f32>().ok()).unwrap_or(if practice { 0.0 } else { r.lead })
                + if practice { r.lead } else { 0.0 };
            let key_map = input::KeyMap::factory();
            let (mut steer_keys, mut lean_keys) = (input::KeyAxis::default(), input::KeyAxis::default());
            // `MM2_STATE` is put in once, at the first frame from its time.
            let mut state_done = false;
            // The player's state each frame as CSV (`--csv FILE`), in the
            // comparison runs' columns, timed from the script's start.
            let mut csv = flag("--csv").map(|_| {
                String::from("t,x,y,z,roll,pitch,heading,vx,vy,vz,speed,gear,rpm,crashed,steer,turn,traction,sx,sy,sz,front_down,rear_down,front_squeeze,rear_squeeze,bars,lean,phase,mix,slow,motion\n")
            });
            // Every rider's state each frame (`--ai-csv FILE`), in the
            // columns of the original's AI tapes, timed from the gate drop.
            let mut ai_csv = flag("--ai-csv").map(|_| {
                String::from("t,bike,x,y,z,vx,vy,vz,speed,heading,airborne,crashed,trick,laps,on_course\n")
            });
            let mut chase = (Vec3::ZERO, Vec3::Z);
            let mut air = 0;
            // A ghost race: the second rider is the ghost of the best lap.
            let mut ghost = args.iter().any(|a| a == "--ghost").then(|| {
                r.ghost = Some(1);
                r.lead = 0.0;
                let tape = mm2_formats::vcr::Tape::new(
                    Vec::new(),
                    vec![mm2_formats::vcr::Rider::default()],
                );
                replay::Ghost::new(1, tape, None, 0.0)
            });
            // The ride on tape, written to `--record`.
            let mut rec = flag("--record").map(|_| {
                replay::Recording::new(replay::Heading {
                    track: path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string(),
                    course: 0,
                    event: 2,
                    mode: 0,
                    laps,
                    riders: (0..r.racers.len())
                        .map(|i| mm2_formats::vcr::Rider {
                            id: i as u32,
                            computer: i != PLAYER,
                            name: format!("Rider {i}"),
                            bike_model: GRID[i % GRID.len()].0.into(),
                            rider_skin: GRID[i % GRID.len()].1.into(),
                            ..Default::default()
                        })
                        .collect(),
                })
            });
            // Tap trick button 6 twice a second.
            let tricks = args.iter().any(|a| a == "--tricks");
            // Or hold the Taunting key, or the Kahuna Dumpster's, in the air.
            let taunt = args.iter().any(|a| a == "--taunt");
            let kahuna = args.iter().any(|a| a == "--kahuna");
            // The engine sounds alone, as heard from the bike, written
            // as a 22.05 kHz WAV (`--engine-wav FILE`).
            let mut engine_wav = flag("--engine-wav").and_then(|_| {
                let audio = audio::Audio::offline(22050.0);
                let res = track.res.as_deref()?;
                let mut sounds = sound::RaceSounds::load(
                    res,
                    &audio,
                    &[sound::class(250, false)],
                    0,
                    frontend::EventKind::Nationals,
                    false,
                    sound::Options::default(),
                );
                sounds.engines_only = true;
                Some((audio, sounds, Vec::<f32>::new(), 0.0f64))
            });
            // Each frame lasts 1/60 s unless `--fps N` says otherwise. At
            // more than 50 fps the original runs one bike step per frame,
            // as long as that frame.
            let fps = flag("--fps").and_then(|f| f.parse::<f32>().ok()).map_or(1.0 / 60.0, |f| 1.0 / f);
            // Or the frames of a tape of the game (`--frames FILE`: the
            // clock at the end of each frame, counted like `--inputs-at`), so
            // each step is as long as the game's was. A gap over 0.025 s is
            // two frames.
            let shift = if practice { r.lead } else { 0.0 };
            let frames: Vec<f32> = flag("--frames")
                .and_then(|p| crate::platform::fs::read_to_string(p).ok())
                .map(|s| s.split_whitespace().filter_map(|x| x.parse::<f32>().ok()).map(|t| t + shift).collect())
                .unwrap_or_default();
            let mut film = match flag("--film") {
                dir if dir.is_some() || std::env::var("MM2_WRECK_LOG").is_ok() => {
                    let size = flag("--size")
                        .and_then(|s| s.split_once('x'))
                        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                        .unwrap_or((640, 360));
                    let from = flag("--film-from").and_then(|t| t.parse().ok()).unwrap_or(0.0);
                    Some(Film::new(&track, dir.map(PathBuf::from), from, size)?)
                }
                _ => None,
            };
            let mut next = 0;
            let mut elapsed = 0.0;
            let mut k = 0usize;
            while elapsed < seconds {
                while next < frames.len() && frames[next] <= r.time + 1e-4 {
                    next += 1;
                }
                let frame = match frames.get(next) {
                    Some(&t) if t - r.time > 0.025 => (t - r.time) / 2.0,
                    Some(&t) => t - r.time,
                    None => fps,
                };
                elapsed += frame;
                k += 1;
                let flying = r.racers[r.player].bike.airborne();
                r.buttons = [tricks && flying && k % 30 < 15, false];
                r.actions = [kahuna && flying, taunt && flying];
                // Stop for the picture halfway through a trick.
                let me = &r.racers[r.player];
                if tricks && me.stunt.busy() && me.trick_clock > 0.5 {
                    break;
                }
                // Or with the head turned furthest, or the rider let go.
                if me.taunt.is_some_and(|(_, p)| (p - 0.5).abs() > 0.25) {
                    break;
                }
                if let race::Bike::Player(b) = &me.bike {
                    if kahuna && b.crash.is_some_and(|(_, left)| left < 1.2) {
                        break;
                    }
                }
                let mut c = match gas_from {
                    Some(t) if r.time < t => Controls { throttle: 0.0, ..controls },
                    _ => controls,
                };
                if let Some(s) = &script {
                    // The keys held at the frame's end: a tape's clock is read
                    // with the frame's input, so a key logged before a frame's
                    // time already counts in that frame (a keyed
                    // ride: the bars move in the first frame after the key).
                    let codes = s.held(r.time + frame - script_at);
                    let held = |a| key_map.held_code(a, &codes);
                    let dt = frame;
                    c = Controls {
                        throttle: held(input::ACCELERATE) as i32 as f32,
                        brake: held(input::BRAKE) as i32 as f32,
                        steer: steer_keys.update(held(input::STEER_LEFT), held(input::STEER_RIGHT), dt),
                        lean: lean_keys.update(held(input::LEAN_BACK), held(input::LEAN_FORWARD), dt),
                    };
                    r.buttons = [held(input::STUNTS_LOW), held(input::STUNTS_HIGH)];
                    r.actions = [held(input::KAHUNA), held(input::TAUNT)];
                }
                // A state from the running game to go on from (`MM2_STATE`:
                // t|com|velocity|right|up|forward|world spin|squeeze front,rear).
                if let Ok(state) = std::env::var("MM2_STATE") {
                    let f: Vec<Vec<f32>> = state
                        .split('|')
                        .map(|p| p.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                        .collect();
                    let v = |i: usize| Vec3::new(f[i][0], f[i][1], f[i][2]);
                    let now = r.time - script_at;
                    if now >= f[0][0] && !state_done {
                        state_done = true;
                        if let race::Bike::Player(p) = &mut r.racers[r.player].bike {
                            // Going on from the game's state, uncrashed.
                            p.crash = None;
                            p.wreck = None;
                            p.set_state(v(1), v(2), glam::Mat3::from_cols(v(3), v(4), v(5)), v(6), [f[7][0], f[7][1]]);
                        }
                    }
                }
                track.run_traffic(frame);
                chase = r.update(&track.ground(), c, frame);
                if let Some(f) = &mut film {
                    let clock = r.time - script_at;
                    f.frame(&mut r, &track, chase, clock)?;
                }
                if let Some((audio, sounds, out, owed)) = &mut engine_wav {
                    let bike = &r.racers[r.player].bike;
                    let listener = sound::Listener {
                        eye: bike.position(),
                        right: Vec3::X,
                        velocity: bike.velocity(),
                    };
                    sounds.update(audio, &r, &listener, frame);
                    *owed += 22050.0 / 60.0;
                    let n = *owed as usize;
                    *owed -= n as f64;
                    out.extend(audio.render(n).chunks(2).map(|f| (f[0] + f[1]) / 2.0));
                }
                throw_dirt(&r, &track.ground(), &mut dirt, frame);
                ride_tyres.push((tyre_places(&r, &track.ground()), frame));
                if let Some(out) = &mut ai_csv {
                    use std::fmt::Write as _;
                    for (i, rc) in r.racers.iter().enumerate() {
                        let b = &rc.bike;
                        let (p, v, f) = (b.position(), b.velocity(), b.forward());
                        let _ = writeln!(
                            out,
                            "{:.4},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.4},{},{},{},{},{}",
                            r.clock(), i, p.x, p.y, p.z, v.x, v.y, v.z, v.length(), f.x.atan2(f.z),
                            b.airborne() as u8, b.crashed() as u8,
                            rc.stunt.trick.map_or(-1, |t| t as i32), rc.laps, rc.on_course() as u8
                        );
                    }
                }
                if let (Some(out), race::Bike::Player(p)) = (&mut csv, &r.racers[r.player].bike) {
                    use std::fmt::Write as _;
                    let (pos, v, m) = (p.position, p.velocity, p.rotation());
                    let f = m.z_axis;
                    let _ = writeln!(
                        out,
                        "{:.4},{:.3},{:.3},{:.3},{:.4},{:.4},{:.4},{:.3},{:.3},{:.3},{:.3},{},{:.0},{},{:.4},{:.3},{:.3},{:.3},{:.3},{:.3},{},{},{:.3},{:.3}",
                        r.time - script_at,
                        pos.x, pos.y, pos.z,
                        -m.x_axis.y.clamp(-1.0, 1.0).asin(),
                        f.y.clamp(-1.0, 1.0).asin(),
                        f.x.atan2(f.z),
                        v.x, v.y, v.z, v.length(),
                        p.drivetrain.gear, p.drivetrain.rpm, p.crash.is_some() as u8,
                        p.steer, p.turn_rate, p.traction,
                        p.spin.x, p.spin.y, p.spin.z,
                        p.tyres_down()[0] as u8, p.tyres_down()[1] as u8, p.squeezes()[0], p.squeezes()[1]
                    );
                    // The rider's pose as the original's tapes have it:
                    // f_504, f_508, f_65c, f_654, f_660 and the first row.
                    out.pop();
                    let mix = &r.racers[r.player].mixes.0;
                    let (phase, w, g, name) = match mix.as_slice() {
                        [a, b, ..] => (a.1, b.2 / (a.2 + b.2).max(1e-6), a.2 + b.2, a.0),
                        [a] => (a.1, 0.0, 1.0, a.0),
                        [] => (0.0, 0.0, 0.0, ""),
                    };
                    let _ = writeln!(out, ",{:.3},{:.3},{:.3},{:.3},{:.3},{}", -p.rider.0, p.rider.1, phase, w, g, name);
                }
                if std::env::var("MM2_TRACE").is_ok() && (k % 6 == 0 || std::env::var("MM2_TRACE").as_deref() == Ok("all")) {
                    let b = &r.racers[r.player].bike;
                    let g = track.ground();
                    eprintln!(
                        "t {:.1} pos {:.0?} vel {:.0?} look,spin,lean {:.2?} crashed {} wall {:?}",
                        k as f32 / 60.0,
                        b.position(),
                        b.velocity(),
                        match b { race::Bike::Player(p) => (p.forward(), p.spin, p.lean), _ => (Vec3::Z, Vec3::ZERO, 0.0) },
                        b.crashed(),
                        mm2_physics::Ground::solid(&g, b.position(), 1.5).map(|h| h.depth)
                    );
                    if let race::Bike::Player(p) = b {
                        let at = p.position;
                        eprintln!("    ground {:.2?} tyres {:?}", mm2_physics::Ground::terrain(&g, at.x, at.z), p.tyres_down());
                    }
                }
                if let Some(rec) = &mut rec {
                    rec.tick(&r, frame);
                }
                if let Some(g) = &mut ghost {
                    g.tick(&mut r, &track.terrain, frame);
                }
                air += r.racers[r.player].bike.airborne() as usize;
            }
            if let (Some((_, _, out, _)), Some(path)) = (&engine_wav, flag("--engine-wav")) {
                let mut wav = Vec::new();
                let bytes = out.len() as u32 * 2;
                wav.extend(b"RIFF");
                wav.extend((36 + bytes).to_le_bytes());
                wav.extend(b"WAVEfmt ");
                wav.extend(16u32.to_le_bytes());
                wav.extend([1u8, 0, 1, 0]);
                wav.extend(22050u32.to_le_bytes());
                wav.extend(44100u32.to_le_bytes());
                wav.extend([2u8, 0, 16, 0]);
                wav.extend(b"data");
                wav.extend(bytes.to_le_bytes());
                for s in out {
                    wav.extend(((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                }
                crate::platform::fs::write(path, wav)?;
            }
            if let Some(g) = &ghost {
                let best = g.best().map(|t| t.length);
                let shown = !r.racers[1].bike.place().x_axis.abs().max_element().eq(&0.0);
                eprintln!("ghost: best lap {best:?}, showing {shown}");
            }
            if let (Some(text), Some(out)) = (ai_csv, flag("--ai-csv")) {
                crate::platform::fs::write(out, text)?;
            }
            if let (Some(text), Some(out)) = (csv, flag("--csv")) {
                crate::platform::fs::write(out, text)?;
            }
            if let (Some(rec), Some(out)) = (rec, flag("--record")) {
                crate::platform::fs::write(out, rec.finish().write())?;
                eprintln!("wrote the tape to {out}");
            }
            let b = &r.racers[r.player].bike;
            let at = b.position();
            eprintln!(
                "rode {seconds}s: at {:.0},{:.0},{:.0}, {:.0} units/s, {:.1}s in the air{}",
                at.x,
                at.y,
                at.z,
                b.speed(),
                air as f32 / 60.0,
                if b.crashed() {
                    ", crashed"
                } else if b.airborne() {
                    ", flying"
                } else {
                    ""
                }
            );
            eprintln!("{}", r.status());
            if let Some(b) = r.tag.as_ref().and_then(|t| t.ball) {
                let p = b.position;
                eprintln!("ball at {:.1},{:.1},{:.1}", p.x, p.y, p.z);
            }
            let me = &r.racers[r.player];
            eprintln!(
                "stunts: {:.0} points, best {:.0}, now {:?}",
                me.stunt_points, me.best_stunt, me.stunt.trick
            );
            for (i, racer) in r.racers.iter().enumerate() {
                let laps: Vec<_> = racer.lap_times.iter().map(|&t| race::time(t)).collect();
                let p = racer.bike.position();
                eprintln!(
                    "  rider {i} at {:.0},{:.0}: lap {} {:?}, gates {}, stunts {:.0}, units {:.2}/{:.2}/{:.2}{}",
                    p.x,
                    p.z,
                    racer.laps,
                    laps,
                    racer.timing.gates,
                    racer.stunt_points,
                    racer.repair,
                    racer.medical,
                    racer.stunt_units,
                    if racer.bike.crashed() {
                        ", crashed"
                    } else {
                        ""
                    }
                );
            }
            if flag("--camera").is_none() {
                camera =
                    Camera::looking_at(renderer::from_game(chase.0), renderer::from_game(chase.1));
            }
            // A close look at the player's bike from in front.
            if args.iter().any(|a| a == "--front") {
                let up = Vec3::Y;
                let eye = at + b.forward() * 7.0 + up * 3.5 + b.forward().cross(up) * 2.0;
                camera = Camera::looking_at(
                    renderer::from_game(eye),
                    renderer::from_game(at + up * 2.5),
                );
            }
            // From behind another rider (`--watch i`).
            if let Some(w) = flag("--watch").and_then(|i| i.parse::<usize>().ok()) {
                let b = &r.racers[w.min(r.racers.len() - 1)].bike;
                let (at, f) = (b.position(), b.forward());
                let eye = at - f * 16.0 + Vec3::Y * 7.0;
                camera = Camera::looking_at(
                    renderer::from_game(eye),
                    renderer::from_game(at + Vec3::Y * 3.0),
                );
            }
            // The podium `--podium T` s in, for the track's kind of event
            // (`--roll` the stage's coin, 0 to 1).
            if let (Some(t), Some(stand)) = (flag("--podium").and_then(|t| t.parse::<f32>().ok()), &track.podium) {
                use frontend::EventKind::*;
                let folder = path.to_string_lossy().to_ascii_uppercase();
                let event = [("/SX/", Supercross), ("NATIONAL", Nationals), ("TAG", Tag), ("BAJA", Baja), ("ENDURO", Enduro)]
                    .into_iter()
                    .find(|(name, _)| folder.contains(name))
                    .map_or(Stunt, |(_, e)| e);
                let roll = flag("--roll").and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.7);
                let p = podium_for(&track, stand, event, roll);
                r.podium = Some((p, r.time));
                r.time += t;
                let (eye, target) = p.camera(t);
                camera = Camera::looking_at(renderer::from_game(eye), renderer::from_game(target));
            }
            time = r.time;
            // A stunt score as if just scored: base,multiplier,seconds ago.
            if let Some(p) = flag("--popup") {
                let v: Vec<f32> = p.split(',').filter_map(|v| v.parse().ok()).collect();
                if let [base, m, age] = v[..] {
                    r.popup = Some((r.time - age, base, m));
                }
            }
            // The Race Info page after this race, instead of the track.
            if let (true, Some((ui, _))) = (args.iter().any(|a| a == "--results"), track.hud()) {
                let game = track.res.as_deref().and_then(Path::parent).unwrap();
                let lines = results(&r, &ui);
                return pollster::block_on(menu_screenshot(game, "", Path::new(out), Some(lines)));
            }
            race = Some(r);
            ride_dirt = Some((dirt, ride_tyres));
        }
        if let (Some(rp), None) = (&mut tape, &race) {
            let mut r = make_race(&track, laps).ok_or("this track has no bike to ride")?;
            rp.time = flag("--at")
                .and_then(|t| t.parse().ok())
                .unwrap_or(rp.start());
            rp.follow = flag("--follow").and_then(|t| t.parse().ok()).unwrap_or(0);
            for _ in 0..flag("--view").and_then(|t| t.parse::<usize>().ok()).unwrap_or(0) {
                r.next_view();
            }
            // From a little before, so the chase camera has settled.
            let at = rp.time;
            rp.time = (at - 1.0).max(rp.start());
            r.time = rp.time;
            rp.show(&mut r, &track.terrain);
            while rp.time < at {
                rp.time = (rp.time + 1.0 / 60.0).min(at);
                chase_at(&mut camera, rp.show(&mut r, &track.terrain));
            }
            camera.fov = r.fov();
            time = r.time;
            race = Some(r);
        }
        return pollster::block_on(screenshot(
            &track,
            race.as_ref(),
            ride_dirt,
            tape.as_mut(),
            &camera,
            time,
            Path::new(out),
            w,
            h,
        ));
    }
    let laps = flag("--laps")
        .and_then(|n| n.parse().ok())
        .unwrap_or(race::DEFAULT_LAPS);
    let race = make_race(&track, laps);
    run(App::new(Some(track), race, None))
}

/// Things that happen outside the window's own events.
enum UserEvent {
    /// The browser has given the GPU (it cannot be waited for there), and
    /// the window's drawing state is ready.
    #[allow(dead_code)]
    Ready(Box<State>),
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// Lets the GPU request, which finishes later in the browser, reach the app.
    static PROXY: std::cell::RefCell<Option<winit::event_loop::EventLoopProxy<UserEvent>>> =
        const { std::cell::RefCell::new(None) };
}

fn run(mut app: App) -> Result<(), BoxError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        event_loop.run_app(&mut app)?;
    }
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::EventLoopExtWebSys;
        PROXY.with(|p| *p.borrow_mut() = Some(event_loop.create_proxy()));
        // The browser runs the loop; this returns at once.
        event_loop.spawn_app(app);
    }
    Ok(())
}

/// `--window WxH`: the window's size, 1024 × 768 unless given.
fn window_size() -> winit::dpi::LogicalSize<f64> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == "--window")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some(winit::dpi::LogicalSize::new(w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or(winit::dpi::LogicalSize::new(1024.0, 768.0))
}

/// The game folder: `GAME` in or above the folder the port starts in, or
/// the copy the launcher made in the user folder.
fn find_game() -> Option<std::path::PathBuf> {
    let here = std::env::current_dir().ok()?;
    here.ancestors()
        .flat_map(|d| [d.join("GAME"), d.to_path_buf()])
        .chain([profile::user_dir().join("GAME")])
        .find(|g| crate::platform::fs::is_file(g.join("UI").join("MAINMENU.DAT")))
}

/// A track's terrain and the objects its scene places.
pub struct Track {
    terrain: Terrain,
    /// The terrain textures to draw: the game's, or a high-res pack's.
    terrain_textures: Vec<hires::Image>,
    /// The solid objects, for the player's bike.
    world: mm2_physics::collision::World,
    /// The player's bike.
    spec: Spec,
    objects: ObjectMesh,
    start: Start,
    course: Option<TrackDef>,
    /// The bikes on the start line, as animated objects; each one's rider
    /// comes next.
    grid: Vec<usize>,
    /// The start gate's animated object.
    gate: Option<usize>,
    /// The flag girl and the 30-second man.
    people: Finish,
    /// The podium and its riders, for after the race.
    podium: Option<Stand>,
    /// The game's `RES` folder, for the race's sounds.
    res: Option<std::path::PathBuf>,
    /// The track's name, shown as the race begins.
    name: String,
    /// A stadium track (TERAFORM/SX).
    supercross: bool,
    /// A desert track (TERAFORM/BAJA), where the enhanced drawing's rain
    /// is overcast instead.
    desert: bool,
    /// The checkpoint gates of a Baja or Enduro course, in order.
    gates: Vec<mm2_physics::race::Line>,
    /// Their arches, as animated objects, and where they stand.
    arches: Vec<(usize, Mat4)>,
    /// The player's trick motions.
    tricks: Vec<Option<race::TrickMotion>>,
    /// The stunt score's models.
    popup: Option<popup::Popup>,
    /// Moto-Tag's IT sign, beach ball and the player's own sign, as
    /// animated objects.
    tag: Option<[usize; 3]>,
    /// Visual Cues: the arrow and the runway lights.
    cues: Option<cues::Cues>,
    /// The 3D countdown numbers.
    countdown: Option<countdown::Countdown>,
    /// The sky box.
    sky: Option<(mm2_formats::cub::Cube, hires::SkyImages)>,
    /// The fog's colour, which the screen is cleared to.
    fog: Option<[f32; 3]>,
    /// The scene's fog Visibility and Haziness, when it has fog: only a
    /// scene with a sky box does (0x4e2d70).
    haze: Option<(f32, f32)>,
    /// The way the scene's directional light travels, which the player's
    /// shadow falls along.
    sun: Vec3,
    /// The scene's lights.
    lights: renderer::SceneLights,
    /// The terrain's detail texture (0x50b140).
    detail: Option<mm2_formats::tga::Image>,
    /// The dirt and dust sheet (`ParticleTextureMapName`).
    particle_sheet: Option<mm2_formats::tga::Image>,
    /// The scene's scripted vehicles.
    traffic: traffic::Traffic,
    /// How each terrain surface type grips.
    zones: [mm2_formats::scn::Zone; 8],
    /// The scene has a crowd (`[Sounds] CrowdPresent`).
    crowd: bool,
    /// The enhanced drawing's spectators (game coordinates) and which way
    /// each looks.
    spectators: Vec<(Vec3, Vec3)>,
    /// The scene's ambient sounds and what each hangs on.
    sounds: Vec<(scn::SoundSpec, sound::Site)>,
}

impl Track {
    /// Where a scene sound's site is at `time` on the race clock.
    fn site(&self, site: sound::Site, time: f32) -> Option<Vec3> {
        match site {
            sound::Site::Fixed(p) => Some(p),
            sound::Site::Object(o, frame) => self.objects.animated.get(o).map(|m| m.frame_position(frame, time)),
            sound::Site::Car(c) => self.traffic.cars.get(c).map(|c| c.vehicle.position),
        }
    }

    /// The in-race display, when the game's files are found.
    fn hud(&self) -> Option<(ui::Ui, hud::Hud)> {
        let game = self.res.as_deref()?.parent()?;
        let mut hud = hud::Hud::default();
        hud.say(&self.name);
        Some((ui::Ui::new(game), hud))
    }

    /// Drives the scene's vehicles on by `dt`.
    fn run_traffic(&mut self, dt: f32) {
        self.traffic.update(dt, &self.terrain, &mut self.world);
    }

    fn ground(&self) -> TrackGround<'_> {
        TrackGround {
            terrain: &self.terrain,
            world: &self.world,
            zones: &self.zones,
            edge: !self.supercross,
        }
    }
}

/// A race on `track` with the bikes on its start line.
fn make_race(track: &Track, laps: u32) -> Option<Race> {
    make_race_as(track, laps, PLAYER)
}

/// A race with the player on bike `player`.
/// See `--ai-check`.
fn ai_check(
    track: &Track,
    tape: &str,
    starts: &[f32],
    seconds: f32,
    level: usize,
    lanes: &[f32],
    out: &str,
) -> Result<(), BoxError> {
    use std::fmt::Write as _;
    let text = crate::platform::fs::read_to_string(tape)?;
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().unwrap_or("").split(',').collect();
    let col = |n: &str| head.iter().position(|h| *h == n).unwrap_or(0);
    let (ct, cb) = (col("t"), col("bike"));
    let cols = ["x", "y", "z", "vx", "vy", "vz", "roll", "pitch", "heading", "spin_x", "spin_y", "spin_z"].map(col);
    // Rows by bike: time and the twelve values.
    let mut rows: Vec<Vec<(f32, [f32; 12])>> = Vec::new();
    for line in lines {
        let f: Vec<&str> = line.split(',').collect();
        let (Ok(t), Ok(b)) = (f[ct].parse::<f32>(), f[cb].parse::<usize>()) else { continue };
        let v = cols.map(|c| f[c].parse::<f32>().unwrap_or(0.0));
        if rows.len() <= b {
            rows.resize(b + 1, Vec::new());
        }
        rows[b].push((t, v));
    }
    let mut text = String::from("start,t,bike,x,y,z,speed,crashed,airborne,trick,vx,vy,vz,fy\n");
    for &start in starts {
        let mut r = make_race_as(track, race::DEFAULT_LAPS, PLAYER).ok_or("no race")?;
        // Scripted: the game's own steps, frame by frame.
        r.fixed_step = false;
        r.ai_frame = 0.0;
        r.level = level.clamp(1, 3) - 1;
        r.time = r.lead + start;
        let k0 = rows[1].iter().position(|(t, _)| *t >= start).ok_or("start past the tape")?;
        for (i, racer) in r.racers.iter_mut().enumerate() {
            let Some(&(_, v)) = rows.get(i).and_then(|b| b.get(k0)) else { continue };
            let m = replay::rotation([v[6], v[7], v[8]]);
            racer.set_computer(
                Vec3::new(v[0], v[1], v[2]),
                Vec3::new(v[3], v[4], v[5]),
                m.z_axis,
                m.y_axis,
                Vec3::new(v[9], v[10], v[11]),
            );
            if let (Some(&lane), race::Bike::Computer(c)) = (lanes.get(i), &mut racer.bike) {
                c.rider.lane = lane;
            }
        }
        let mut k = k0;
        while k + 1 < rows[1].len() && rows[1][k + 1].0 <= start + seconds {
            let dt = rows[1][k + 1].0 - rows[1][k].0;
            k += 1;
            if dt <= 0.0 {
                continue;
            }
            r.update(&track.ground(), Controls::default(), dt);
            for (i, racer) in r.racers.iter().enumerate() {
                let b = &racer.bike;
                let p = b.position();
                let _ = writeln!(
                    text,
                    "{start},{:.4},{i},{:.3},{:.3},{:.3},{:.3},{},{},{},{:.3},{:.3},{:.3},{:.4}",
                    rows[1][k].0,
                    p.x,
                    p.y,
                    p.z,
                    b.velocity().length(),
                    b.crashed() as u8,
                    b.airborne() as u8,
                    racer.stunt.busy() as u8,
                    b.velocity().x,
                    b.velocity().y,
                    b.velocity().z,
                    b.place().z_axis.y
                );
            }
        }
    }
    crate::platform::fs::write(out, text)?;
    Ok(())
}

fn make_race_as(track: &Track, laps: u32, player: usize) -> Option<Race> {
    let mut race = make_race_on(track, laps, player)?;
    race.tricks = track.tricks.clone();
    Some(race)
}

fn make_race_on(track: &Track, laps: u32, player: usize) -> Option<Race> {
    let grid: Vec<_> = track
        .grid
        .iter()
        .map(|&o| (o, track.objects.animated[o].position()))
        .collect();
    // A `--spawn` heading replaces the start line's.
    let direction = SPAWN.get().map_or(track.start.direction, |&(_, way)| way);
    Race::new(
        &track.ground(),
        &grid,
        player,
        direction,
        track.course.as_ref(),
        laps,
        track.spec.clone(),
        track.gates.clone(),
    )
    .map(|mut race| {
        race.supercross = track.supercross;
        // With the 30-second man out, the countdown is his 8.2 s walk.
        if track.people.man.is_some() {
            race.lead = finish::MAN_LEAD;
        }
        race
    })
}

/// Moves the bikes and riders, and holds the gate until it drops.
/// The camera's lens on the 640×480 screen, in pixels (our 60° view).
const FOCAL: f32 = 415.69;

/// Where each bike's tyres touch the ground, game coordinates (none for a
/// tyre in the air), for the enhanced drawing's ruts.
fn tyre_places(race: &Race, ground: &TrackGround) -> Vec<([Option<Vec3>; 2], dust::Kick)> {
    // Whether the ground here raises dust, as the game's own trail asks.
    let dusty = |at: Option<(Vec3, bool)>| at.is_some_and(|(at, on)| if on { ground.zones[7].dust } else { ground.zone(at).dust });
    race.racers
        .iter()
        .map(|r| match &r.bike {
            race::Bike::Player(b) => {
                let at = b.tyre_contacts().map(|t| t.filter(|_| b.crash.is_none()));
                let kick = dust::Kick {
                    dusty: dusty(at[1]) || dusty(at[0]),
                    spin: b.roost().1,
                };
                (at.map(|t| t.map(|(at, _)| at)), kick)
            }
            race::Bike::Computer(c) => match &c.wreck {
                Some(_) => ([None; 2], dust::Kick::default()),
                None => {
                    let rd = &c.rider;
                    let ends = [rd.position + rd.look * 2.5, rd.position - rd.look * 2.5];
                    let at = [0, 1].map(|k| rd.down[k].then_some(ends[k]));
                    let kick = dust::Kick {
                        dusty: at.iter().flatten().any(|&p| dusty(Some((p, false)))),
                        spin: 0.0,
                    };
                    (at, kick)
                }
            },
            race::Bike::Taped(_) => {
                let b = &r.bike;
                let down = !b.airborne() && !b.crashed();
                let at = [down.then(|| b.position() + b.forward() * 2.5), down.then(|| b.position() - b.forward() * 2.5)];
                let kick = dust::Kick {
                    dusty: at.iter().flatten().any(|&p| dusty(Some((p, false)))),
                    spin: 0.0,
                };
                (at, kick)
            }
        })
        .collect()
}

/// The bikes feed their dust, dirt, exhaust and spark emitters, and what
/// is in flight flies on (see `particles`).
/// One raced frame of the live ride log: the race clock, the frame, the
/// actions held (bit per action: 0 left, 1 right, 2 lean forward, 3 lean
/// back, 4 gas, 5 brake, ...) and the player's state.
fn log_ride(out: &mut impl std::io::Write, r: &Race, acts: &[bool], dt: f32) {
    let race::Bike::Player(p) = &r.racers[r.player].bike else { return };
    let keys = acts.iter().enumerate().fold(0u32, |k, (i, &a)| k | (a as u32) << i);
    let (pos, v, m) = (p.position, p.velocity, p.rotation());
    let f = m.z_axis;
    let _ = writeln!(
        out,
        "{:.4},{:.4},{},{:.3},{:.3},{:.3},{:.4},{:.4},{:.4},{:.3},{:.3},{:.3},{:.3},{},{:.0},{},{:.3},{:.3},{},{},{:.3},{:.3}",
        r.time, dt, keys, pos.x, pos.y, pos.z,
        -m.x_axis.y.clamp(-1.0, 1.0).asin(), f.y.clamp(-1.0, 1.0).asin(), f.x.atan2(f.z),
        v.x, v.y, v.z, v.length(), p.drivetrain.gear, p.drivetrain.rpm, p.crash.is_some() as u8,
        -p.rider.0, p.rider.1, p.tyres_down()[0] as u8, p.tyres_down()[1] as u8,
        p.squeezes()[0], p.squeezes()[1]
    );
}

fn throw_dirt(race: &Race, ground: &TrackGround, dirt: &mut particles::Particles, dt: f32) {
    let tyre = |at: Vec3, on_object: bool| {
        let zone = if on_object { ground.zones[7] } else { ground.zone(at) };
        particles::Tyre {
            at,
            dust: zone.dust,
            dirt: zone.dirt,
        }
    };
    // The exhaust's place on the bike (0x4087e0).
    let exhaust = |place: Mat4| place.transform_point3(Vec3::new(0.0, 2.5, -2.75));
    // A bike on the full model: the player's, or a computer rider's once
    // crashed.
    let full = |b: &mm2_physics::Bike| {
        let (traction, spin, throw) = b.roost();
        let crashed = b.crash.is_some();
        particles::Feed {
            tyres: b.tyre_contacts().map(|t| t.filter(|_| !crashed).map(|(at, on)| tyre(at, on))),
            traction,
            spin,
            throw,
            right: b.rotation().x_axis,
            bars: b.steer,
            running: b.speed() >= 0.2,
            exhaust: exhaust(b.place()),
            gas: b.drivetrain.throttle,
            gas_ok: !crashed && b.velocity.length() < 22.0,
            hit: b.hit,
            slide: b.wreck.as_ref().filter(|w| w.touching).map(|w| w.position),
            moved: false,
        }
    };
    for (i, r) in race.racers.iter().enumerate() {
        let feed = match &r.bike {
            race::Bike::Player(b) => full(b),
            race::Bike::Computer(c) => match &c.wreck {
                Some(b) => full(b),
                None => {
                    // The computer's bikes keep their tyres' traction at 1
                    // and spin at 0: dust, but no chunks.
                    let rd = &c.rider;
                    let ends = [rd.position + rd.look * 2.5, rd.position - rd.look * 2.5];
                    particles::Feed {
                        tyres: [0, 1].map(|k| rd.down[k].then(|| tyre(ends[k], false))),
                        traction: 1.0,
                        right: rd.up.cross(rd.look).normalize_or(Vec3::X),
                        running: rd.forward_speed() >= 0.2,
                        exhaust: exhaust(r.bike.place()),
                        // The computer's throttle opens as its speed rises.
                        gas: rd.speed,
                        gas_ok: rd.speed < 22.0,
                        ..Default::default()
                    }
                }
            },
            race::Bike::Taped(_) => {
                let down = !r.bike.airborne() && !r.bike.crashed();
                let at = r.bike.position() - r.bike.forward() * 2.5;
                particles::Feed {
                    tyres: [None, down.then(|| tyre(at, false))],
                    traction: 1.0,
                    running: r.bike.speed() >= 0.2,
                    exhaust: exhaust(r.bike.place()),
                    ..Default::default()
                }
            }
        };
        dirt.bike(i, &feed, dt);
    }
    dirt.update(dt);
}

/// The thrown rider's collision shapes (`Rider.col`, loaded by 0x4337c0
/// from 0x535830): each part's frame in Rider.mcf and its corners there.
static RIDER_COL: std::sync::OnceLock<Vec<(usize, Vec<Vec3>)>> = std::sync::OnceLock::new();

/// Hands each thrown rider's pelvis and collision shapes, as the crash
/// motion poses them, to the wreck physics: the body is the pelvis, and
/// the shapes are its ground contacts.
fn feel_wrecks(race: &mut Race, renderer: &TerrainRenderer) {
    let Some(parts) = RIDER_COL.get() else {
        return;
    };
    for r in &mut race.racers {
        let Some((place, _)) = r.bike.wreck() else {
            continue;
        };
        let Some(pelvis) = renderer.object_frame_index(r.object + 1, "Pelvis") else {
            continue;
        };
        let frames: Vec<usize> = parts.iter().map(|p| p.0).chain([pelvis]).collect();
        let posed = renderer.object_frames(r.object + 1, &frames, race.time);
        let Some(Some(at)) = posed.last().copied() else {
            continue;
        };
        let to_body = at.inverse();
        let points: Vec<Vec3> = posed
            .iter()
            .zip(parts)
            .filter_map(|(m, (_, corners))| m.map(|m| (to_body * m, corners)))
            .flat_map(|(m, corners)| corners.iter().map(move |c| m.transform_point3(*c)))
            .collect();
        r.bike.set_wreck_pose(place.inverse() * at, points);
    }
}

fn show(race: &Race, track: &Track, renderer: &mut TerrainRenderer, eye: Vec3, view: Option<cues::Eye>) {
    // The player's shadow: the bike and rider, seen from the sun.
    {
        let me = &race.racers[race.player];
        renderer.set_shadow([me.object, me.object + 1], me.bike.position() + Vec3::Y * 1.5, track.sun);
    }
    for (i, r) in race.racers.iter().enumerate() {
        // A ghost race's ghost is see-through.
        let ghost = race.ghost == Some(i);
        renderer.set_object_ghost(r.object, ghost);
        renderer.set_object_ghost(r.object + 1, ghost);
        renderer.show_mud(i, [r.object, r.object + 1]);
        // Crossed up in the air, the cross-up motions turn the bike and
        // the rider themselves.
        let place = if r.pose.crossed() {
            r.bike.place()
        } else {
            r.bike.place() * Mat4::from_quat(r.bike.drawn_turn())
        };
        renderer.set_object_place(r.object, place);
        // The wheels turn with the bike's speed (0x518c40).
        for (frame, (angle, _)) in ["TireFront", "TireRear"].into_iter().zip(r.wheels) {
            renderer.set_spin(r.object, frame, angle);
        }
        // In a crash the rider is thrown off and plays the fall.
        let wreck = r.bike.wreck();
        let (rider, motion) = wreck.unwrap_or((place, RIDING));
        renderer.set_object_place(r.object + 1, rider);
        match r.trick_motion(&race.tricks).filter(|_| wreck.is_none()) {
            Some((trick, clock)) => {
                let start = race.time - clock;
                renderer.set_motion(r.object + 1, &trick.rider, start);
                renderer.set_motion(r.object, &trick.bike, start);
            }
            None if wreck.is_some() => {
                // A bail starts at once (0x537890); a fall and the landing
                // after it fade in over 0.5 s (0x537980, 0x535d50).
                use mm2_physics::wreck::{FALLS, HITS};
                let span = if FALLS.contains(&motion) || HITS.contains(&motion) { 0.5 } else { 0.0 };
                renderer.fade_motion(r.object + 1, motion, race.time, span);
                renderer.set_motion(r.object, BIKE_RIDING, 0.0);
            }
            None => {
                // Riding: the turn and lean motions mixed by the steer,
                // the rider's weight and the speed (0x48fe50).
                let (rider, bike) = &r.mixes;
                renderer.set_motion(r.object + 1, RIDING, 0.0);
                renderer.set_blend(r.object + 1, rider);
                renderer.set_motion(r.object, BIKE_RIDING, 0.0);
                renderer.set_blend(r.object, bike);
            }
        }
    }
    if let Some(gate) = track.gate {
        renderer.set_motion_start(gate, race.lead);
    }
    let hidden = |at: Mat4| Mat4::from_cols(Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, at.w_axis);
    // The flag girl waves the flag she has out; the other copy is hidden.
    if let Some((girls, place)) = track.people.girl {
        let g = race.flag_girl;
        for (k, object) in girls.into_iter().enumerate() {
            let out = !race.practice && g.white == (k == 1);
            renderer.set_motion(object, g.motion, g.start);
            renderer.set_motion_start(object, g.start);
            renderer.set_object_place(object, if out { place } else { hidden(place) });
        }
    }
    // The 30-second man walks in 15 ft before the player's bike during
    // the countdown (0x421f90, 0x419fe3) and is gone when the gate drops.
    if let Some(man) = track.people.man {
        let me = &race.racers[race.player].bike;
        let forward = me.forward();
        let mut at = me.position() + forward * finish::MAN_AHEAD;
        at.y = ride::ground(&track.terrain, at.x, at.z);
        let place = objects::orientation((-forward).into(), [0.0, 1.0, 0.0], Vec3::ONE, at.into());
        let out = !race.practice && !race.gate_down();
        renderer.set_object_place(man, if out { place } else { hidden(place) });
        renderer.set_motion_start(man, race.lead - finish::MAN_LEAD);
    }
    // The first three on the podium after the race; the rest are hidden.
    if let Some(stand) = &track.podium {
        let shown = race.podium.as_ref().map(|(p, start)| (p, *start, race.standings()));
        for (k, &model) in stand.models.iter().enumerate() {
            let place = shown.as_ref().filter(|(p, ..)| p.model() == k).map(|(p, ..)| p.place());
            renderer.set_object_place(model, place.unwrap_or(hidden(Mat4::IDENTITY)));
        }
        for (i, &object) in stand.winners.iter().enumerate() {
            let on = shown.as_ref().and_then(|(p, start, order)| {
                let count = order.len().min(3);
                let k = order.iter().take(count).position(|&r| r == i)?;
                Some((p.rider(k, count), *start))
            });
            match on {
                Some(((at, motion), start)) => {
                    renderer.set_motion(object, &motion, start);
                    renderer.set_motion_start(object, start);
                    renderer.set_object_place(object, at);
                }
                None => renderer.set_object_place(object, hidden(Mat4::IDENTITY)),
            }
        }
    }
    // The player's last stunt score.
    if let Some(popup) = &track.popup {
        let me = &race.racers[race.player];
        let score = race
            .popup
            .map(|(at, base, m)| (race.time - at, base, m))
            .filter(|_| !me.bike.crashed());
        let places = popup.places(score, me.bike.position(), me.bike.forward(), eye, FOCAL);
        for (object, place) in places {
            renderer.set_object_place(object, place);
        }
    }
    // Moto-Tag's signs and ball, hidden outside a tag game; in any other
    // race with more than one rider the player's own sign stands over
    // their bike (0x418cd0, 0x41e0b0) when Player Indicator is on.
    if let Some(models) = track.tag {
        let hidden = Mat4::from_cols(Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, Vec4::W);
        let mut places = match &race.tag {
            Some(t) => {
                let riders = race.tag_riders();
                let me = race.names.len().gt(&1).then_some(race.player);
                t.places(&riders, me, race.time)
            }
            None if race.racers.len() > 1 => {
                let riders = race.tag_riders();
                [hidden, hidden, tag::own_sign(&riders[race.player], race.time)]
            }
            None => [hidden; 3],
        };
        if !race.player_indicator || race.ghost.is_some() {
            places[2] = hidden;
        }
        for (object, place) in models.into_iter().zip(places) {
            renderer.set_object_place(object, place);
        }
    }
    if let Some(cues) = &track.cues {
        place_cues(race, track, cues, renderer, view);
    }
    if let Some(c) = &track.countdown {
        place_countdown(race, track, c, renderer, view);
    }
    for (object, place) in track.traffic.places() {
        renderer.set_object_place(object, place);
    }
    // Only the gate the player rides for and the one after stand
    // (0x4052c0); the others are hidden.
    let count = track.arches.len();
    let next = race.racers[race.player].timing.gate;
    for (k, &(object, place)) in track.arches.iter().enumerate() {
        let shown = k == next || k == (next + 1) % count;
        renderer.set_object_place(
            object,
            if shown {
                place
            } else {
                Mat4::from_cols(Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, place.w_axis)
            },
        );
    }
}

/// The 3D countdown numbers (0x48cf70): in a race, in the live race's
/// own view, before the gate drops when the 30-second man is not out, and
/// while the player is off the course.
fn place_countdown(race: &Race, track: &Track, c: &countdown::Countdown, renderer: &mut TerrainRenderer, view: Option<cues::Eye>) {
    let hide = |place: Mat4| Mat4::from_cols(Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, place.w_axis);
    let me = &race.racers[race.player];
    let left = track.people.man.is_none().then_some(race.lead - race.time);
    let shown = view
        .filter(|_| !race.practice && race.ghost.is_none() && race.watched.is_none())
        .and_then(|v| {
            // Only the lap races' tracker (0x4eb170) counts the time off
            // the course.
            let off = if race.gates.is_empty() { me.off_course() } else { 0.0 };
            countdown::number(left, off).map(|k| (k, v))
        });
    for (k, &object) in c.numbers.iter().enumerate() {
        let place = match shown {
            Some((n, v)) if n == k => c.place(k, v),
            _ => hide(Mat4::IDENTITY),
        };
        renderer.set_object_place(object, place);
    }
}

/// The Visual Cue arrow and the runway lights (0x48c0a0, 0x48b700): only
/// in the live race's own view, with Visual Cues on and the player still
/// racing.
fn place_cues(race: &Race, track: &Track, cues: &cues::Cues, renderer: &mut TerrainRenderer, view: Option<cues::Eye>) {
    let hide = |place: Mat4| Mat4::from_cols(Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, place.w_axis);
    let me = &race.racers[race.player];
    let racing = race.visual_cues && race.ghost != Some(race.player) && !me.timing.finished && race.podium.is_none();
    let ground = |x: f32, z: f32| {
        let h = |x: f32, z: f32| ride::ground(&track.terrain, x, z);
        let normal = Vec3::new(h(x - 1.0, z) - h(x + 1.0, z), 2.0, h(x, z - 1.0) - h(x, z + 1.0)).normalize();
        (h(x, z), normal)
    };
    // Off the course in a lap race, the place to get back to.
    let rejoin = race.course.as_ref().and_then(|c| me.rejoin(c)).filter(|_| race.gates.is_empty());
    let aim = if let Some(t) = &race.tag {
        // IT, unless that is the player; with the ball, the ball.
        match (&t.ball, t.it) {
            (Some(b), _) => Some(b.position),
            (None, Some(it)) if it != race.player => Some(race.racers[it].bike.position()),
            _ => None,
        }
    } else if let Some(gate) = race.gates.get(me.timing.gate) {
        Some(gate.centre)
    } else {
        rejoin.map(|(at, _)| {
            let mut at = at;
            at.y = ground(at.x, at.z).0;
            at
        })
    };
    let arrow = match (view, aim) {
        (Some(view), Some(aim)) if racing => cues.arrow(view, aim),
        _ => hide(Mat4::IDENTITY),
    };
    renderer.set_object_place(cues.arrow, arrow);
    let lights = rejoin.filter(|_| racing && view.is_some() && race.tag.is_none());
    for (k, &object) in cues.lights.iter().enumerate() {
        let place = lights.map(|(at, way)| cues.lights(at, way, race.time, ground)[k]);
        renderer.set_object_place(
            object,
            match place {
                Some((place, true)) => place,
                Some((place, false)) => hide(place),
                None => hide(Mat4::IDENTITY),
            },
        );
    }
}

/// The flag girl, with the checkered flag and with the white one, and the
/// 30-second man, as animated objects.
#[derive(Debug, Clone, Copy, Default)]
struct Finish {
    girl: Option<([usize; 2], Mat4)>,
    man: Option<usize>,
}

/// The two podium models and each grid rider's `Winner.mcf`, as objects, and
/// the scene's own podium spot and direction.
#[derive(Debug, Clone, Default)]
struct Stand {
    models: [usize; 2],
    winners: Vec<usize>,
    site: Option<(Vec3, Vec3)>,
}

/// Moto-Tag's models in `RES/MODELS.RES`: the IT sign, the ball (0x4ad900)
/// and the sign over the player's own bike (0x419fe3's `youind.slt`).
const TAG_MODELS: [&str; 3] = ["Itind.slt", "BBHighLod.slt", "YouInd.slt"];

/// A ghost race's ghost bike and rider (`Ghost.slt`, `GhostRider.slt`).
const GHOST: (&str, &str) = ("Ghost.mcf", "GhostRider.mcf");

/// The rider's motion on the bike, and the bike's.
const RIDING: &str = "LeanR02";
const BIKE_RIDING: &str = "BLeanR02";

/// The player's bike from the game's `UI/UIRES.RES`: the Honda CR250,
/// a 250 (`bikes.pb`) with the first preset of its class (`presets.pb`).
fn player_spec(res: &Path, cc: u32) -> Spec {
    let presets = res
        .parent()
        .map(|game| game.join("UI").join("UIRES.RES"))
        .and_then(|p| read_archive(&p).ok())
        .and_then(|bytes| {
            let archive = rs2::Archive::parse(&bytes).ok()?;
            let text = String::from_utf8_lossy(archive.get("presets.pb")?).into_owned();
            Spec::from_presets(&text, spec::category(cc), Some(0))
        });
    presets.unwrap_or_else(|| {
        eprintln!("no presets.pb in UI/UIRES.RES, so the bike uses the default spec");
        Spec::default()
    })
}

/// Where a race starts, in game coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Start {
    position: Vec3,
    /// Level and of unit length.
    direction: Vec3,
    /// Where the grid's middle stands from `position`: ft behind it and
    /// ft left of it. None goes by the start line.
    grid: Option<(f32, f32)>,
}

/// Reads a track's `.ENV` archive, and the models it uses from the game's
/// `RES` folder when that can be found above it.
/// `rides` are each racer's bike, outfit and number from the menus, the
/// player's first; any left out take the default grid.
async fn load_track(
    path: &Path,
    course: usize,
    riders: usize,
    rides: Vec<Option<roster::Ride>>,
    me: usize,
    ghost: Option<usize>,
) -> Result<Track, BoxError> {
    let ride = rides.get(me).cloned().flatten();
    load_step().await;
    let track_bytes = read_archive(path)?;
    let archive = rs2::Archive::parse(&track_bytes)?;
    let find = |ext: &str| {
        archive
            .entries()
            .iter()
            .find(|e| e.name.to_ascii_lowercase().ends_with(ext))
    };
    let entry = find(".trn").ok_or("no terrain (.trn) in this archive")?;
    let terrain = trn::parse(archive.data(entry))?;
    load_step().await;
    let used_before = hires::pack().map_or(0, HiRes::used);
    let terrain_textures = terrain_textures(&terrain, &file_name(path), hires::pack());
    load_step().await;

    let scene = find(".scn")
        .map(|e| scn::Scene::parse(archive.data(e)))
        .transpose()?;
    let zones = scene.as_ref().map_or_else(Default::default, |s| s.zones);
    load_step().await;
    let (mut start, course, stem) = race_start(&archive, scene.as_ref(), &terrain, course);
    // A Baja or Enduro course's gates, from the `.wpt` beside its `.tdf`.
    let waypoints = stem
        .and_then(|stem| archive.get(&format!("{stem}.wpt")))
        .map(|b| gates::Waypoints::parse(&String::from_utf8_lossy(b)))
        .unwrap_or_default();
    let gate_list = waypoints.gates(|x, z| ride::ground(&terrain, x, z));
    load_step().await;
    // A course with no start line starts at its last gate, the finish
    // arch: the original's lone rider stands 7.96 ft through it and
    // 0.78 ft right of its middle (all six Baja and Enduro tapes).
    if course.as_ref().is_some_and(|c| c.start.is_none()) {
        if let Some(last) = gate_list.last() {
            start = Start {
                position: Vec3::new(last.position.x, start.position.y, last.position.z),
                direction: last.look,
                grid: Some((-7.96, -0.78)),
            };
        }
    }
    let name = scene.as_ref().map(|s| s.name.clone()).unwrap_or_default();
    let (mut grid, mut gate, mut arch_objects) = (Vec::new(), None, Vec::new());
    let mut people = Finish::default();
    let mut podium_objects = None;
    let mut tricks = Vec::new();
    let mut popup_models = None;
    let mut tag_models = None;
    let mut cue_models = None;
    let mut countdown_models = None;
    let mut world = mm2_physics::collision::World::default();
    let mut traffic = traffic::Traffic::default();
    let crowd = scene.as_ref().is_some_and(|s| s.crowd);
    let mut scene_sounds = Vec::new();
    let res = path
        .ancestors()
        .map(|dir| dir.join("RES"))
        .find(|res| crate::platform::fs::is_file(res.join("MODELS.RES")));
    let cc = ride.as_ref().map_or(250, |r| r.bike.cc);
    let spec = res
        .as_deref()
        .map(|res| player_spec(res, cc))
        .unwrap_or_default();
    let sky = load_sky(res.as_deref(), scene.as_ref());
    load_step().await;
    let fog = scene.as_ref().and_then(|s| s.fog_color);
    let haze = scene
        .as_ref()
        .filter(|s| s.cube_file.is_some())
        .map(|s| (s.fog_visibility, s.fog_haziness));
    let detail = res.as_deref().and_then(|r| detail_texture(r, scene.as_ref()));
    let particle_sheet = res.as_deref().and_then(|r| {
        let name = scene.as_ref().and_then(|s| s.particle_texture.as_deref());
        name.and_then(|n| global_texture(r, n)).or_else(|| dirt_sheet(r))
    });
    let scene_lights = scene
        .as_ref()
        .map_or_else(|| renderer::SceneLights::from_scene(&[]), |s| renderer::SceneLights::from_scene(&s.lights));
    let sun = scene
        .as_ref()
        .and_then(|s| {
            s.lights
                .iter()
                .find(|l| l.kind == scn::LightKind::Directional)
                .and_then(|l| l.direction)
        })
        .map(Vec3::from)
        .filter(|d| d.y < -1e-3)
        .unwrap_or(Vec3::new(1.0, -10.0, 0.0))
        .normalize();
    // The enhanced drawing's spectators: where each stands, looking which way.
    let mut spectators = Vec::new();
    let objects = match (scene, res.clone()) {
        (Some(scene), Some(res)) => {
            // Track makers pack their own models, textures and motions
            // into the track's file; they are found there first.
            let mut model_files = vec![read_archive(&res.join("MODELS.RES"))?, track_bytes.clone()];
            let mut texture_files = vec![
                (file_name(path).to_ascii_uppercase(), track_bytes.clone()),
                ("OBJECTS.RES".to_string(), read_archive(&res.join("OBJECTS.RES"))?),
            ];
            load_step().await;
            // Bikes and riders keep their models, motions and skins together.
            let mut animations = scene.animations.clone();
            let mut skins = Vec::new();
            let mut grid_sources = Vec::new();
            match read_archive(&res.join("BIKE.RES")) {
                Ok(bytes) => {
                    let bike = ride.as_ref().map_or(GRID[0].0, |r| r.bike.model.as_str());
                    tricks = rs2::Archive::parse(&bytes)
                        .map(|ar| trick_motions(&ar, bike))
                        .unwrap_or_default();
                    // The player's collision points, from the bike's `.col`.
                    if let Ok(ar) = rs2::Archive::parse(&bytes) {
                        let stem = bike.rsplit_once('.').map_or(bike, |(s, _)| s);
                        let col = ar
                            .get(&format!("{stem}.col"))
                            .or_else(|| ar.get("honda.col"))
                            .and_then(|d| mm2_formats::col::parse(d).ok());
                        if let Some(col) = col {
                            race::set_hull(mm2_physics::bike::Hull::new(&col));
                        }
                        if let Some(col) = ar.get("Rider.col").and_then(|d| mm2_formats::col::parse(d).ok()) {
                            let parts = col
                                .parts
                                .iter()
                                .filter_map(|p| {
                                    let corners = p.body.vertices.iter().map(|v| Vec3::from(*v)).collect();
                                    Some((p.frame? as usize, corners))
                                })
                                .collect();
                            let _ = RIDER_COL.set(parts);
                        }
                        if let Some(frames) = cross_up(&ar, bike) {
                            race::set_cross_up(frames);
                        }
                    }
                    model_files.push(bytes.clone());
                    texture_files.push(("BIKE.RES".to_string(), bytes));
                    grid_sources = start_grid(
                        &mut animations,
                        &mut skins,
                        &start,
                        &terrain,
                        riders,
                        &rides,
                        ghost,
                    );
                }
                Err(err) => eprintln!("{err}"),
            }
            // An outdoor track's trees and bushes, from `ECO.RES`.
            let mut statics = scene.models.clone();
            load_step().await;
            // A lap race's finish arch and flag girl (0x418cd0). Custom
            // tracks hide them with a finish line that has no direction:
            // the game turns them to face along it (0x500bb0), and with
            // nothing to face along they shrink to nothing.
            let finish_line = course
                .as_ref()
                .and_then(|c| c.finish)
                .filter(|l| l.direction[0] != 0.0 || l.direction[2] != 0.0)
                .filter(|_| gate_list.is_empty());
            let arch_width = rs2::Archive::parse(&model_files[0])
                .ok()
                .and_then(|ar| mm2_formats::slb::parse(ar.get("MetalArch.slb")?).ok())
                .map(|m| {
                    let x = m.lods.iter().take(1).flat_map(|l| &l.surfaces);
                    2.0 * x.flat_map(|s| &s.positions).fold(0.0f32, |w, p| w.max(p[0].abs()))
                });
            let mut girl_sources = None;
            let mut girl_place = Mat4::IDENTITY;
            if let (Some(line), Some(width)) = (finish_line, arch_width) {
                let (arch, girl_box, girl) =
                    finish::layout(&line, width, |x, z| ride::ground(&terrain, x, z));
                statics.push(arch);
                statics.push(girl_box);
                skins.resize(animations.len(), None);
                girl_sources = Some([animations.len(), animations.len() + 1]);
                girl_place = objects::orientation(girl.look, girl.up, Vec3::ONE, girl.offset);
                animations.push(girl.clone());
                animations.push(girl);
                skins.push(None);
                skins.push(Some(format!("{}{}", objects::SECOND, finish::WHITE)));
            }
            let man_source = animations
                .iter()
                .position(|a| a.model.eq_ignore_ascii_case(finish::MAN));
            load_step().await;
            // Each rider on the podium, in their own outfit (0x45e490).
            skins.resize(animations.len(), None);
            let winner_sources: Vec<usize> = grid_sources
                .iter()
                .map(|&bike| {
                    let outfit = skins[bike + 1].clone();
                    animations.push(scn::Animation {
                        model: podium::WINNER.into(),
                        offset: [0.0; 3],
                        look: [0.0, 0.0, 1.0],
                        up: [0.0, 1.0, 0.0],
                        scale: [1.0; 3],
                        motion: Some("Podium4_01".into()),
                        lighting: Default::default(),
                    });
                    skins.push(outfit);
                    animations.len() - 1
                })
                .collect();
            let mut trunks = Vec::new();
            if let Some(eco) = find(".esb").and_then(|e| {
                mm2_formats::esb::parse(archive.data(e))
                    .map_err(|err| eprintln!("{}: {err}", e.name))
                    .ok()
            }) {
                match read_archive(&res.join("ECO.RES")) {
                    Ok(bytes) => {
                        let (placed, solid) =
                            eco::place(&eco, &rs2::Archive::parse(&bytes)?, &terrain);
                        eprintln!("{} trees and bushes, {} trunks", placed.len(), solid.len());
                        statics.extend(placed);
                        trunks = solid;
                        model_files.push(bytes.clone());
                        texture_files.push(("ECO.RES".to_string(), bytes));
                    }
                    Err(err) => eprintln!("{err}"),
                }
            }
            load_substep().await;
            for name in &scene.resource_files {
                let name = name.to_ascii_uppercase();
                match read_archive(&res.join(&name)) {
                    Ok(bytes) => texture_files.push((name, bytes)),
                    Err(err) => eprintln!("{err}"),
                }
            }
            let textures = texture_files
                .iter()
                .map(|(name, b)| Ok((name.as_str(), rs2::Archive::parse(b)?)))
                .collect::<Result<Vec<_>, mm2_formats::Error>>()?;
            let models = model_files
                .iter()
                .map(|b| rs2::Archive::parse(b))
                .collect::<Result<Vec<_>, _>>()?;
            let arches = waypoints.placements(&gate_list);
            let procvue = read_archive(&res.join("PROCVUE.RES")).unwrap_or_default();
            let procvue = rs2::Archive::parse(&procvue).ok();
            let vehicles = traffic::load(&scene, procvue.as_ref());
            let mut movable = arches.clone();
            movable.extend(popup::Popup::placements());
            movable.extend(TAG_MODELS.map(|m| mm2_formats::scn::Placement {
                model: m.into(),
                position: [0.0; 3],
                look: [0.0, 0.0, 1.0],
                up: [0.0, 1.0, 0.0],
                scale: [0.0; 3],
                lighting: Default::default(),
            }));
            movable.extend(podium::MODELS.map(|m| mm2_formats::scn::Placement {
                model: m.into(),
                position: [0.0; 3],
                look: [0.0, 0.0, 1.0],
                up: [0.0, 1.0, 0.0],
                scale: [0.0; 3],
                lighting: Default::default(),
            }));
            movable.extend(vehicles.iter().map(|(_, _, model)| model.clone()));
            movable.extend(cues::MODELS.map(|m| mm2_formats::scn::Placement {
                model: m.into(),
                position: [0.0; 3],
                look: [0.0, 0.0, 1.0],
                up: [0.0, 1.0, 0.0],
                scale: [0.0; 3],
                lighting: Default::default(),
            }));
            movable.extend(countdown::MODELS.map(|m| mm2_formats::scn::Placement {
                model: m.into(),
                position: [0.0; 3],
                look: [0.0, 0.0, 1.0],
                up: [0.0, 1.0, 0.0],
                scale: [0.0; 3],
                lighting: Default::default(),
            }));
            // The enhanced drawing's trackside dressing, outdoors.
            let desert = path.parent().and_then(|d| d.file_name()).is_some_and(|d| d.eq_ignore_ascii_case("BAJA"));
            let extras;
            (extras, spectators) = match crate::graphics::enhanced() {
                Some(e) if e.scenery && scene.stadium.is_none() => {
                    let gates: Vec<Vec3> = gate_list.iter().map(|g| g.position).collect();
                    let placed = scenery::place(&terrain, course.as_ref(), &gates, (start.position, start.direction), &statics, desert);
                    eprintln!("{} trackside props", placed.0.len());
                    placed
                }
                _ => (Vec::new(), Vec::new()),
            };
            load_substep().await;
            let mut objects = ObjectMesh::build(
                &statics,
                &extras,
                &movable,
                &animations,
                &skins,
                &models,
                &textures,
                hires::pack(),
            );
            objects.surfaced = materials::surfaced(&file_name(path));
            load_substep().await;
            match read_archive(&res.join("COLSEG.RES")) {
                Ok(bytes) => {
                    let colseg = rs2::Archive::parse(&bytes)?;
                    world = ground::solids(
                        &scene.models,
                        &colseg,
                        &models,
                        scene.stadium.as_deref(),
                    );
                    // A stadium's walls hold the bikes in.
                    if let (Some(seg), Some(frame)) = (
                        scene.stadium.as_deref().and_then(|n| colseg.get(n)),
                        &scene.stadium_frame,
                    ) {
                        let walls = ground::stadium_walls(&String::from_utf8_lossy(seg), frame);
                        if crate::graphics::enhanced().is_some_and(|e| e.scenery) {
                            let outline = ground::stadium_outline(&String::from_utf8_lossy(seg), frame);
                            spectators = scenery::stadium_crowd(&outline, frame.bottom, |p| objects.surface_below(p, frame.bottom + 150.0));
                        }
                        eprintln!("{} stadium walls", walls.len());
                        world.solids.extend(walls);
                    }
                    world.trunks = mm2_physics::collision::Trunks::new(trunks);
                    traffic.first = world.solids.len();
                    for (source, vehicle, model) in &vehicles {
                        let first = world.solids.len();
                        world.solids.extend(traffic::solids(&model.model, &colseg));
                        traffic.cars.push(traffic::Car {
                            vehicle: vehicle.clone(),
                            solids: first..world.solids.len(),
                            object: None,
                            source: *source,
                        });
                    }
                    eprintln!("{} scripted vehicles", traffic.cars.len());
                    eprintln!("{} solid objects", world.solids.len());
                }
                Err(err) => eprintln!("{err}"),
            }
            let object = |source: usize| objects.sources.iter().position(|&s| s == source);
            grid = grid_sources.iter().filter_map(|&s| object(s)).collect();
            arch_objects = arches
                .iter()
                .enumerate()
                .filter_map(|(j, p)| {
                    let place = objects::orientation(p.look, p.up, Vec3::from(p.scale), p.position);
                    Some((object(animations.len() + j)?, place))
                })
                .collect();
            let first = animations.len() + arches.len();
            let score: Vec<_> = (first..first + popup::Popup::placements().len())
                .map(object)
                .collect();
            popup_models = popup::Popup::new(&score);
            let first = first + popup::Popup::placements().len();
            let found: Option<Vec<usize>> = (first..first + TAG_MODELS.len()).map(object).collect();
            tag_models = found.and_then(|f| f.try_into().ok());
            let first = first + TAG_MODELS.len();
            podium_objects = object(first).zip(object(first + 1)).map(|(a, b)| Stand {
                models: [a, b],
                winners: winner_sources.iter().filter_map(|&w| object(w)).collect(),
                site: scene.podium.map(|(p, d)| (Vec3::from(p), Vec3::from(d))),
            });
            let first = first + podium::MODELS.len();
            load_substep().await;
            for (j, car) in traffic.cars.iter_mut().enumerate() {
                car.object = object(first + j);
            }
            let first = first + vehicles.len();
            let found: Option<Vec<usize>> = (first..first + cues::MODELS.len()).map(object).collect();
            cue_models = found.map(|f| cues::Cues {
                arrow: f[0],
                lights: [f[1], f[2], f[3], f[4], f[5]],
                size: model_size(&models, "viscue.slb").unwrap_or(1.0),
            });
            let first = first + cues::MODELS.len();
            let found: Option<Vec<usize>> = (first..first + countdown::MODELS.len()).map(object).collect();
            countdown_models = found.map(|f| countdown::Countdown {
                numbers: [f[0], f[1], f[2], f[3], f[4]],
                sizes: countdown::MODELS.map(|m| model_size(&models, &m.replace(".slt", ".slb")).unwrap_or(1.0)),
            });
            load_step().await;
            // The scene's ambient sounds, on what they hang on.
            for sound in &scene.sounds {
                let site = match &sound.site {
                    scn::SoundSite::Fixed(p) => Some(sound::Site::Fixed(Vec3::from(*p))),
                    scn::SoundSite::Model(i) => {
                        scene.models.get(*i).map(|m| sound::Site::Fixed(Vec3::from(m.position)))
                    }
                    scn::SoundSite::Animation(i, frame) => object(*i).map(|o| {
                        let f = frame.as_deref().and_then(|f| objects.animated[o].frame_named(f));
                        sound::Site::Object(o, f)
                    }),
                    scn::SoundSite::Procedural(i) => traffic
                        .cars
                        .iter()
                        .position(|c| c.source == *i)
                        .map(sound::Site::Car),
                };
                if let Some(site) = site {
                    scene_sounds.push((sound.spec.clone(), site));
                }
            }
            gate = animations
                .iter()
                .position(|a| a.model.eq_ignore_ascii_case("StartGate.mcf"))
                .and_then(object);
            people = Finish {
                girl: girl_sources.and_then(|[a, b]| Some(([object(a)?, object(b)?], girl_place))),
                man: man_source.and_then(object),
            };
            eprintln!(
                "{}: placed {} of {} objects ({} animated)",
                scene.name,
                objects.placed,
                scene.models.len() + animations.len(),
                objects.animated.len()
            );
            if !objects.missing_models.is_empty() {
                eprintln!("models not found: {}", objects.missing_models.join(", "));
            }
            objects
        }
        (_, None) => {
            eprintln!("no RES/MODELS.RES above the track, so no objects");
            ObjectMesh::default()
        }
        _ => ObjectMesh::default(),
    };
    if let Some(h) = hires::pack() {
        eprintln!("using {} high-res textures", h.used() - used_before);
    }
    load_step().await;
    Ok(Track {
        terrain,
        terrain_textures,
        world,
        spec,
        objects,
        start,
        course,
        grid,
        gate,
        people,
        podium: podium_objects,
        res,
        name,
        supercross: path.parent().and_then(|d| d.file_name()).is_some_and(|d| d.eq_ignore_ascii_case("SX")),
        desert: path.parent().and_then(|d| d.file_name()).is_some_and(|d| d.eq_ignore_ascii_case("BAJA")),
        gates: gate_list.iter().map(gates::Gate::line).collect(),
        arches: arch_objects,
        tricks,
        popup: popup_models,
        tag: tag_models,
        cues: cue_models,
        countdown: countdown_models,
        sky,
        fog,
        haze,
        sun,
        lights: scene_lights,
        detail,
        particle_sheet,
        traffic,
        zones,
        crowd,
        spectators,
        sounds: scene_sounds,
    })
}

/// Twice a model's largest half extent, its most detailed LOD's (0x5036e0,
/// as an `ArcadeObject` sizes itself on the screen).
fn model_size(models: &[rs2::Archive], name: &str) -> Option<f32> {
    let data = models.iter().find_map(|m| m.get(name))?;
    let model = mm2_formats::slb::parse(data).ok()?;
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in model.lods.first()?.surfaces.iter().flat_map(|s| &s.positions) {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    (hi.x >= lo.x).then(|| (hi - lo).max_element())
}

/// The scene's sky box from `RES/SKIES.RES`.
/// The dirt particles' sheet, `DirtPart.TGA` in `RES/GLOBAL.RES`.
/// The scene's detail texture from `RES/GLOBAL.RES`: the one it names, or
/// `GrayNoise.tga`. A named one the archive lacks (the ski lodge's
/// `whitenoise.tga`) leaves the terrain without.
fn detail_texture(res: &std::path::Path, scene: Option<&scn::Scene>) -> Option<mm2_formats::tga::Image> {
    let name = scene.and_then(|s| s.detail_texture.as_deref()).unwrap_or("GrayNoise.tga");
    global_texture(res, name)
}

/// A texture in `RES/GLOBAL.RES`.
fn global_texture(res: &std::path::Path, name: &str) -> Option<mm2_formats::tga::Image> {
    let raw = crate::platform::fs::read(res.join("GLOBAL.RES")).ok()?;
    let bytes = mm2_formats::faoe::decode(&raw, "GLOBAL.RES").unwrap_or(raw);
    let archive = mm2_formats::rs2::Archive::parse(&bytes).ok()?;
    let data = archive
        .entries()
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(name))
        .map(|e| archive.data(e))?;
    mm2_formats::texture::parse_overlay(data).ok()
}

fn dirt_sheet(res: &std::path::Path) -> Option<mm2_formats::tga::Image> {
    let raw = crate::platform::fs::read(res.join("GLOBAL.RES")).ok()?;
    let bytes = mm2_formats::faoe::decode(&raw, "GLOBAL.RES").unwrap_or(raw);
    let archive = mm2_formats::rs2::Archive::parse(&bytes).ok()?;
    let data = archive
        .entries()
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case("DirtPart.TGA"))
        .map(|e| archive.data(e))?;
    mm2_formats::texture::parse_overlay(data).ok()
}

fn load_sky(
    res: Option<&Path>,
    scene: Option<&scn::Scene>,
) -> Option<(mm2_formats::cub::Cube, hires::SkyImages)> {
    let name = scene?.cube_file.as_deref()?;
    let skies = read_archive(&res?.join("SKIES.RES")).ok()?;
    let skies = rs2::Archive::parse(&skies).ok()?;
    match mm2_formats::cub::Cube::parse(skies.get(name)?) {
        Ok(cube) => {
            let images = hires::sky_images(&cube, name, hires::pack());
            Some((cube, images))
        }
        Err(e) => {
            eprintln!("sky {name}: {e}");
            None
        }
    }
}

/// The bike model's root frame on each frame of its cross-up motion
/// (`CrosBik1`), as turns of the bike.
fn cross_up(bike_res: &rs2::Archive, bike: &str) -> Option<Vec<glam::Quat>> {
    let mcf = mm2_formats::anim::Mcf::parse(bike_res.get(bike)?).ok()?;
    let info = mcf
        .motions
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case("CrosBik1"))?;
    let motion = mm2_formats::anim::Motion::parse(bike_res.get(&info.file)?).ok()?;
    motion
        .frames
        .iter()
        .map(|keys| {
            let k = keys.iter().find(|k| k.frame == 0)?;
            let look = glam::Vec3::from(k.look).normalize_or(glam::Vec3::Z);
            let right = glam::Vec3::from(k.up).cross(look).normalize_or(glam::Vec3::X);
            let up = look.cross(right);
            Some(glam::Quat::from_mat3(&glam::Mat3::from_cols(right, up, look)))
        })
        .collect()
}

/// The rider's and the bike's motions for each trick, from `Rider.mcf` and
/// the bike's `.mcf` (the bike's are named `b<trick>`).
fn trick_motions(bike_res: &rs2::Archive, bike: &str) -> Vec<Option<race::TrickMotion>> {
    let mcf = |name: &str| {
        bike_res
            .get(name)
            .and_then(|b| mm2_formats::anim::Mcf::parse(b).ok())
    };
    let (Some(rider), Some(bike)) = (mcf("Rider.mcf"), mcf(bike)) else {
        return Vec::new();
    };
    let names = mm2_physics::stunt::TRICKS
        .iter()
        .map(|t| t.to_string())
        .chain(mm2_physics::stunt::TRICKS.iter().map(|t| format!("{t}Alt")));
    names
        .map(|name| {
            let r = rider
                .motions
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(&name))?;
            let b = format!("b{name}");
            let b = bike
                .motions
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(&b));
            Some(race::TrickMotion {
                rider: r.name.clone(),
                bike: b.map_or(String::new(), |b| b.name.clone()),
                fps: r.fps,
                frames: r.frame_count as i32,
            })
        })
        .collect()
}

/// The start line from the track definition (`.tdf`), or the scene's
/// default start when there is none.
fn race_start(
    archive: &rs2::Archive,
    scene: Option<&scn::Scene>,
    terrain: &Terrain,
    which: usize,
) -> (Start, Option<TrackDef>, Option<String>) {
    // Tracks with several courses (Baja, enduro) number them.
    let mut defs: Vec<_> = archive
        .entries()
        .iter()
        .filter(|e| e.name.to_ascii_lowercase().ends_with(".tdf"))
        .collect();
    defs.sort_by_key(|e| e.name.to_ascii_lowercase());
    let def = defs.get(which).or(defs.first());
    let stem = def.map(|e| e.name[..e.name.len() - 4].to_string());
    let course = def.and_then(|e| {
        tdf::parse(archive.data(e))
            .map_err(|err| eprintln!("{}: {err}", e.name))
            .ok()
    });
    let line = course.as_ref().and_then(TrackDef::start_line);
    let (mut position, direction) = match (line, scene) {
        (Some(line), _) => (Vec3::from(line.position), Vec3::from(line.direction)),
        (None, Some(scene)) => (
            Vec3::from(scene.start_position),
            Vec3::from(scene.start_direction),
        ),
        (None, None) => {
            let mid = (terrain.size / 2) as f32 * terrain.scale;
            (Vec3::new(mid, 0.0, mid), Vec3::Z)
        }
    };
    position.y = ride::ground(terrain, position.x, position.z);
    let direction = Vec3::new(direction.x, 0.0, direction.z).normalize_or(Vec3::Z);
    (
        Start {
            position,
            direction,
            grid: None,
        },
        course,
        stem,
    )
}

/// Bikes, their riders' outfits from `RES/BIKE.RES`.
/// The player rides the first; the computer's riders take the rest in turn.
const GRID: [(&str, &str); 8] = [
    ("honda.mcf", "FOX02_360Red_08.tga"),
    ("yamaha.mcf", "FOX02_360Navy_08.tga"),
    ("ktm.mcf", "FOX02_360Orange_08.tga"),
    ("suzuki.mcf", "FOX02_360Yellow_08.tga"),
    ("honda.mcf", "FOX02_360Green_08.tga"),
    ("yamaha.mcf", "FOX02_360Purple_08.tga"),
    ("ktm.mcf", "FOX02_360Black_08.tga"),
    ("suzuki.mcf", "FOX02_360Patriot_08.tga"),
];
/// The player's name until there are profiles (the game takes it from
/// the one chosen on the main menu).
const PLAYER_NAME: &str = "Player";

/// A racer's name: the one the menus gave, else the player's stand-in or
/// a computer rider's from the game's list (`LANG.DLL` 5000 to 5035;
/// which rider gets which name is our pick).
fn rider_name(race: &Race, ui: &ui::Ui, rider: usize) -> String {
    match race.names.get(rider).filter(|n| !n.is_empty()) {
        Some(name) => name.clone(),
        None if rider == race.player => PLAYER_NAME.to_string(),
        None => ui.lang(5000 + (rider as u32 * 7 + 3) % 36),
    }
}

/// The Race Info page's lines for a race, in finishing order (`0x4521b0`).
fn results(race: &Race, ui: &ui::Ui) -> frontend::RaceResults {
    let unset = "-:--.--".to_string();
    let lines = race
        .standings()
        .into_iter()
        .enumerate()
        .map(|(place, i)| {
            let r = &race.racers[i];
            let name = rider_name(race, ui, i);
            let t = &r.timing;
            if race.stunt_event {
                // The Stunt Quarry lists names and scores (result +0x2c).
                return [
                    name,
                    String::new(),
                    format!("{:.0}", r.stunt_points),
                    String::new(),
                ];
            }
            let best = if t.best_lap > 0.0 {
                hud::clock_text(t.best_lap)
            } else {
                unset.clone()
            };
            let total = if t.finished {
                hud::clock_text(t.race_time)
            } else {
                ui.lang(4080)
            };
            [
                name,
                (place + 1).to_string(),
                format!("{best} ({total})"),
                String::new(),
            ]
        })
        .collect();
    let mut placings = vec![frontend::Placing::default(); race.racers.len()];
    for (place, i) in race.standings().into_iter().enumerate() {
        let r = &race.racers[i];
        placings[i] = frontend::Placing {
            place: place + 1,
            repair: r.repair,
            medical: r.medical,
            stunt: r.stunt_units,
        };
    }
    frontend::RaceResults {
        lines,
        stunt: race.stunt_event,
        placings,
        replay: None,
    }
}

/// The number on each rider's plates. The game takes the player's from
/// the profile (EditPlateNumber) and the computer riders' from their
/// records; these are our picks until those are read.
fn plate_number(rider: usize) -> u32 {
    [7, 22, 4, 13, 31, 2, 18, 9, 45, 11, 26][rider % 11]
}

/// Which of them the player rides.
const PLAYER: usize = 0;
/// Riders when the command line starts a race: the player and three.
const RIDERS: usize = 4;
/// Bikes side by side in a row at the start.
const ROW: usize = 8;
/// The start grid's spacing, and how far left of the line's middle it
/// sits, ft.
const SLOT_WIDTH: f32 = 7.75;
const GRID_OFFSET: f32 = 3.03;
/// `--spawn x,z,heading`: where the first bike stands instead, to replay
/// a tape from exactly its start.
static SPAWN: std::sync::OnceLock<(Vec3, Vec3)> = std::sync::OnceLock::new();

/// Moves the start gate, and whatever the scene placed with it (the
/// 30-second board man), onto the start line as the game does, and lines up
/// bikes and riders behind it. Returns the bikes' indices in `animations`;
/// `skins` gets a rider outfit for each animation.
fn start_grid(
    animations: &mut Vec<scn::Animation>,
    skins: &mut Vec<Option<String>>,
    start: &Start,
    terrain: &Terrain,
    riders: usize,
    rides: &[Option<roster::Ride>],
    ghost: Option<usize>,
) -> Vec<usize> {
    let gate = animations
        .iter()
        .find(|a| a.model.eq_ignore_ascii_case("StartGate.mcf"))
        .map(|g| g.offset);
    let (d, up) = (start.direction, Vec3::Y);
    let right = up.cross(d);
    if let Some(gate) = gate {
        for a in animations.iter_mut().filter(|a| a.offset == gate) {
            a.offset = start.position.into();
            a.look = d.into();
            a.up = up.into();
        }
    }
    // Behind the line: the original's bikes stand 4.7 ft back (its tapes,
    // gate or not); clear of the bars where there is no line to go by.
    let (back, left) = start
        .grid
        .unwrap_or((if gate.is_some() { 4.6 } else { 2.0 }, GRID_OFFSET));
    skins.resize(animations.len(), None);
    let mut bikes = Vec::new();
    for i in 0..riders.max(1) {
        let (mut bike, mut outfit) = (
            GRID[i % GRID.len()].0.to_string(),
            GRID[i % GRID.len()].1.to_string(),
        );
        let mut skin = Some(format!("{}{}", objects::PLATE, plate_number(i)));
        let mut rider = "Rider.mcf";
        if let Some(Some(r)) = rides.get(i) {
            bike = r.bike.model.clone();
            outfit = r.outfit.clone();
            // No paint (a replay of a computer rider on the default grid)
            // keeps the model's own; an empty one would put the number
            // plate's texture on the whole bike.
            skin = Some(match r.bike.texture.as_str() {
                "" => format!("{}{}", objects::PLATE, r.plate),
                paint => format!("{}{}:{paint}", objects::PLATE, r.plate),
            });
        }
        // A ghost race's ghost rides its own models (0x41a770).
        let ghostly = ghost == Some(i);
        if ghostly {
            (bike, rider, skin) = (GHOST.0.into(), GHOST.1, None);
        }
        // Rows of up to eight, the player on the right: the original's
        // tapes put them 7.75 ft apart, centred 3 ft left of the line's
        // middle (five riders on SX01; a lone practice rider on every
        // supercross track).
        let (row, slot) = (i / ROW, i % ROW);
        let in_row = (riders - row * ROW).min(ROW);
        let across = (in_row as f32 - 1.0) / 2.0 - slot as f32;
        let mut at = start.position - d * (back + row as f32 * 8.0)
            + right * (across * SLOT_WIDTH - left);
        let mut look = d;
        if let (0, Some(&(spot, way))) = (i, SPAWN.get()) {
            (at, look) = (spot, way);
        }
        at.y = ride::ground(terrain, at.x, at.z);
        let place = |model: &str, motion: &str| scn::Animation {
            model: model.to_string(),
            offset: at.into(),
            look: look.into(),
            up: up.into(),
            scale: [1.0; 3],
            motion: Some(motion.to_string()),
            lighting: Default::default(),
        };
        bikes.push(animations.len());
        animations.push(place(&bike, BIKE_RIDING));
        skins.push(skin);
        animations.push(place(rider, RIDING));
        skins.push((!ghostly).then_some(outfit));
    }
    let p = start.position;
    eprintln!("start line at {:.0},{:.0},{:.0}", p.x, p.y, p.z);
    bikes
}

/// Two copies racing over the network, headless, for testing: each rides
/// on autopilot for `seconds` of real time and says where the other's
/// bike is.
fn net_test(
    track: &Track,
    how: &str,
    seconds: f32,
    tag: Option<(f32, bool)>,
) -> Result<(), BoxError> {
    let ball = tag.is_some_and(|t| t.1);
    let me = |name: &str| net::Player {
        name: name.into(),
        ready: true,
        ..Default::default()
    };
    let session = if how == "host" {
        net::Session::host("test", me("Host"))?
    } else {
        let addr = how.strip_prefix("join:").ok_or("--net host or join:ADDR")?;
        net::Session::join(addr.parse()?, me("Guest"))?
    };
    net::set(Some(session));
    let started = Instant::now();
    let n = loop {
        if started.elapsed().as_secs() > 20 {
            return Err("nobody came".into());
        }
        let heard = net::with(|s| s.poll()).unwrap_or_default();
        let (host, players, me) = net::with(|s| (s.host, s.players.clone(), s.me)).unwrap();
        if host && players.len() >= 2 {
            net::with(|s| {
                s.say(net::Msg::Start {
                    game: net::GameInfo::default(),
                    players: players.clone(),
                })
            });
            break frontend::NetRace {
                me: 0,
                humans: players.len(),
                host: true,
                tag_ball: ball,
                tag_stunts: false,
            };
        }
        if let Some(net::Heard::Start { players, .. }) = heard
            .into_iter()
            .find(|h| matches!(h, net::Heard::Start { .. }))
        {
            let at = players.iter().position(|p| p.id == me).unwrap_or(0);
            break frontend::NetRace {
                me: at,
                humans: players.len(),
                host: false,
                tag_ball: ball,
                tag_stunts: false,
            };
        }
        crate::platform::thread::sleep(web_time::Duration::from_millis(10));
    };
    let mut r = make_race_as(track, race::ENDLESS, n.me).ok_or("no bikes")?;
    // Scripted: the game's own steps, frame by frame.
    r.fixed_step = false;
    r.ai_frame = 0.0;
    r.autopilot = true;
    if let Some((minutes, ball)) = tag {
        let mut t = tag::Tag::new(r.racers.len(), ball, false);
        t.judge = n.host;
        t.local = (0..r.racers.len()).map(|i| owned(n, i)).collect();
        r.tag = Some(t);
        r.minutes = Some(minutes);
    }
    let mut remote: Vec<replay::Remote> = (0..r.racers.len()).map(|_| Default::default()).collect();
    for (i, racer) in r.racers.iter_mut().enumerate() {
        if !owned(n, i) {
            racer.bike = replay::taped_like(&racer.bike);
        }
    }
    let mut wait = 0.0;
    let step = 1.0 / 60.0;
    let start = Instant::now();
    let mut k = 0u32;
    while start.elapsed().as_secs_f32() < seconds {
        r.update(&track.ground(), Controls::default(), step);
        let _ = net_race(n, &mut r, &mut remote, &mut wait, &track.terrain, step);
        k += 1;
        let ahead = k as f32 * step - start.elapsed().as_secs_f32();
        if ahead > 0.0 {
            crate::platform::thread::sleep(web_time::Duration::from_secs_f32(ahead));
        }
    }
    for (i, racer) in r.racers.iter().enumerate() {
        let p = racer.bike.position();
        let whose = if owned(n, i) { "mine" } else { "theirs" };
        eprintln!("racer {i} ({whose}) at {:.1},{:.1},{:.1}", p.x, p.y, p.z);
    }
    if let Some(t) = &r.tag {
        let times: Vec<String> = (0..r.racers.len())
            .map(|i| race::time(t.time_it(i)))
            .collect();
        eprintln!("tag: IT {:?}, time as IT {times:?}; {}", t.it, r.status());
    }
    net::set(None);
    Ok(())
}

/// Whether this machine moves racer `i` in a network race: its player's,
/// and the host's computer riders on the host.
fn owned(n: frontend::NetRace, i: usize) -> bool {
    i == n.me || (n.host && i >= n.humans)
}

/// A network race's traffic each frame: the other machines' bikes in, and
/// ours out at [`replay::NET_SEND`], as the game's whole-bike packets.
fn net_race(
    n: frontend::NetRace,
    race: &mut Race,
    remote: &mut [replay::Remote],
    wait: &mut f32,
    terrain: &Terrain,
    dt: f32,
) -> Vec<String> {
    let mut chat = Vec::new();
    for h in net::with(|s| s.poll()).unwrap_or_default() {
        if let net::Heard::Chat(line) = &h {
            chat.push(line.clone());
        }
        if let net::Heard::Bike {
            racer, kind, data, ..
        } = &h
        {
            tag_heard(n, race, *racer, *kind, data);
        }
        if let net::Heard::Bike {
            racer,
            time,
            kind: mm2_formats::vcr::FULL,
            data,
        } = h
        {
            if !n.host
                && racer == 0
                && !race.gate_down()
                && time < race.lead
                && (time - race.time).abs() > 0.05
            {
                race.time = time;
            }
            let i = racer as usize;
            if let (Some(st), Some(rm), false) = (
                mm2_formats::vcr::State::full(&data),
                remote.get_mut(i),
                owned(n, i),
            ) {
                rm.push(time, st);
            }
        }
    }
    let at = race.time;
    for (i, rm) in remote.iter_mut().enumerate() {
        if owned(n, i) {
            continue;
        }
        if let (Some(st), Some(r)) = (rm.at(at), race.racers.get_mut(i)) {
            let t = replay::Taped::from_state(&st, terrain);
            if t.trick != r.stunt.trick {
                r.trick_clock = 0.0;
            } else {
                r.trick_clock += dt;
            }
            r.stunt.trick = t.trick;
            r.bike = race::Bike::Taped(Box::new(t));
        }
    }
    tag_say(race);
    *wait -= dt;
    if *wait > 0.0 {
        return chat;
    }
    *wait += replay::NET_SEND;
    if *wait < 0.0 {
        *wait = replay::NET_SEND;
    }
    let order = race.standings();
    let clock = (race.time * 1000.0) as u32;
    let msgs: Vec<net::Msg> = (0..race.racers.len())
        .filter(|&i| owned(n, i))
        .map(|i| {
            let place = order.iter().position(|&k| k == i).unwrap_or(0) as u8 + 1;
            let st = replay::state_of(&race.racers[i], clock, place, i as u8, race.clock());
            net::Msg::Bike {
                racer: i as u8,
                time: race.time,
                kind: mm2_formats::vcr::FULL,
                data: st.write_full(),
            }
        })
        .collect();
    net::with(|s| {
        for m in msgs {
            s.say(m);
        }
    });
    chat
}

/// Moto-Tag's packets: who is IT, from the host (0x87), and a player's
/// claim to the host (0x87 + 0x8c); one byte, the racer or 0xff for none.
const TAG_IT: i32 = 0x87;
const TAG_CLAIM: i32 = 0x87 + 0x8c;

fn tag_heard(n: frontend::NetRace, race: &mut Race, racer: u8, kind: i32, data: &[u8]) {
    let who = |b: Option<&u8>| b.copied().filter(|&b| b != 0xff).map(usize::from);
    let time = race.time;
    let mut points: Vec<f32> = race.racers.iter().map(|r| r.stunt_points).collect();
    let Some(t) = &mut race.tag else { return };
    match kind {
        TAG_IT if !n.host => t.pass(who(data.first()), time, &mut points),
        TAG_CLAIM if n.host => {
            t.claim(racer as usize, who(data.first()), time, &mut points);
        }
        _ => return,
    }
    for (r, p) in race.racers.iter_mut().zip(points) {
        r.stunt_points = p;
    }
}

/// Sends the host's IT changes, or this player's claims.
fn tag_say(race: &mut Race) {
    let Some(t) = &mut race.tag else { return };
    let byte = |w: Option<usize>| vec![w.map_or(0xff, |w| w as u8)];
    let mut msgs: Vec<net::Msg> = t
        .changes
        .drain(..)
        .map(|w| net::Msg::Bike {
            racer: w.map_or(0xff, |w| w as u8),
            time: race.time,
            kind: TAG_IT,
            data: byte(w),
        })
        .collect();
    msgs.extend(t.claims.drain(..).map(|(who, w)| net::Msg::Bike {
        racer: who as u8,
        time: race.time,
        kind: TAG_CLAIM,
        data: byte(w),
    }));
    if msgs.is_empty() {
        return;
    }
    net::with(|s| {
        for m in msgs {
            s.say(m);
        }
    });
}

/// The race's sounds are heard from the camera at `eye`, looking at
/// `target`, moving with the player's bike (0x4e84a0).
fn listener(r: &Race, eye: Vec3, target: Vec3) -> sound::Listener {
    let f = target - eye;
    sound::Listener {
        eye,
        right: Vec3::new(f.z, 0.0, -f.x).normalize_or_zero(),
        velocity: r.racers[r.player].bike.velocity(),
    }
}

/// Builds the header saved at the start of a race recording: which
/// track and event, and every rider's bike and outfit.
fn heading(setup: &frontend::Setup, race: &Race) -> replay::Heading {
    let riders = (0..race.racers.len())
        .map(|i| {
            let ride = if i == PLAYER {
                setup.ride.clone()
            } else {
                setup.rides.get(i).cloned().flatten()
            };
            let (model, skin, outfit, cc) = match &ride {
                Some(r) => (
                    r.bike.model.clone(),
                    r.bike.texture.clone(),
                    r.outfit.clone(),
                    r.bike.cc,
                ),
                None => {
                    let g = GRID[i % GRID.len()];
                    (g.0.to_string(), String::new(), g.1.to_string(), 250)
                }
            };
            mm2_formats::vcr::Rider {
                cc,
                flags: (i == PLAYER) as u32 * 7,
                id: i as u32,
                plate: ride.as_ref().map_or(plate_number(i), |r| r.plate),
                computer: i != PLAYER,
                name: setup.names.get(i).cloned().unwrap_or_default(),
                bike_model: model,
                rider_model: "rider.mcf".into(),
                bike_skin: skin,
                rider_skin: outfit,
            }
        })
        .collect();
    replay::Heading {
        track: setup
            .track
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string(),
        course: setup.course,
        event: setup.event as u32,
        mode: setup.mode,
        laps: setup.laps,
        riders,
    }
}

/// The race a tape was made in, to load its track and riders.
fn replay_setup(game: &Path, tape: &mm2_formats::vcr::Tape) -> Option<frontend::Setup> {
    use mm2_formats::vcr::setting;
    let event = frontend::EventKind::from_number(tape.setting(setting::EVENT))?;
    let stem = tape.track();
    let course = tape.course();
    let entries = frontend::list_tracks(&game.join(event.folder()), event);
    let entry = entries
        .iter()
        .filter(|e| {
            e.path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case(&stem))
        })
        .find(|e| e.course == course || !event.courses())?
        .clone();
    let ride = |r: &mm2_formats::vcr::Rider, i: usize| {
        // "Res\suzuki.mcf": the model's name in `RES/BIKE.RES`.
        let model = r
            .bike_model
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or("")
            .to_string();
        (!model.is_empty()).then(|| roster::Ride {
            bike: roster::BikeChoice {
                name: String::new(),
                model,
                ui_model: String::new(),
                texture: r.bike_skin.clone(),
                cc: r.cc,
                four_stroke: false,
            },
            outfit: r.rider_skin.clone(),
            plate: plate_number(i),
        })
    };
    let rides: Vec<_> = tape
        .riders
        .iter()
        .enumerate()
        .map(|(i, r)| ride(r, i))
        .collect();
    Some(frontend::Setup {
        event,
        track: entry.path,
        course: entry.course,
        laps: tape.setting(setting::LAPS).max(1),
        opponents: tape.riders.len().saturating_sub(1) as u32,
        mode: tape.setting(setting::MODE),
        name: entry.name,
        ride: rides.first().cloned().flatten(),
        level: 1,
        names: tape.riders.iter().map(|r| r.name.clone()).collect(),
        rides,
        ghost: None,
        net: None,
    })
}

/// Reads an RS2 archive, decoding FAOE if needed.
/// A track's terrain textures, each replaced by its high-res image when
/// `hires` has one. Textures the tiles use all share the largest size, so
/// if only some were replaced the rest are enlarged to match.
fn terrain_textures(terrain: &Terrain, track: &str, hires: Option<&HiRes>) -> Vec<hires::Image> {
    let mut images: Vec<_> = terrain
        .textures
        .iter()
        .enumerate()
        .map(|(i, tex)| hires::image(hires, &hires::terrain_key(track, i), tex))
        .collect();
    let original = terrain.textures.iter().map(|t| t.size).max().unwrap_or(1);
    let size = images.iter().map(|t| t.size).max().unwrap_or(1);
    for (image, tex) in images.iter_mut().zip(&terrain.textures) {
        if tex.size == original {
            image.grow_to(size);
        }
    }
    images
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_archive(path: &Path) -> Result<Vec<u8>, BoxError> {
    let raw = crate::platform::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    match Container::detect(&raw) {
        Container::Faoe => Ok(faoe::decode(&raw, name)?),
        Container::Rs2 => Ok(raw),
        Container::Unknown => Err(format!("{}: not an RS2 or FAOE archive", path.display()).into()),
    }
}

async fn gpu(
    compatible_surface: Option<&wgpu::Surface<'_>>,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), BoxError> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    gpu_from(&instance, compatible_surface).await
}

/// Sets the surface up as `config` says. The game draws to an sRGB format
/// (its shaders blend in linear light), which a canvas cannot be: there the
/// canvas is the plain format, drawn to through an sRGB view of it.
fn configure_surface(surface: &wgpu::Surface<'_>, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) {
    #[cfg(target_arch = "wasm32")]
    {
        let mut canvas = config.clone();
        canvas.format = config.format.remove_srgb_suffix();
        canvas.view_formats = vec![config.format];
        surface.configure(device, &canvas);
    }
    #[cfg(not(target_arch = "wasm32"))]
    surface.configure(device, config);
}

async fn gpu_from(
    instance: &wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'_>>,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), BoxError> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface,
            ..Default::default()
        })
        .await?;
    // Anti-aliasing with 2 or 8 samples, where the GPU has them.
    let specific = adapter.features() & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("mm2"),
            required_features: specific,
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await?;
    if !specific.is_empty() {
        let counts = |f| adapter.get_texture_format_features(f).flags.supported_sample_counts();
        let depth = counts(renderer::DEPTH_FORMAT);
        graphics::set_sample_counts(counts(graphics::HDR_FORMAT).into_iter().filter(|c| depth.contains(c)).collect());
    }
    Ok((adapter, device, queue))
}

/// Renders one frame offscreen and saves it as a PNG.
fn parse_camera(s: &str) -> Option<Camera> {
    let v: Vec<f32> = s
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, z, yaw, pitch] = v[..] else {
        return None;
    };
    Some(Camera {
        position: renderer::from_game(Vec3::new(x, y, z)),
        yaw: yaw.to_radians(),
        pitch: pitch.to_radians(),
        fov: camera::FOV,
    })
}

/// The podium for `event` on `track`: the scene's own spot, a supercross
/// track's start gate, or the terrain's middle (0x45e490).
fn podium_for(track: &Track, stand: &Stand, event: frontend::EventKind, roll: f32) -> podium::Podium {
    use frontend::EventKind::*;
    let gate = (event == Supercross && track.gate.is_some())
        .then_some((track.start.position, track.start.direction));
    // The middle 256-sample block, rounded down: with `n` blocks a side,
    // block n/2 of an odd count, (n-1)/2 of an even one.
    let n = (track.terrain.size - 1) as f32 / 256.0;
    let k = if n as i32 % 2 == 1 { n } else { n - 1.0 } * 0.5;
    let half = (k as i32) as f32 * 256.0 * track.terrain.scale;
    podium::Podium::new(
        stand.site,
        gate,
        Vec3::new(half, 0.0, half),
        |x, z| ride::ground(&track.terrain, x, z),
        matches!(event, Nationals | Supercross | Tag),
        roll,
    )
}

/// A number from 0 to 1 that differs each time.
fn rand_unit() -> f32 {
    let nanos = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    (nanos % 1000) as f32 / 1000.0
}

/// Points `camera` from a chase camera's eye at its target.
fn chase_at(camera: &mut Camera, (eye, target): (Vec3, Vec3)) {
    *camera = Camera::looking_at(renderer::from_game(eye), renderer::from_game(target));
}

#[allow(clippy::too_many_arguments)]
async fn screenshot(
    track: &Track,
    race: Option<&Race>,
    dirt: Option<(particles::Particles, Vec<(Vec<([Option<Vec3>; 2], dust::Kick)>, f32)>)>,
    replay: Option<&mut replay::Replay>,
    camera: &Camera,
    time: f32,
    out: &Path,
    width: u32,
    height: u32,
) -> Result<(), BoxError> {
    let (adapter, device, queue) = gpu(None).await?;
    eprintln!("rendering on {}", adapter.get_info().name);
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = track_renderer(track, &device, &queue, format);
    if let Some((d, tyres)) = dirt {
        renderer.particles.adopt(d);
        for (frame, dt) in &tyres {
            renderer.tyres(frame, *dt);
        }
    }
    if let Some(r) = race {
        let eye = renderer::from_game(camera.position);
        let view = cues::Eye {
            at: eye,
            target: eye + renderer::from_game(camera.forward()),
            fov: camera.fov,
            aspect: width as f32 / height.max(1) as f32,
        };
        show(r, track, &mut renderer, eye, Some(view));
    }
    // A replay is filmed as through a lens, in focus on the rider followed.
    renderer.cinematic = race
        .filter(|_| replay.is_some())
        .map(|r| renderer::from_game(r.racers[r.player].bike.position()).distance(camera.position));
    let target = offscreen(&device, format, width, height);
    let view = target.create_view(&Default::default());
    // MM2_BENCH=N: draws the frame N times more first, and says how long
    // a frame takes to record and to finish.
    if let Some(n) = std::env::var("MM2_BENCH").ok().and_then(|n| n.parse::<u32>().ok()) {
        let draw = renderer.render(&device, &queue, &view, (width, height), camera, time);
        queue.submit([draw]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let (mut record, start) = (0.0, web_time::Instant::now());
        for _ in 0..n {
            let t = web_time::Instant::now();
            let draw = renderer.render(&device, &queue, &view, (width, height), camera, time);
            record += t.elapsed().as_secs_f64();
            queue.submit([draw]);
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
        let n = n.max(1) as f64;
        eprintln!("bench: {:.2} ms a frame ({:.3} ms recording)", start.elapsed().as_secs_f64() * 1000.0 / n, record * 1000.0 / n);
    }
    let draw = renderer.render(&device, &queue, &view, (width, height), camera, time);
    // The in-race display over the race.
    let mut encoder = device.create_command_encoder(&Default::default());
    if let (Some(race), Some((mut ui, mut hud))) = (race, track.hud()) {
        let mut sprites = sprites::Sprites::new(&device, format);
        sprites.stretch = true;
        sprites.scale = (width as f32 / 640.0).min(height as f32 / 480.0);
        sprites.widescreen = graphics::showing().is_some() && replay.is_none();
        let mut gfx = ui::Gfx {
            sprites: &mut sprites,
            device: &device,
            queue: &queue,
        };
        match replay {
            Some(rp) => {
                // Twice: the slider's thumb finds its place once drawn.
                rp.draw(&mut ui, &mut gfx, 0.0);
                gfx.sprites.discard();
                rp.draw(&mut ui, &mut gfx, 0.0);
            }
            None => {
                let aspect = width as f32 / height.max(1) as f32;
                hud.view_proj =
                    Mat4::perspective_rh(camera.fovy(), aspect, 1.0, 20_000.0) * camera.view();
                hud.heads = race.racers.iter().map(|r| renderer.object_frame(r.object + 1, "Head", time)).collect();
                hud.look(&track.terrain, renderer::from_game(camera.position));
                hud.draw(&mut ui, &mut gfx, race, 1.0 / 60.0)
            }
        }
        sprites.render(&device, &mut encoder, &view, (width, height), false);
    }
    queue.submit([draw]);
    save_png(&device, &queue, encoder.finish(), &target, out)
}

/// A renderer for `track` with its sky, lights, detail, fog and dirt.
fn track_renderer(track: &Track, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> TerrainRenderer {
    let mut renderer = TerrainRenderer::new(
        device,
        queue,
        &track.terrain,
        &track.terrain_textures,
        &track.objects,
        format,
    );
    if let Some((cube, images)) = &track.sky {
        renderer.set_sky(device, queue, cube, images, format);
    }
    renderer.set_lights(track.lights);
    renderer.set_desert(track.desert);
    renderer.set_crowd(device, &track.spectators);
    if let Some(detail) = &track.detail {
        renderer.set_detail(device, queue, detail);
    }
    if let Some(fog) = track.fog {
        renderer.set_fog(fog);
    }
    if let Some((visibility, haziness)) = track.haze {
        renderer.set_visibility(profile::GraphicsOptions::default().terrain, visibility, haziness);
    }
    if let Some(sheet) = &track.particle_sheet {
        renderer.set_dirt(device, queue, sheet, format);
    }
    renderer
}

/// A scripted ride filmed (`--film DIR`): the models posed each frame as
/// live play poses them, so a thrown rider feels its motion, and from
/// `--film-from` s on the chase view saved 15 times a second as
/// DIR/NNNN.png (`--size`). `MM2_WRECK_LOG=FILE` also writes the player's
/// thrown rider each frame: t, stage, motion, the pelvis (x y z, then its
/// x, y and z axes).
struct Film {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: TerrainRenderer,
    dir: Option<PathBuf>,
    from: f32,
    size: (u32, u32),
    next: f32,
    shot: usize,
    log: Option<(PathBuf, String)>,
}

impl Film {
    fn new(track: &Track, dir: Option<PathBuf>, from: f32, size: (u32, u32)) -> Result<Self, BoxError> {
        let (_, device, queue) = pollster::block_on(gpu(None))?;
        let renderer = track_renderer(track, &device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb);
        let log = std::env::var("MM2_WRECK_LOG")
            .ok()
            .map(|f| (PathBuf::from(f), String::from("t,stage,motion,x,y,z,xx,xy,xz,yx,yy,yz,zx,zy,zz\n")));
        Ok(Film { device, queue, renderer, dir, from, size, next: from, shot: 0, log })
    }

    /// Poses the race at `clock` s into the ride, and films it.
    fn frame(&mut self, race: &mut Race, track: &Track, chase: (Vec3, Vec3), clock: f32) -> Result<(), BoxError> {
        let eye = renderer::from_game(chase.0);
        show(race, track, &mut self.renderer, eye, None);
        feel_wrecks(race, &self.renderer);
        if let (Some((_, text)), race::Bike::Player(b)) = (&mut self.log, &race.racers[race.player].bike) {
            if let Some(w) = &b.wreck {
                use std::fmt::Write as _;
                let m = w.body();
                let _ = writeln!(
                    text,
                    "{clock:.4},{:?},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
                    w.stage, w.motion, m.w_axis.x, m.w_axis.y, m.w_axis.z,
                    m.x_axis.x, m.x_axis.y, m.x_axis.z, m.y_axis.x, m.y_axis.y, m.y_axis.z, m.z_axis.x, m.z_axis.y, m.z_axis.z
                );
            }
        }
        let Some(dir) = &self.dir else { return Ok(()) };
        if clock + 1e-4 < self.next {
            return Ok(());
        }
        self.next += 1.0 / 15.0;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let (w, h) = self.size;
        let target = offscreen(&self.device, format, w, h);
        let view = target.create_view(&Default::default());
        let mut camera = Camera::looking_at(eye, renderer::from_game(chase.1));
        camera.fov = race.fov();
        let draw = self.renderer.render(&self.device, &self.queue, &view, (w, h), &camera, race.time);
        let encoder = self.device.create_command_encoder(&Default::default());
        self.queue.submit([draw]);
        let out = dir.join(format!("{:04}.png", self.shot));
        self.shot += 1;
        save_png(&self.device, &self.queue, encoder.finish(), &target, &out)
    }
}

impl Drop for Film {
    fn drop(&mut self) {
        if let Some((path, text)) = &self.log {
            let _ = crate::platform::fs::write(path, text);
        }
    }
}

/// The menus as they look after clicking at each `x,y` of `clicks`
/// (or typing `type:text`, pressing `key:z` or `esc`, or finishing the
/// race just started in a place, `race:1`)
/// (separated by `;`), a second apart.
async fn menu_screenshot(
    game: &Path,
    clicks: &str,
    out: &Path,
    results: Option<frontend::RaceResults>,
) -> Result<(), BoxError> {
    let (_, device, queue) = gpu(None).await?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut sprites = sprites::Sprites::new(&device, format);
    // MM2_SHOT_SCALE renders larger, as a window that size would.
    let scale: f32 = std::env::var("MM2_SHOT_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    sprites.scale = scale;
    let mut front =
        frontend::Frontend::new(game).ok_or("the game's UI/MAINMENU.DAT cannot be read")?;
    let audio = audio::Audio::new();
    if let Some(r) = results {
        front.race_over(r);
    }
    let mut now = 0.0;
    let mut started: Option<frontend::Setup> = None;
    for click in clicks.split(';').filter(|c| !c.trim().is_empty()) {
        // The race just started, over with the player in this place and
        // the computer riders behind or ahead in turn.
        if let Some(place) = click.strip_prefix("race:") {
            if let Some(setup) = started.take() {
                let riders = setup.opponents as usize + 1;
                let mine: usize = place.trim().parse().unwrap_or(1);
                let mut places = (1..=riders).filter(|&p| p != mine);
                let placings = (0..riders)
                    .map(|i| frontend::Placing {
                        place: if i == 0 {
                            mine
                        } else {
                            places.next().unwrap_or(riders)
                        },
                        ..Default::default()
                    })
                    .collect::<Vec<frontend::Placing>>();
                let mut lines: Vec<frontend::ResultLine> = placings
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let name = if i == 0 {
                            "You".to_string()
                        } else {
                            format!("Rider {i}")
                        };
                        let time = format!("1:{:02}.00", 20 + p.place);
                        [name, p.place.to_string(), time, String::new()]
                    })
                    .collect();
                lines.sort_by_key(|l| l[1].parse::<usize>().unwrap_or(0));
                front.race_over(frontend::RaceResults {
                    lines,
                    placings,
                    ..Default::default()
                });
            }
            continue;
        }
        // Typing, a key, or Esc, between the clicks.
        if let Some(text) = click.strip_prefix("type:") {
            text.chars().for_each(|c| front.type_char(c));
            front.type_char('\r');
            continue;
        }
        // Time passing, with the screen drawn (movies, animations).
        if let Some(t) = click
            .strip_prefix("wait:")
            .and_then(|t| t.trim().parse::<f32>().ok())
        {
            let mut gfx = ui::Gfx {
                sprites: &mut sprites,
                device: &device,
                queue: &queue,
            };
            // In steps, so what counts frame time (the clips' wait) sees it.
            let mut left = t;
            while left > 0.0 {
                let step = left.min(0.05);
                front.draw(&mut gfx, now, step);
                gfx.sprites.discard();
                now += step;
                left -= step;
            }
            continue;
        }
        if click.trim() == "intro" {
            front.intro();
            continue;
        }
        if click.trim() == "esc" {
            front.escape(now);
            continue;
        }
        if let Some(key) = click.strip_prefix("key:") {
            let code = match key {
                "z" => KeyCode::KeyZ,
                "space" => KeyCode::Space,
                "up" => KeyCode::ArrowUp,
                _ => KeyCode::KeyA,
            };
            front.key(&audio, code);
            continue;
        }
        // `move:x,y` rests the pointer there without clicking.
        let (hover, click) = match click.strip_prefix("move:") {
            Some(c) => (true, c),
            None => (false, click),
        };
        let (x, y) = click
            .split_once(',')
            .and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)))
            .ok_or("--clicks wants x,y;x,y")?;
        // Draw once so lists know where their lines are.
        let mut gfx = ui::Gfx {
            sprites: &mut sprites,
            device: &device,
            queue: &queue,
        };
        front.draw(&mut gfx, now, 0.0);
        sprites.discard();
        front.mouse_move(&audio, x, y);
        if hover {
            continue;
        }
        front.mouse_down(x, y);
        if let frontend::Next::Race(setup) = front.mouse_up(&audio, x, y, now) {
            eprintln!("race: {setup:?}");
            started = Some(setup);
        }
        now += 1.0;
    }
    let (w, h) = ((640.0 * scale) as u32, (480.0 * scale) as u32);
    let target = offscreen(&device, format, w, h);
    let mut gfx = ui::Gfx {
        sprites: &mut sprites,
        device: &device,
        queue: &queue,
    };
    front.draw(&mut gfx, now, 0.5);
    let mut encoder = device.create_command_encoder(&Default::default());
    let view = target.create_view(&Default::default());
    sprites.render(&device, &mut encoder, &view, (w, h), true);
    let mut commands = vec![encoder.finish()];
    // The Bike & Rider page's models, part way round.
    if let Some((ride, _)) = front.preview() {
        let res = read_archive(&game.join("RES").join("BIKE.RES"))?;
        let res = rs2::Archive::parse(&res)?;
        if let Some(mut p) = preview::Preview::new(&device, &queue, &res, &ride, format) {
            p.window = front.preview_window();
            p.spread = front.spread() == 2;
            commands.push(p.render(&device, &queue, &view, (w, h), 2.0, 1.0, true));
        }
    }
    let last = commands.pop().unwrap();
    queue.submit(commands);
    save_png(&device, &queue, last, &target, out)
}

fn offscreen(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("screenshot"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// A frame not drawn, in the log at most once a second.
fn skipped(why: &str) {
    static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if last.is_none_or(|t| t.elapsed().as_secs_f32() > 1.0) {
        eprintln!("frame skipped: {why}");
        *last = Some(Instant::now());
    }
}

/// Runs `draw` and saves `target` as a PNG.
fn save_png(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    draw: wgpu::CommandBuffer,
    target: &wgpu::Texture,
    out: &Path,
) -> Result<(), BoxError> {
    let (width, height) = (target.width(), target.height());
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([draw, encoder.finish()]);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
    device.poll(wgpu::PollType::wait_indefinitely())?;
    let data = readback.slice(..).get_mapped_range()?;
    let pixels: Vec<u8> = data
        .chunks(row as usize)
        .flat_map(|r| &r[..(width * 4) as usize])
        .copied()
        .collect();
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(crate::platform::fs::create(out)?),
        width,
        height,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header()?.write_image_data(&pixels)?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

/// The loading bar's steps so far: the game's bar is 20 steps long (0x451e30)
/// and steps one at each stage of the race's loading (0x49cb00, called from
/// 0x45d930, 0x418cd0 and 0x41a770), 11 of them in a race, so it shows a
/// little over half full when the race begins.
static LOAD_STEPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const LOAD_BAR_STEPS: u32 = 20;
/// In the browser the bar goes full over the steps there are (the 11 of the
/// game's loading and the 4 more here), where the game's stops a little
/// over half.
#[cfg(target_arch = "wasm32")]
const LOAD_TOTAL: u32 = 15;
#[cfg(not(target_arch = "wasm32"))]
const LOAD_TOTAL: u32 = LOAD_BAR_STEPS;

/// In the browser it also hands the page back for a moment, so the loading
/// bar is drawn and the page answers while the track loads.
async fn load_step() {
    LOAD_STEPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    crate::platform::thread::yield_now().await;
}

/// A step of its own in the browser, where the game's 11 are too far apart
/// for the bar to move smoothly; the game's bar has no such step.
async fn load_substep() {
    #[cfg(target_arch = "wasm32")]
    load_step().await;
}

/// A track loading on another thread while the loading screen shows.
struct Loading {
    setup: frontend::Setup,
    job: crate::platform::thread::Job<Result<Track, String>>,
}

struct App {
    track: Option<Track>,
    state: Option<State>,
    keys: HashSet<KeyCode>,
    /// The steering and lean keys as the game's key axes.
    steer_keys: input::KeyAxis,
    lean_keys: input::KeyAxis,
    /// The keyboard's keys for the 14 actions, from the profile.
    key_map: input::KeyMap,
    /// The profile's joystick map (its current device, or the first
    /// joystick when that is the keyboard), and its Analog Axes switch.
    pad_map: [(i32, i32); profile::ACTIONS],
    analog: bool,
    /// Each raced frame's keys, controls and the player's state
    /// (`MM2_RIDE_LOG=FILE`), to set a live ride beside a tape of the
    /// game's.
    ride_log: Option<std::io::BufWriter<crate::platform::fs::File>>,
    /// A chat line being typed in a network race (`/`, 0x4203e0).
    chat: Option<String>,
    /// Ctrl+S has turned the race's sounds off.
    sound_off: bool,
    /// Ctrl+V's arena reverb, heard only with EAX on the Sound page; it
    /// starts on in Supercross and Moto-Tag (0x4e81a0's +0x54).
    reverb: bool,
    /// F4 Visual Cues; a new profile starts with them on (+0x6a8), and the
    /// profile file does not keep them.
    visual_cues: bool,
    looking: bool,
    last: Instant,
    /// F2 (or `--show-fps`): the frame rate, in the window's
    /// title and the terminal, each second: since when, the frames and the
    /// slowest.
    fps: Option<(Instant, u32, f32)>,
    start: Instant,
    race: Option<Race>,
    /// The camera follows the bike rather than flying free.
    follow: bool,
    /// The window title last set, to change it only when the race status
    /// changes.
    title: String,
    /// The menus, when the game started from them.
    front: Option<frontend::Frontend>,
    loading: Option<Loading>,
    audio: audio::Audio,
    sounds: Option<sound::RaceSounds>,
    /// The in-race display, with the game's text and pictures.
    hud: Option<(ui::Ui, hud::Hud)>,
    /// The last frame's length, seconds.
    frame: f32,
    /// The pointer on the 640×480 screen.
    pointer: (f32, f32),
    /// The pad's A and B last frame, to click and go back in the menus.
    pad_click: (bool, bool),
    /// The race going on tape, and a tape playing (with the track it
    /// waits for while that loads).
    recording: Option<replay::Recording>,
    ghost: Option<replay::Ghost>,
    replay: Option<replay::Replay>,
    tape: Option<replay::Replay>,
    /// The race's event, for its podium.
    event: Option<frontend::EventKind>,
    /// The track file and course of the race on, for the track records.
    record_track: Option<(PathBuf, usize)>,
    /// A network race: whose bikes are whose, the other machines' bikes
    /// as they arrive, and the wait to send ours again.
    net: Option<frontend::NetRace>,
    remote: Vec<replay::Remote>,
    send_wait: f32,
    /// The profile's JoyDirectionFlipped setting: the stick's Y axis
    /// the other way round (0x4c6ec0).
    joy_flipped: bool,
    #[cfg(feature = "gamepad")]
    pads: Option<pad::Pads>,
    #[cfg(not(feature = "gamepad"))]
    pads: Option<()>,
}

/// The device whose map the race uses when the profile's own is the
/// keyboard: the joystick's kind, or the first joystick's map.
#[cfg(feature = "gamepad")]
fn pad_device(pads: &Option<pad::Pads>) -> usize {
    match pads {
        Some(p) if p.pad().is_some() => p.device(),
        _ => 1,
    }
}

#[cfg(not(feature = "gamepad"))]
fn pad_device(_pads: &Option<()>) -> usize {
    1
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// The track's renderer, while racing.
    renderer: Option<TerrainRenderer>,
    sprites: sprites::Sprites,
    camera: Camera,
    /// The Bike & Rider page's turning models, and `RES/BIKE.RES` for them.
    preview: Option<preview::Preview>,
    bike_res: Option<Vec<u8>>,
}

impl App {
    fn new(
        track: Option<Track>,
        race: Option<Race>,
        mut front: Option<frontend::Frontend>,
    ) -> Self {
        let audio = audio::Audio::new();
        if let Some(f) = &mut front {
            f.set_audio(&audio);
        }
        let sounds = track
            .as_ref()
            .and_then(|t| t.res.as_deref())
            .map(|res| {
                sound::RaceSounds::load(
                    res,
                    &audio,
                    &[sound::class(250, false)],
                    0,
                    frontend::EventKind::Nationals,
                    false,
                    sound::Options::default(),
                )
            });
        let hud = track.as_ref().and_then(Track::hud);
        App {
            track,
            state: None,
            keys: HashSet::new(),
            steer_keys: input::KeyAxis::default(),
            lean_keys: input::KeyAxis::default(),
            key_map: input::KeyMap::factory(),
            pad_map: input::factory_pad(),
            analog: true,
            ride_log: std::env::var("MM2_RIDE_LOG").ok().and_then(|p| crate::platform::fs::create(p).ok()).map(|f| {
                use std::io::Write as _;
                let mut w = std::io::BufWriter::new(f);
                let _ = writeln!(w, "t,dt,keys,x,y,z,roll,pitch,heading,vx,vy,vz,speed,gear,rpm,crashed,bars,lean,front_down,rear_down,front_squeeze,rear_squeeze");
                w
            }),
            chat: None,
            sound_off: false,
            reverb: false,
            visual_cues: true,
            looking: false,
            last: Instant::now(),
            fps: std::env::args().any(|a| a == "--show-fps").then(|| (Instant::now(), 0, 0.0)),
            start: Instant::now(),
            follow: race.is_some(),
            race,
            title: String::new(),
            front,
            loading: None,
            audio,
            hud,
            frame: 0.0,
            sounds,
            pointer: (0.0, 0.0),
            pad_click: (false, false),
            recording: None,
            ghost: None,
            replay: None,
            tape: None,
            event: None,
            record_track: None,
            net: None,
            remote: Vec::new(),
            send_wait: 0.0,
            joy_flipped: false,
            #[cfg(feature = "gamepad")]
            pads: pad::Pads::new(),
            #[cfg(not(feature = "gamepad"))]
            pads: None,
        }
    }

    /// The window, and the GPU instance and surface that draw to it.
    fn open_window(
        event_loop: &ActiveEventLoop,
    ) -> Result<(Arc<Window>, wgpu::Instance, wgpu::Surface<'static>), BoxError> {
        let attrs = Window::default_attributes()
            .with_title("Motocross Madness 2")
            .with_window_icon(window_icon());
        // The page's CSS sizes the canvas to the whole browser window.
        #[cfg(not(target_arch = "wasm32"))]
        let attrs = attrs.with_inner_size(window_size());
        // In the browser the window is the page's canvas.
        #[cfg(target_arch = "wasm32")]
        let attrs = {
            use wasm_bindgen::JsCast;
            use winit::platform::web::WindowAttributesExtWebSys;
            let canvas = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("mm2-canvas"))
                .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok())
                .ok_or("the page has no canvas with id mm2-canvas")?;
            attrs.with_canvas(Some(canvas)).with_prevent_default(true)
        };
        // `--fullscreen`: the whole screen, as Alt+Enter gives.
        let attrs = if std::env::args().any(|a| a == "--fullscreen") {
            attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)))
        } else {
            attrs
        };
        let window = Arc::new(event_loop.create_window(attrs)?);
        #[cfg(windows)]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(RawWindowHandle::Win32(h)) = window.window_handle().map(|h| h.as_raw()) {
                winwatch::start(h.hwnd.get());
            }
        }
        let display = Box::new(event_loop.owned_display_handle());
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(display),
        );
        let surface = instance.create_surface(window.clone())?;
        Ok((window, instance, surface))
    }

    /// The window's drawing state, once the GPU is chosen.
    fn make_state(
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Result<State, BoxError> {
        let size = window.inner_size();
        platform::set_screen(size.width.max(1), size.height.max(1));
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("this GPU cannot draw to the window")?;
        let caps = surface.get_capabilities(&adapter);
        config.format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(config.format);
        // A canvas offers no sRGB formats, but the game draws to one: see
        // `configure_surface`.
        #[cfg(target_arch = "wasm32")]
        {
            config.format = config.format.add_srgb_suffix();
        }
        // A canvas blends with the page behind it by the alpha the game
        // leaves in the frame; a window does not. The game means its frame to
        // be opaque.
        #[cfg(target_arch = "wasm32")]
        if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        }
        // `--no-vsync`: draw as fast as the machine can rather than at the
        // screen's refresh. Either way the bike steps once a frame, as the
        // game does.
        if std::env::args().any(|a| a == "--no-vsync") {
            config.present_mode = wgpu::PresentMode::AutoNoVsync;
        }
        configure_surface(&surface, &device, &config);
        let sprites = sprites::Sprites::new(&device, config.format);
        Ok(State {
            window,
            surface,
            device,
            queue,
            config,
            renderer: None,
            sprites,
            camera: Camera::looking_at(Vec3::ZERO, Vec3::Z),
            preview: None,
            bike_res: None,
        })
    }

    /// Draws the track being ridden, if any, in a new state.
    fn attach_track(&self, state: &mut State) {
        if let Some(track) = &self.track {
            Self::show_track(state, track, self.visibility());
        }
    }

    /// Opens the window and waits for the GPU (the desktop way).
    #[cfg(not(target_arch = "wasm32"))]
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<State, BoxError> {
        let (window, instance, surface) = Self::open_window(event_loop)?;
        let (adapter, device, queue) = pollster::block_on(gpu_from(&instance, Some(&surface)))?;
        let mut state = Self::make_state(window, surface, adapter, device, queue)?;
        self.attach_track(&mut state);
        Ok(state)
    }

    /// The Visibility detail, 0 to 9: the Advanced page's slider, else the
    /// overall graphic quality (global +0x1ee4).
    fn visibility(&self) -> u32 {
        let default = profile::GraphicsOptions::default().terrain;
        self.front.as_ref().map_or(default, |f| {
            let u = f.user();
            u.settings.number("SldVisibility", u.profile.graphics.terrain as i64).clamp(0, 9) as u32
        })
    }

    fn show_track(state: &mut State, track: &Track, quality: u32) {
        let mut renderer = TerrainRenderer::new(
            &state.device,
            &state.queue,
            &track.terrain,
            &track.terrain_textures,
            &track.objects,
            state.config.format,
        );
        if let Some((cube, images)) = &track.sky {
            renderer.set_sky(
                &state.device,
                &state.queue,
                cube,
                images,
                state.config.format,
            );
        }
        renderer.set_lights(track.lights);
        renderer.set_desert(track.desert);
        renderer.set_crowd(&state.device, &track.spectators);
        if let Some(detail) = &track.detail {
            renderer.set_detail(&state.device, &state.queue, detail);
        }
        if let Some(fog) = track.fog {
            renderer.set_fog(fog);
        }
        if let Some((visibility, haziness)) = track.haze {
            renderer.set_visibility(quality, visibility, haziness);
        }
        if let Some(sheet) = &track.particle_sheet {
            renderer.set_dirt(&state.device, &state.queue, &sheet, state.config.format);
        }
        state.renderer = Some(renderer);
        state.camera = renderer::start_camera(&track.terrain);
    }

    /// Starts loading the chosen race's track.
    fn load(&mut self, setup: frontend::Setup) {
        let (path, course, riders) = (
            setup.track.clone(),
            setup.course,
            setup.opponents as usize + 1,
        );
        // A ghost race's ghost is a second bike like the player's.
        let ghost = setup.mode == frontend::GHOST;
        let riders = if ghost { 2 } else { riders };
        let me = setup.net.map_or(PLAYER, |n| n.me);
        let mut rides = setup.rides.clone();
        rides.resize(riders.max(1), None);
        rides[PLAYER] = setup.ride.clone();
        if ghost {
            rides[1] = setup.ride.clone();
        }
        LOAD_STEPS.store(0, std::sync::atomic::Ordering::Relaxed);
        let job = crate::platform::thread::spawn_async(move || async move {
            load_track(&path, course, riders, rides, me, ghost.then_some(1)).await.map_err(|e| e.to_string())
        });
        self.loading = Some(Loading {
            setup,
            job,
        });
    }

    /// Once the track has loaded, the race begins.
    fn finish_loading(&mut self) {
        if !self.loading.as_ref().is_some_and(|l| l.job.is_finished()) {
            return;
        }
        let Loading { setup, job, .. } = self.loading.take().unwrap();
        match job.join() {
            Ok(Ok(mut track)) => {
                track.name = setup.name.clone();
                // Practice has no end.
                let practice = setup.mode == frontend::PRACTICE || setup.mode == frontend::GHOST;
                let stunt = setup.event == frontend::EventKind::Stunt;
                let tag = setup.event == frontend::EventKind::Tag;
                let laps = if practice || stunt || tag {
                    race::ENDLESS
                } else {
                    setup.laps
                };
                self.net = setup.net;
                self.event = Some(setup.event);
                self.record_track = Some((setup.track.clone(), setup.course));
                self.race = make_race_as(&track, laps, setup.net.map_or(PLAYER, |n| n.me));
                self.replay = self.tape.take();
                self.remote.clear();
                self.send_wait = 0.0;
                if let (Some(n), Some(r)) = (self.net, &mut self.race) {
                    self.remote = (0..r.racers.len())
                        .map(|_| replay::Remote::default())
                        .collect();
                    // The other machines' bikes wait at the start for them.
                    for (i, racer) in r.racers.iter_mut().enumerate() {
                        if !owned(n, i) {
                            racer.bike = replay::taped_like(&racer.bike);
                        }
                    }
                }
                let ghost = setup.mode == frontend::GHOST;
                self.recording = match (&self.race, &self.replay) {
                    (Some(r), None) if !ghost => Some(replay::Recording::new(heading(&setup, r))),
                    _ => None,
                };
                self.ghost = None;
                if let (Some(r), true) = (&mut self.race, ghost) {
                    r.ghost = Some(1);
                    let mut h = heading(&setup, r);
                    h.riders.truncate(1);
                    let saved = setup.ghost.as_ref().and_then(|p| {
                        let bytes = crate::platform::fs::read(p).ok()?;
                        mm2_formats::vcr::Tape::parse(&bytes).ok()
                    });
                    self.ghost = Some(replay::Ghost::new(1, replay::header(h), saved, 0.0));
                }
                if let Some(r) = &mut self.race {
                    r.practice = practice;
                    // Practice and ghost races have no countdown (race
                    // +0x15c stays 0, 0x421f90).
                    if practice {
                        r.lead = 0.0;
                    }
                    r.stunt_event = stunt;
                    r.minutes = ((stunt || tag) && !practice).then_some(setup.laps as f32);
                    if tag {
                        let n = setup.net;
                        let mut t = tag::Tag::new(
                            r.racers.len(),
                            n.is_some_and(|n| n.tag_ball),
                            n.is_some_and(|n| n.tag_stunts),
                        );
                        if let Some(n) = n {
                            t.judge = n.host;
                            t.local = (0..r.racers.len()).map(|i| owned(n, i)).collect();
                        }
                        r.tag = Some(t);
                    }
                    r.level = setup.level;
                    r.names = setup.names.clone();
                    // The profile's keys and gyro options.
                    if let Some(f) = &self.front {
                        let p = &f.user().profile;
                        r.wreck_reset = p.game.wreck_reset || setup.net.is_some();
                        r.player_indicator = p.game.player_indicator;
                        r.visual_cues = self.visual_cues;
                        let map = input::KeyMap::from_profile(&p.keys);
                        self.key_map = if map.0.iter().any(Option::is_some) {
                            map
                        } else {
                            input::KeyMap::factory()
                        };
                        // The joystick's map: the one the controls page
                        // shows if a joystick's, else the pad plugged in.
                        let joy = |d: usize| p.keys.keys.get(d).filter(|m| m.iter().any(|&(k, _)| k == 2));
                        let shown = p.controls.device as usize;
                        self.joy_flipped = f.user().settings.number("JoyDirectionFlipped", 0) != 0;
                        self.pad_map = match (shown < input::KEYBOARD).then(|| joy(shown)).flatten() {
                            Some(m) => *m,
                            None => joy(pad_device(&self.pads)).copied().unwrap_or_else(input::factory_pad),
                        };
                        self.analog = p.controls.analog_axes;
                        if let race::Bike::Player(b) = &mut r.racers[r.player].bike {
                            b.gas_gyro = p.controls.gas_gyro;
                            b.brake_gyro = p.controls.brake_gyro;
                            // The sliders (0x496a50).
                            let c = &p.controls;
                            b.sliders = mm2_physics::bike::Sliders::from_values(
                                c.steering,
                                c.cross_up,
                                c.pitch,
                                c.press_brakes,
                                c.release_brakes,
                            );
                            // The garage's curve, once a bike has put one
                            // there; else the engine's own (0x4d7090).
                            let g = &p.garage;
                            if g.ride_rpm[1] > 0 && g.ride_rpm[1] > g.ride_rpm[0] {
                                let curve = Spec {
                                    rpm_low: g.ride_rpm[0] as f32,
                                    rpm_high: g.ride_rpm[1] as f32,
                                    power: g.ride_power.map(|v| v as f32),
                                    ..b.spec.clone()
                                };
                                b.drivetrain = mm2_physics::drivetrain::Drivetrain::new(&curve, 1.0, 1.0);
                            }
                            b.drivetrain.open_rate = mm2_physics::bike::slider(c.press_throttle);
                            b.drivetrain.close_rate = mm2_physics::bike::slider(c.release_throttle);
                        }
                    }
                    // The shipped Pro Circuit tables' stunt money for the event.
                    r.purse = (setup.event != frontend::EventKind::Stunt).then(|| {
                        mm2_physics::career::Tables::shipped().stunt_purse(setup.event as usize)
                    });
                }
                // Each racer's engine class, from their bike (the default
                // grid rides 250s).
                let player = self.race.as_ref().map_or(PLAYER, |r| r.player);
                let classes: Vec<usize> = (0..self.race.as_ref().map_or(1, |r| r.racers.len()))
                    .map(|i| {
                        let ride = if i == player { setup.ride.as_ref() } else { setup.rides.get(i).and_then(Option::as_ref) };
                        ride.map_or(sound::class(250, false), |r| sound::class(r.bike.cc, r.bike.four_stroke))
                    })
                    .collect();
                let options = self
                    .front
                    .as_ref()
                    .map_or_else(Default::default, |f| sound::Options::from_profile(&f.user().profile.sound));
                self.sound_off = false;
                self.reverb = matches!(setup.event, frontend::EventKind::Supercross | frontend::EventKind::Tag);
                self.apply_reverb();
                self.sounds = track.res.as_deref().map(|res| {
                    let mut s = sound::RaceSounds::load(res, &self.audio, &classes, player, setup.event, track.crowd, options);
                    s.set_scene(track.sounds.iter().cloned());
                    s
                });
                self.follow = self.race.is_some();
                let quality = self.visibility();
                if let Some(state) = &mut self.state {
                    Self::show_track(state, &track, quality);
                }
                self.hud = track.hud();
                if let (Some((_, hud)), Some(f)) = (&mut self.hud, &self.front) {
                    hud.show = hud::Show::from_profile(&f.user().profile);
                }
                self.track = Some(track);
                self.start = Instant::now();
            }
            Ok(Err(e)) => {
                eprintln!("{}: {e}", setup.track.display());
                self.back_to_menu();
            }
            Err(_) => self.back_to_menu(),
        }
    }

    /// Whether a race played from the menus is over by itself (0x45feb0,
    /// run each frame): a lap or gate race the frame after the player
    /// crosses the line for the last time, once 0x497740 has said the
    /// place; a Stunt Quarry or tag session when its minutes are up.
    /// Practice and ghost races never end, and a network race is left to
    /// Esc here.
    fn race_done(&self) -> bool {
        let Some(r) = &self.race else { return false };
        if self.front.is_none() || self.replay.is_some() || self.net.is_some() || r.practice || r.podium.is_some() {
            return false;
        }
        match r.time_left() {
            Some(left) => left <= 0.0,
            None => r.racers[r.player].finished.is_some(),
        }
    }

    /// Ends the race: with the podium first when the player made the top
    /// three of a finished race and the Podiums option is on (0x45f610,
    /// 0x45e490); Esc again leaves it.
    fn end_race(&mut self) {
        let (Some(r), Some(track), Some(f)) = (&mut self.race, &self.track, &self.front) else {
            return self.back_to_menu();
        };
        let me = &r.racers[r.player];
        let over = me.finished.is_some() || r.time_left().is_some_and(|t| t <= 0.0);
        let top = self.net.is_some() || r.position() <= 3;
        let wanted = f.user().profile.game.podiums && self.replay.is_none();
        let event = self.event.unwrap_or(frontend::EventKind::Nationals);
        match &track.podium {
            Some(stand) if r.podium.is_none() && wanted && over && top && !r.practice => {
                r.podium = Some((podium_for(track, stand, event, rand_unit()), r.time));
                if let Some(s) = &mut self.sounds {
                    s.podium(&self.audio);
                }
                self.follow = true;
            }
            _ => self.back_to_menu(),
        }
    }

    /// Enters the player's result in the track's records, as the game does
    /// when a race ends (0x49d180, 0x49d220, 0x524a10): the best lap (or
    /// stunt, or longest time as IT) always, and the race's total (or the
    /// session's score or time as IT) as well when it was a 5 or 10 lap
    /// (or minute) race. Practice, ghost, network and replayed races don't
    /// count, nor does a race that wasn't run to its end.
    fn save_track_records(&self) {
        let (Some(r), Some(track), Some(f), Some(event), Some((path, course))) =
            (&self.race, &self.track, &self.front, self.event, &self.record_track)
        else {
            return;
        };
        if self.replay.is_some() || self.net.is_some() || r.practice || r.ghost.is_some() {
            return;
        }
        let me = &r.racers[r.player];
        let timed = r.time_left().is_some();
        if me.finished.is_none() && !r.time_left().is_some_and(|t| t <= 0.0) {
            return;
        }
        let number = event as u32;
        // Per table: the session's length that picks it (None for the first,
        // which every race fills), and the value it keeps.
        let length = if timed { r.minutes.unwrap_or(0.0) } else { r.laps as f32 };
        let values: [Option<f32>; 3] = match event {
            frontend::EventKind::Stunt => [
                Some(me.best_stunt),
                (length == 5.0).then_some(me.stunt_points),
                (length == 10.0).then_some(me.stunt_points),
            ],
            frontend::EventKind::Tag => {
                let it = r.tag.as_ref().map_or((0.0, 0.0), |t| (t.longest_it(r.player), t.time_it(r.player)));
                [Some(it.0), (length == 5.0).then_some(it.1), (length == 10.0).then_some(it.1)]
            }
            _ => {
                let total = me.timing.finished.then_some(me.timing.race_time);
                [
                    Some(me.timing.best_lap),
                    if length == 5.0 { total } else { None },
                    if length == 10.0 { total } else { None },
                ]
            }
        };
        let user = &f.user().store.dir;
        let name = f.user().profile.name.clone();
        for (table, value) in values.into_iter().enumerate() {
            // A zero is no result (0x524a10).
            let Some(value) = value.filter(|v| *v > 0.0) else {
                continue;
            };
            let record = screens::records::Record { name: name.clone(), value };
            if let Err(e) = screens::records::add(user, number, path, *course, &track.name, table, record) {
                eprintln!("could not save the track records: {e}");
            }
        }
    }

    /// Leaves the race: for the Race Info page with its results, or the
    /// main menu when the track did not load.
    fn back_to_menu(&mut self) {
        self.save_track_records();
        self.net = None;
        self.remote.clear();
        let mut results = match (&self.race, &self.hud) {
            (Some(race), Some((ui, _))) => Some(results(race, ui)),
            _ => None,
        };
        // The race's tape is kept for the View Replay button.
        if let (Some(rec), Some(r), Some(f)) = (self.recording.take(), &mut results, &self.front) {
            let path = replay::tape_path(&f.user().store.dir);
            let written = path
                .parent()
                .map_or(Ok(()), crate::platform::fs::create_dir_all)
                .and_then(|_| crate::platform::fs::write(&path, rec.finish().write()));
            match written {
                Ok(()) => r.replay = Some(path),
                Err(e) => eprintln!("{}: {e}", path.display()),
            }
        }
        // A ghost race offers to keep a lap that beat the ghost it started
        // with, once the results are up.
        let new_ghost = self
            .ghost
            .take()
            .filter(|g| g.improved)
            .and_then(|g| g.best().cloned());
        let replayed = self.replay.take().is_some() || self.tape.take().is_some();
        self.race = None;
        self.track = None;
        self.follow = false;
        if let Some(s) = &mut self.sounds {
            s.stop(&self.audio);
        }
        self.sounds = None;
        self.hud = None;
        self.audio.stop_all();
        self.audio.set_reverb(None);
        if let Some(state) = &mut self.state {
            state.renderer = None;
            state.window.set_title("Motocross Madness 2");
        }
        self.title.clear();
        if let Some(f) = &mut self.front {
            match results {
                _ if replayed => f.replay_over(),
                Some(r) => f.race_over(r),
                None => f.main_menu(),
            }
            if let Some(lap) = new_ghost {
                f.ask_ghost(lap);
            }
        }
    }

    /// Plays a tape: its track loads with its riders, then the tape runs.
    fn start_replay(&mut self, path: &Path) {
        let Some(tape) = replay::Replay::open(path) else {
            if let Some(f) = &mut self.front {
                f.replay_over();
            }
            return;
        };
        let game = self
            .front
            .as_ref()
            .and_then(|f| f.game())
            .map(Path::to_path_buf);
        let Some(setup) = game.and_then(|g| replay_setup(&g, &tape.tape)) else {
            eprintln!("{}: the tape's track is not here", path.display());
            if let Some(f) = &mut self.front {
                f.replay_over();
            }
            return;
        };
        if let Some(f) = &mut self.front {
            f.loading(setup.event);
        }
        self.tape = Some(tape);
        self.load(setup);
    }

    /// The menus are showing (not a race, nor a track loading).
    fn in_menus(&self) -> bool {
        self.front.is_some() && self.track.is_none() && self.loading.is_none()
    }

    fn fly(&mut self, dt: f32) {
        let Some(state) = &mut self.state else { return };
        let key = |k| self.keys.contains(&k);
        let cam = &mut state.camera;
        let mut dir = Vec3::ZERO;
        if key(KeyCode::KeyW) {
            dir += cam.forward()
        }
        if key(KeyCode::KeyS) {
            dir -= cam.forward()
        }
        if key(KeyCode::KeyD) {
            dir += cam.right()
        }
        if key(KeyCode::KeyA) {
            dir -= cam.right()
        }
        if key(KeyCode::KeyE) || key(KeyCode::Space) {
            dir += Vec3::Y
        }
        if key(KeyCode::KeyQ) || key(KeyCode::ControlLeft) {
            dir -= Vec3::Y
        }
        let speed = if key(KeyCode::ShiftLeft) || key(KeyCode::ShiftRight) {
            900.0
        } else {
            200.0
        };
        cam.position += dir.normalize_or_zero() * speed * dt;
        let turn = 1.5 * dt;
        if key(KeyCode::ArrowLeft) {
            cam.look(-turn, 0.0)
        }
        if key(KeyCode::ArrowRight) {
            cam.look(turn, 0.0)
        }
        if key(KeyCode::ArrowUp) {
            cam.look(0.0, -turn)
        }
        if key(KeyCode::ArrowDown) {
            cam.look(0.0, turn)
        }
    }

    /// What the keyboard and the first gamepad ask of the bike.
    /// Ctrl+S: the race's sounds off, or on again (0x4e9250).
    fn flip_sound(&mut self) {
        self.sound_off = !self.sound_off;
        let gain = self
            .front
            .as_ref()
            .map_or_else(sound::Options::default, |f| sound::Options::from_profile(&f.user().profile.sound))
            .gain;
        if let Some(s) = &mut self.sounds {
            s.set_on(&self.audio, !self.sound_off, gain);
        }
        if let Some((ui, hud)) = &mut self.hud {
            hud.say_switch(ui, 5161, !self.sound_off);
        }
    }

    /// The listener's EAX environment: the arena while the reverb is on,
    /// and only with EAX ticked (the game needs an EAX card for any).
    fn apply_reverb(&self) {
        let eax = self.front.as_ref().is_some_and(|f| f.user().profile.sound.eax);
        self.audio.set_reverb((eax && self.reverb).then_some(reverb::ARENA));
    }

    /// Ctrl+V: the reverb on or off, said (0x4e9250, string 0x14c3).
    fn flip_reverb(&mut self) {
        let eax = self.front.as_ref().is_some_and(|f| f.user().profile.sound.eax);
        if !eax || self.sounds.is_none() {
            return;
        }
        self.reverb = !self.reverb;
        self.apply_reverb();
        if let Some((ui, hud)) = &mut self.hud {
            hud.say_switch(ui, 0x14c3, self.reverb);
        }
    }

    /// F4: the Visual Cues on or off (0x4e54c0, string 0x13b6).
    fn flip_cues(&mut self) {
        self.visual_cues = !self.visual_cues;
        if let Some(r) = &mut self.race {
            r.visual_cues = self.visual_cues;
        }
        if let Some((ui, hud)) = &mut self.hud {
            hud.say_switch(ui, 0x13b6, self.visual_cues);
        }
    }

    /// F11 or F12: the overall graphic quality down or up a step, 0 to 9,
    /// which sets all four details, redraws the fog and the draw distance
    /// and says "Graphic Quality : n" (0x4e54c0).
    fn step_quality(&mut self, by: i64) {
        let Some(f) = &mut self.front else { return };
        let user = f.user_mut();
        let q = (user.profile.graphics.terrain as i64 + by).clamp(0, 9) as u32;
        user.profile.graphics.terrain = q;
        for key in screens::options::DETAILS {
            user.settings.set(key, &q.to_string());
        }
        user.save_settings();
        if let Err(e) = user.store.save(&user.profile) {
            eprintln!("could not save the profile: {e}");
        }
        self.refog(q);
        if let Some((ui, hud)) = &mut self.hud {
            hud.say(&format!("{} : {}", ui.lang(0x14c4), q + 1));
        }
    }

    /// Redraws the fog and the draw distance of the race at quality `q`.
    fn refog(&mut self, q: u32) {
        if let (Some(state), Some(track)) = (&mut self.state, &self.track) {
            if let (Some(renderer), Some((visibility, haziness))) = (&mut state.renderer, track.haze) {
                renderer.set_visibility(q, visibility, haziness);
            }
        }
    }

    /// The start page's Graphics choices (`platform::page_graphics`): once
    /// the player has a profile they go into it, its settings and the race
    /// on screen, and the profile's own are noted for the page to show.
    fn sync_page_graphics(&mut self) {
        use platform::page_graphics as page;
        let Some(f) = &mut self.front else { return };
        let named = !f.user().profile.name.is_empty();
        if let Some(c) = page::take(named) {
            let user = f.user_mut();
            let g = &mut user.profile.graphics;
            (g.shadows, g.particles, g.sky, g.terrain) = (c.shadows, c.particles, c.sky, c.quality);
            for key in screens::options::DETAILS {
                user.settings.set(key, &c.quality.to_string());
            }
            user.save_settings();
            if let Err(e) = user.store.save(&user.profile) {
                eprintln!("could not save the profile: {e}");
            }
            if let Some((_, hud)) = &mut self.hud {
                (hud.show.shadows, hud.show.particles, hud.show.sky) = (c.shadows, c.particles, c.sky);
            }
            self.refog(c.quality);
        }
        let Some(f) = &self.front else { return };
        let g = &f.user().profile.graphics;
        page::publish(
            (!f.user().profile.name.is_empty()).then_some(page::Choice {
                shadows: g.shadows,
                particles: g.particles,
                sky: g.sky,
                quality: g.terrain,
            }),
        );
    }

    fn ctrl(&self) -> bool {
        self.keys.contains(&KeyCode::ControlLeft) || self.keys.contains(&KeyCode::ControlRight)
    }

    /// Ctrl+G or Ctrl+B: the gas or brake gyro on or off, said and kept.
    fn flip_gyro(&mut self, gas: bool) {
        let Some(r) = &mut self.race else { return };
        let me = r.player;
        let race::Bike::Player(b) = &mut r.racers[me].bike else {
            return;
        };
        let on = if gas {
            b.gas_gyro = !b.gas_gyro;
            b.gas_gyro
        } else {
            b.brake_gyro = !b.brake_gyro;
            b.brake_gyro
        };
        if let Some((ui, hud)) = &mut self.hud {
            hud.say_switch(ui, if gas { 5133 } else { 5165 }, on);
        }
        if let Some(f) = &mut self.front {
            let user = f.user_mut();
            if gas {
                user.profile.controls.gas_gyro = on;
            } else {
                user.profile.controls.brake_gyro = on;
            }
            if let Err(e) = user.store.save(&user.profile) {
                eprintln!("could not save the profile: {e}");
            }
        }
    }

    /// `/` opens the chat line unless the chat box is always off; Shift+/
    /// steps its mode: normal, always on, always off ("ChatBox : Normal").
    fn chat_key(&mut self, shift: bool) {
        let Some(f) = &mut self.front else { return };
        let user = f.user_mut();
        if !shift {
            if user.profile.game.chat_mode != 2 {
                self.chat = Some(String::new());
                self.keys.clear();
            }
            return;
        }
        let mode = (user.profile.game.chat_mode + 1) % 3;
        user.profile.game.chat_mode = mode;
        if let Err(e) = user.store.save(&user.profile) {
            eprintln!("could not save the profile: {e}");
        }
        if let Some((ui, hud)) = &mut self.hud {
            let what = [5131, 5130, 5132][mode as usize];
            hud.say(&format!("{} : {}", ui.lang(5160), ui.lang(what)));
        }
    }

    /// A key while the chat line is open: Enter sends it, Escape drops it.
    fn type_chat(&mut self, event: &winit::event::KeyEvent) {
        let Some(line) = &mut self.chat else { return };
        match event.physical_key {
            PhysicalKey::Code(KeyCode::Enter | KeyCode::NumpadEnter) => {
                let line = self.chat.take().unwrap_or_default();
                if !line.trim().is_empty() {
                    self.send_chat(&line);
                }
                return;
            }
            PhysicalKey::Code(KeyCode::Escape) => {
                self.chat = None;
                return;
            }
            PhysicalKey::Code(KeyCode::Backspace) => {
                line.pop();
            }
            _ => {
                if let Some(text) = &event.text {
                    for c in text.chars().filter(|c| !c.is_control()) {
                        if line.chars().count() < 73 {
                            line.push(c);
                        }
                    }
                }
            }
        }
        let shown = format!("> {line}_");
        if let Some((_, hud)) = &mut self.hud {
            hud.say(&shown);
        }
    }

    /// Sends a chat line as "Name: line" and shows it here too.
    fn send_chat(&mut self, line: &str) {
        let name = self.front.as_ref().map_or_else(String::new, |f| f.user().profile.name.clone());
        let text = format!("{name}: {line}");
        net::with(|s| s.say(net::Msg::Chat(text.clone())));
        if let Some((_, hud)) = &mut self.hud {
            hud.say(&text);
        }
    }

    /// F5 to F10 flip the race panel, radar, speedometer, sky, dirt and
    /// shadows, and say so.
    fn flip_switch(&mut self, key: KeyCode) {
        let Some((ui, hud)) = &mut self.hud else {
            return;
        };
        let s = &mut hud.show;
        let (flag, what) = match key {
            KeyCode::F5 => (&mut s.stats, 5141),
            KeyCode::F6 => (&mut s.radar, 5185),
            KeyCode::F7 => (&mut s.gauges, 5142),
            KeyCode::F8 => (&mut s.sky, 5137),
            KeyCode::F9 => (&mut s.particles, 5135),
            _ => (&mut s.shadows, 5136),
        };
        *flag = !*flag;
        let on = *flag;
        hud.say_switch(ui, what, on);
        self.keep_switches();
    }

    /// The profile keeps the race's switches.
    fn keep_switches(&mut self) {
        let (Some((_, hud)), Some(f)) = (&self.hud, &mut self.front) else {
            return;
        };
        let user = f.user_mut();
        hud.show.store(&mut user.profile);
        if let Err(e) = user.store.save(&user.profile) {
            eprintln!("could not save the profile: {e}");
        }
    }

    fn controls(&mut self, dt: f32) -> Controls {
        let acts = self.actions_held();
        let held = |a: usize| acts[a];
        let keyboard = Controls {
            throttle: held(input::ACCELERATE) as i32 as f32,
            brake: held(input::BRAKE) as i32 as f32,
            steer: self
                .steer_keys
                .update(held(input::STEER_LEFT), held(input::STEER_RIGHT), dt),
            lean: self
                .lean_keys
                .update(held(input::LEAN_BACK), held(input::LEAN_FORWARD), dt),
        };
        self.gamepad(keyboard)
    }

    #[cfg(not(feature = "gamepad"))]
    fn gamepad(&mut self, c: Controls) -> Controls {
        c
    }

    /// The trick buttons (the game's buttons 6 and 7): the profile's keys
    /// (Z and X from the factory), or the first gamepad's A and B (our
    /// picks until the controls page sets them).
    fn trick_buttons(&self) -> [bool; 2] {
        let b = [
            self.key_map.held(input::STUNTS_LOW, &self.keys) || self.pad_held(input::STUNTS_LOW),
            self.key_map.held(input::STUNTS_HIGH, &self.keys) || self.pad_held(input::STUNTS_HIGH),
        ];
        b
    }

    /// Each action held this frame, on the keys or the gamepad.
    fn actions_held(&self) -> [bool; profile::ACTIONS] {
        std::array::from_fn(|a| self.key_map.held(a, &self.keys) || self.pad_held(a))
    }

    /// The joystick in use, as the profile maps it (0x449e20 binds the
    /// map, 0x52e3e0 reads it): an action on a button is held with it;
    /// the steering axis has a 0.04 dead centre and is cubed, fine near
    /// the middle and full lock at the edge; the lean axis has no dead
    /// centre. Accelerate on the Z axis (a flight stick's throttle
    /// lever) opens the gas past a third of its travel, the way the
    /// game's analog throttle does (0.2 floor, 0.33 to count). Analog
    /// Axes off, a stick works the bars and lean like the keys. A game
    /// pad's triggers also work the gas and brake, past a third.
    #[cfg(feature = "gamepad")]
    fn gamepad(&mut self, mut c: Controls) -> Controls {
        let map = self.pad_map;
        let flip = if self.joy_flipped { -1.0 } else { 1.0 };
        let Some(pad) = self.pads.as_ref() else {
            return c;
        };
        let trigger = |b| pad.trigger(b);
        if trigger(gilrs::Button::RightTrigger2) > pad::THROTTLE_ON {
            c.throttle = 1.0;
        }
        if trigger(gilrs::Button::LeftTrigger2) > pad::THROTTLE_ON {
            c.brake = 1.0;
        }
        if let (2, code) = map[input::ACCELERATE] {
            if let Some(v) = pad.throttle_axis(code) {
                if v > pad::THROTTLE_ON {
                    c.throttle = 1.0;
                }
            }
        }
        // An action's pull, 0 to 1; `side` is the axis's half it reads.
        let pull = |action: usize, side: f32, dead: f32| -> f32 {
            match map[action] {
                (2, code) => match pad.axis(code) {
                    Some(v) if code < 0 => {
                        let v = if code == -5 { v * flip } else { v };
                        let v = if v.abs() < dead { 0.0 } else { v };
                        (v * side).max(0.0)
                    }
                    _ => 0.0,
                },
                _ => 0.0,
            }
        };
        let steer = pull(input::STEER_RIGHT, 1.0, pad::STEER_DEAD)
            - pull(input::STEER_LEFT, -1.0, pad::STEER_DEAD);
        let lean = pull(input::LEAN_FORWARD, -1.0, 0.0) - pull(input::LEAN_BACK, 1.0, 0.0);
        if self.analog {
            if steer != 0.0 {
                c.steer = input::stick_steer(steer);
            }
            if lean != 0.0 {
                c.lean = lean;
            }
        }
        c
    }

    /// Whether the joystick holds `action`'s button, or, with Analog
    /// Axes off, pushes its axis's side over half way, as a key (the
    /// game's game pad reading, 0x4c74e0).
    #[cfg(feature = "gamepad")]
    fn pad_held(&self, action: usize) -> bool {
        let Some(pad) = self.pads.as_ref() else {
            return false;
        };
        match self.pad_map[action] {
            (2, code) if code >= 0 => pad.button(code),
            (2, code) if !self.analog => pad.axis(code).is_some_and(|v| {
                let v = if code == -5 && self.joy_flipped { -v } else { v };
                let side = match action {
                    input::STEER_LEFT | input::LEAN_FORWARD => -1.0,
                    _ => 1.0,
                };
                v * side > pad::PUSHED
            }),
            _ => false,
        }
    }

    /// Takes the joysticks' events each frame. In the menus the pad works
    /// the pointer (the game's menus took only the mouse and keys): the
    /// left stick or D-pad moves it, A clicks and B goes back. On the
    /// controls page, while an action waits, a button or stick pressed
    /// goes to the page to bind instead.
    #[cfg(feature = "gamepad")]
    fn poll_pads(&mut self, event_loop: &ActiveEventLoop, dt: f32) {
        let Some(pads) = &mut self.pads else {
            return;
        };
        let presses = pads.poll();
        let (stick, a, b) = pads.pointer();
        let (was_a, was_b) = std::mem::replace(&mut self.pad_click, (a, b));
        if !self.in_menus() {
            return;
        }
        let now = self.start.elapsed().as_secs_f32();
        let Some(f) = &mut self.front else { return };
        if f.wants_pad() {
            for press in presses {
                let input = match press {
                    pad::Press::Button(n) => (2, n),
                    pad::Press::Axis(code) => (2, code),
                };
                let Some(f) = &mut self.front else { return };
                let next = f.pad_input(&self.audio, input);
                if !matches!(next, frontend::Next::Stay) {
                    self.next(next, event_loop);
                    return;
                }
            }
            return;
        }
        // Past a fifth of the stick, up to 500 of the 640×480 screen a second.
        let speed = |v: f32| if v.abs() < 0.2 { 0.0 } else { v * 500.0 * dt };
        let (dx, dy) = (speed(stick[0]), speed(stick[1]));
        if dx != 0.0 || dy != 0.0 {
            let (x, y) = self.pointer;
            let (x, y) = ((x + dx).clamp(0.0, 639.0), (y + dy).clamp(0.0, 479.0));
            self.pointer = (x, y);
            f.mouse_move(&self.audio, x, y);
            if let Some(s) = &self.state {
                let (w, h) = (s.config.width as f32, s.config.height as f32);
                let scale = (w / sprites::SCREEN.0).min(h / sprites::SCREEN.1);
                let (ox, oy) = ((w - sprites::SCREEN.0 * scale) / 2.0, (h - sprites::SCREEN.1 * scale) / 2.0);
                let at = winit::dpi::PhysicalPosition::new((x * scale + ox) as f64, (y * scale + oy) as f64);
                let _ = s.window.set_cursor_position(at);
            }
        }
        let (x, y) = self.pointer;
        if a && !was_a {
            f.mouse_down(x, y);
        } else if !a && was_a {
            let next = f.mouse_up(&self.audio, x, y, now);
            self.next(next, event_loop);
            return;
        }
        if b && !was_b {
            let next = f.escape(now);
            self.next(next, event_loop);
        }
    }

    #[cfg(not(feature = "gamepad"))]
    fn poll_pads(&mut self, _event_loop: &ActiveEventLoop, _dt: f32) {}
    #[cfg(not(feature = "gamepad"))]
    fn pad_held(&self, _action: usize) -> bool {
        false
    }

    /// Rides the bike, or flies the camera when not following it.
    fn update(&mut self, dt: f32) {
        self.finish_loading();
        if self.track.is_none() {
            return;
        }
        let podium_over = |r: &Race| r.podium.is_some_and(|(_, at)| r.time + dt - at >= podium::LENGTH);
        if self.race.as_ref().is_some_and(podium_over) {
            self.back_to_menu();
            return;
        }
        if self.race_done() {
            self.end_race();
            return;
        }
        let controls = if self.follow {
            self.controls(dt)
        } else {
            Controls::default()
        };
        let buttons = if self.follow {
            self.trick_buttons()
        } else {
            [false; 2]
        };
        if self.replay.is_none() && self.race.is_some() {
            if let Some(track) = &mut self.track {
                track.run_traffic(dt);
            }
        }
        let acts = self.actions_held();
        let Some(track) = &self.track else { return };
        let Some(state) = &mut self.state else { return };
        if let (Some(rp), Some(r)) = (&mut self.replay, &mut self.race) {
            rp.advance(dt);
            let (eye, target) = rp.show(r, &track.terrain);
            if let Some(sounds) = &mut self.sounds {
                let l = listener(r, eye, target);
                sounds.update(&self.audio, r, &l, dt);
                sounds.ambient(&self.audio, &l, |site| track.site(site, r.time), dt);
            }
            if let Some(renderer) = &mut state.renderer {
                show(r, track, renderer, eye, None);
            }
            if self.follow {
                state.camera =
                    Camera::looking_at(renderer::from_game(eye), renderer::from_game(target));
                state.camera.fov = r.fov();
                return;
            }
            self.fly(dt);
            return;
        }
        // The podium runs its 7 s and then leaves for the results.
        if let Some((p, start)) = self.race.as_ref().and_then(|r| r.podium) {
            let Some(r) = &mut self.race else { return };
            r.time += dt;
            let (eye, target) = p.camera(r.time - start);
            if let Some(renderer) = &mut state.renderer {
                show(r, track, renderer, eye, None);
            }
            state.camera = Camera::looking_at(renderer::from_game(eye), renderer::from_game(target));
            return;
        }
        match &mut self.race {
            Some(r) => {
                r.buttons = buttons;
                let held = |a: usize| self.follow && acts[a];
                r.cam_keys = ride::CamKeys {
                    thrill: held(input::THRILL_CAM),
                    reverse: held(input::REVERSE_CAM),
                };
                let (clutch, back_up) = (held(input::CLUTCH), held(input::BACK_UP));
                let me = r.player;
                r.racers[me].bike.hands(clutch, back_up);
                r.actions = if self.follow {
                    [
                        acts[input::KAHUNA],
                        acts[input::TAUNT],
                    ]
                } else {
                    [false; 2]
                };
                let (eye, target) = r.update(&track.ground(), controls, dt);
                if let Some(g) = &mut self.ghost {
                    g.tick(r, &track.terrain, dt);
                }
                if let Some(n) = self.net {
                    let chat = net_race(
                        n,
                        r,
                        &mut self.remote,
                        &mut self.send_wait,
                        &track.terrain,
                        dt,
                    );
                    // Chat Box Always Off hides what the others say.
                    let shown = self.front.as_ref().is_none_or(|f| f.user().profile.game.chat_mode != 2);
                    if let (Some((_, hud)), true) = (&mut self.hud, shown) {
                        for line in chat {
                            hud.say(&line);
                        }
                    }
                }
                if let Some(renderer) = &mut state.renderer {
                    throw_dirt(r, &track.ground(), &mut renderer.particles, dt);
                    renderer.tyres(&tyre_places(r, &track.ground()), dt);
                }
                if let Some(rec) = &mut self.recording {
                    rec.tick(r, dt);
                }
                if let Some(log) = &mut self.ride_log {
                    log_ride(log, r, &acts, dt);
                }
                if let Some(sounds) = &mut self.sounds {
                    let l = listener(r, eye, target);
                    sounds.update(&self.audio, r, &l, dt);
                    sounds.ambient(&self.audio, &l, |site| track.site(site, r.time), dt);
                }
                if let Some(renderer) = &mut state.renderer {
                    let view = cues::Eye { at: eye, target, fov: r.fov(), aspect: renderer.aspect() };
                    show(r, track, renderer, eye, Some(view));
                    feel_wrecks(r, renderer);
                }
                let status = r.status();
                if status != self.title {
                    state
                        .window
                        .set_title(&format!("Motocross Madness 2 - {status}"));
                    self.title = status;
                }
                if self.follow {
                    state.camera =
                        Camera::looking_at(renderer::from_game(eye), renderer::from_game(target));
                    state.camera.fov = r.fov();
                    return;
                }
            }
            None => self.follow = false,
        }
        self.fly(dt);
    }

    fn redraw(&mut self) {
        self.sync_page_graphics();
        let now = self.start.elapsed().as_secs_f32();
        let progress = self.loading.as_ref().map_or(1.0, |_| {
            let steps = LOAD_STEPS.load(std::sync::atomic::Ordering::Relaxed).min(LOAD_TOTAL);
            steps as f32 / LOAD_TOTAL as f32
        });
        let Some(s) = &mut self.state else { return };
        let frame = match s.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                configure_surface(&s.surface, &s.device, &s.config);
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                skipped("the window's surface is out of date");
                configure_surface(&s.surface, &s.device, &s.config);
                return;
            }
            other => {
                skipped(&format!("{other:?}"));
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(s.config.format),
            ..Default::default()
        });
        let size = (s.config.width, s.config.height);
        // Text is drawn at the window's own resolution, Retina included.
        s.sprites.scale = (size.0 as f32 / 640.0).min(size.1 as f32 / 480.0);
        let mut commands = Vec::new();
        let racing = self.track.is_some() && self.loading.is_none();
        if let (true, Some(renderer)) = (racing, &mut s.renderer) {
            // Animations run on the race's clock, so a restart replays the gate.
            let time = match &self.race {
                Some(r) => r.time,
                None => now,
            };
            if let Some((_, hud)) = &mut self.hud {
                renderer.sky_on = hud.show.sky;
                renderer.shadows_on = hud.show.shadows;
                renderer.particles_on = hud.show.particles;
                let aspect = size.0 as f32 / size.1.max(1) as f32;
                hud.view_proj = Mat4::perspective_rh(s.camera.fovy(), aspect, 1.0, 20_000.0)
                    * s.camera.view();
            }
            if let (Some((_, hud)), Some(race), Some(track)) = (&mut self.hud, &self.race, &self.track) {
                hud.heads = race.racers.iter().map(|r| renderer.object_frame(r.object + 1, "Head", time)).collect();
                hud.look(&track.terrain, renderer::from_game(s.camera.position));
            }
            renderer.cinematic = self
                .race
                .as_ref()
                .filter(|_| self.replay.is_some())
                .map(|r| renderer::from_game(r.racers[r.player].bike.position()).distance(s.camera.position));
            commands.push(renderer.render(&s.device, &s.queue, &view, size, &s.camera, time));
            if let (Some((ui, hud)), Some(race)) = (&mut self.hud, &self.race) {
                let mut gfx = ui::Gfx {
                    sprites: &mut s.sprites,
                    device: &s.device,
                    queue: &s.queue,
                };
                // A replay shows the VCR player instead.
                match &mut self.replay {
                    Some(rp) => rp.draw(ui, &mut gfx, now),
                    None if race.podium.is_some() => {}
                    None => hud.draw(ui, &mut gfx, race, self.frame),
                }
                let mut encoder = s.device.create_command_encoder(&Default::default());
                s.sprites.stretch = true;
                s.sprites.widescreen = graphics::showing().is_some() && self.replay.is_none();
                s.sprites
                    .render(&s.device, &mut encoder, &view, size, false);
                s.sprites.stretch = false;
                s.sprites.widescreen = false;
                commands.push(encoder.finish());
            }
        } else if let Some(front) = &mut self.front {
            let mut gfx = ui::Gfx {
                sprites: &mut s.sprites,
                device: &s.device,
                queue: &s.queue,
            };
            front.draw(&mut gfx, now, progress);
            let mut encoder = s.device.create_command_encoder(&Default::default());
            s.sprites.render(&s.device, &mut encoder, &view, size, true);
            commands.push(encoder.finish());
            match front.preview() {
                Some((ride, turning)) => {
                    if s.bike_res.is_none() {
                        s.bike_res = front
                            .game()
                            .and_then(|g| read_archive(&g.join("RES").join("BIKE.RES")).ok());
                    }
                    if s.preview.as_ref().is_none_or(|p| *p.ride() != ride) {
                        s.preview = s
                            .bike_res
                            .as_deref()
                            .and_then(|b| rs2::Archive::parse(b).ok())
                            .and_then(|res| {
                                preview::Preview::new(
                                    &s.device,
                                    &s.queue,
                                    &res,
                                    &ride,
                                    s.config.format,
                                )
                            });
                    }
                    if let Some(p) = &mut s.preview {
                        p.window = front.preview_window();
                        p.spread = front.spread() == 2;
                        commands.push(
                            p.render(&s.device, &s.queue, &view, size, now, self.frame, turning),
                        );
                    }
                }
                None => s.preview = None,
            }
        }
        s.queue.submit(commands);
        s.window.pre_present_notify();
        s.queue.present(frame);
    }

    /// Acts on what the menus ask.
    fn next(&mut self, next: frontend::Next, event_loop: &ActiveEventLoop) {
        match next {
            frontend::Next::Stay => {}
            frontend::Next::Quit => {
                platform::quit();
                event_loop.exit();
            }
            frontend::Next::Race(setup) => self.load(setup),
            frontend::Next::Replay(path) => self.start_replay(&path),
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    #[cfg(not(target_arch = "wasm32"))]
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        match self.init(event_loop) {
            Ok(state) => {
                state.window.request_redraw();
                self.state = Some(state);
            }
            Err(e) => {
                eprintln!("error: {e}");
                event_loop.exit();
            }
        }
    }

    /// The browser cannot be waited on for the GPU, so the request goes off
    /// and the state comes back as [`UserEvent::Ready`].
    #[cfg(target_arch = "wasm32")]
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        use std::sync::atomic::{AtomicBool, Ordering};
        static ASKED: AtomicBool = AtomicBool::new(false);
        if self.state.is_some() || ASKED.swap(true, Ordering::Relaxed) {
            return;
        }
        let (window, instance, surface) = match Self::open_window(event_loop) {
            Ok(w) => w,
            Err(e) => {
                web_sys::console::error_1(&format!("error: {e}").into());
                return;
            }
        };
        wasm_bindgen_futures::spawn_local(async move {
            let made = async {
                let (adapter, device, queue) = gpu_from(&instance, Some(&surface)).await?;
                Self::make_state(window, surface, adapter, device, queue)
            };
            match made.await {
                Ok(state) => {
                    PROXY.with(|p| {
                        if let Some(p) = p.borrow().as_ref() {
                            let _ = p.send_event(UserEvent::Ready(Box::new(state)));
                        }
                    });
                }
                Err(e) => web_sys::console::error_1(&format!("error: {e}").into()),
            }
        });
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Ready(state) => {
                let mut state = *state;
                self.attach_track(&mut state);
                state.window.request_redraw();
                self.state = Some(state);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let now = self.start.elapsed().as_secs_f32();
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(on) => {
                eprintln!("{now:.2}s: the game {} focus", if on { "has" } else { "lost" });
            }
            WindowEvent::Occluded(hidden) => {
                eprintln!("{now:.2}s: the game is {}", if hidden { "covered" } else { "uncovered" });
            }
            WindowEvent::Resized(size) => {
                if let Some(s) = &mut self.state {
                    s.config.width = size.width.max(1);
                    s.config.height = size.height.max(1);
                    platform::set_screen(s.config.width, s.config.height);
                    configure_surface(&s.surface, &s.device, &s.config);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(s) = &self.state {
                    let size = (s.config.width, s.config.height);
                    // The replay player is drawn stretched over the window.
                    let stretch = self.replay.is_some() && !self.in_menus();
                    let mode = self.front.as_ref().map_or(0, |f| f.spread());
                    self.pointer = if mode > 0 && sprites::Sprites::layout(size).is_some() {
                        sprites::Sprites::to_screen_spread(size, position.x, position.y, mode)
                    } else {
                        sprites::Sprites::to_screen_as(size, position.x, position.y, stretch)
                    };
                }
                if self.in_menus() {
                    let (x, y) = self.pointer;
                    if let Some(f) = &mut self.front {
                        f.mouse_move(&self.audio, x, y);
                    }
                }
                if let (Some(rp), Some((ui, _))) = (&mut self.replay, &mut self.hud) {
                    let (x, y) = self.pointer;
                    rp.mouse_move(ui, x, y);
                }
            }
            WindowEvent::MouseWheel { delta, .. } if self.in_menus() => {
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y.round() as i32,
                    winit::event::MouseScrollDelta::PixelDelta(p) => (p.y / 15.0).round() as i32,
                };
                if let Some(f) = &mut self.front {
                    f.wheel(lines);
                }
            }
            WindowEvent::MouseInput {
                button: MouseButton::Left,
                state,
                ..
            } if self.in_menus() => {
                let (x, y) = self.pointer;
                let Some(f) = &mut self.front else { return };
                if state == ElementState::Pressed {
                    f.mouse_down(x, y);
                } else {
                    let next = f.mouse_up(&self.audio, x, y, now);
                    self.next(next, event_loop);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // The port's own keys, not the game's: Alt+Enter for full
                // screen, F2 for the frame rate, and with the enhanced
                // graphics F3 to compare them with the original look.
                if let (ElementState::Pressed, false, PhysicalKey::Code(code)) =
                    (event.state, event.repeat, event.physical_key)
                {
                    let alt = self.keys.contains(&KeyCode::AltLeft) || self.keys.contains(&KeyCode::AltRight);
                    if alt && matches!(code, KeyCode::Enter | KeyCode::NumpadEnter) {
                        if let Some(s) = &self.state {
                            let full = s.window.fullscreen().is_none();
                            s.window.set_fullscreen(full.then_some(winit::window::Fullscreen::Borderless(None)));
                        }
                        return;
                    }
                    if code == KeyCode::F2 {
                        self.fps = match self.fps {
                            Some(_) => {
                                if let Some(s) = &self.state {
                                    s.window.set_title("Motocross Madness 2");
                                }
                                None
                            }
                            None => Some((Instant::now(), 0, 0.0)),
                        };
                        return;
                    }
                    if code == KeyCode::F3 && graphics::enhanced().is_some() {
                        let on = graphics::toggle();
                        eprintln!("graphics: {}", if on { "enhanced" } else { "original" });
                        return;
                    }
                }
                // Typing into the menus' boxes.
                if self.in_menus() && event.state == ElementState::Pressed {
                    if let Some(f) = &mut self.front {
                        // The controls page takes the key itself.
                        if let PhysicalKey::Code(code) = event.physical_key {
                            if code != KeyCode::Escape && !event.repeat {
                                let next = f.key(&self.audio, code);
                                if !matches!(next, frontend::Next::Stay) {
                                    self.next(next, event_loop);
                                    return;
                                }
                            }
                        }
                        let Some(f) = &mut self.front else { return };
                        if event.physical_key == PhysicalKey::Code(KeyCode::Backspace) {
                            f.type_char('\u{8}');
                        } else if let Some(text) = &event.text {
                            text.chars().for_each(|c| f.type_char(c));
                        }
                    }
                }
                // A chat line takes the keys while it is typed.
                if self.chat.is_some() && event.state == ElementState::Pressed {
                    self.type_chat(&event);
                    return;
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape && event.state == ElementState::Pressed {
                        if self.in_menus() {
                            if let Some(f) = &mut self.front {
                                let next = f.escape(now);
                                self.next(next, event_loop);
                            }
                        } else if self.front.is_some() {
                            if self.loading.is_none() {
                                self.end_race();
                            }
                        } else {
                            event_loop.exit();
                        }
                    }
                    if event.state == ElementState::Pressed && !event.repeat && self.track.is_some()
                    {
                        // Enter or Space ends the podium (0x460190).
                        let podium = self.race.as_ref().is_some_and(|r| r.podium.is_some());
                        if podium && matches!(code, KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space) {
                            self.back_to_menu();
                            return;
                        }
                        match code {
                            KeyCode::Tab => self.follow = !self.follow && self.race.is_some(),
                            KeyCode::KeyR => {
                                if let (Some(r), Some(t)) = (&mut self.race, &self.track) {
                                    r.restart(&t.ground());
                                }
                            }
                            KeyCode::Equal | KeyCode::NumpadAdd => {
                                if let Some((_, hud)) = &mut self.hud {
                                    hud.zoom_by(-1.0);
                                }
                            }
                            KeyCode::Minus | KeyCode::NumpadSubtract => {
                                if let Some((_, hud)) = &mut self.hud {
                                    hud.zoom_by(1.0);
                                }
                            }
                            // Numpad 5 cycles the views (0x467ec0); C too,
                            // for keyboards without a number pad.
                            KeyCode::KeyS if self.ctrl() => self.flip_sound(),
                            KeyCode::KeyV if self.ctrl() => self.flip_reverb(),
                            KeyCode::KeyC if self.ctrl() => {
                                let said = match (&mut self.sounds, &self.audio) {
                                    (Some(s), a) => s.step_crowd(a),
                                    _ => None,
                                };
                                if let (Some(id), Some((ui, hud))) = (said, &mut self.hud) {
                                    hud.say(&ui.lang(id));
                                }
                            }
                            KeyCode::Numpad0 if self.follow => {
                                if let Some(r) = &mut self.race {
                                    r.freeze_camera();
                                }
                            }
                            KeyCode::Numpad5 | KeyCode::KeyC if self.follow => {
                                if let Some(r) = &mut self.race {
                                    let name = r.next_view();
                                    if let Some((_, hud)) = &mut self.hud {
                                        hud.say(name);
                                    }
                                }
                            }
                            // Ctrl+G and Ctrl+B flip the gyros; Ctrl and a
                            // digit sends that chat message, and / opens the
                            // chat (Shift+/ its mode) in a network race;
                            // [ and ], Page Up and Down watch the others
                            // (0x4203e0).
                            KeyCode::KeyG | KeyCode::KeyB if self.ctrl() => self.flip_gyro(code == KeyCode::KeyG),
                            KeyCode::Digit0
                            | KeyCode::Digit1
                            | KeyCode::Digit2
                            | KeyCode::Digit3
                            | KeyCode::Digit4
                            | KeyCode::Digit5
                            | KeyCode::Digit6
                            | KeyCode::Digit7
                            | KeyCode::Digit8
                            | KeyCode::Digit9
                                if self.ctrl() && self.net.is_some() =>
                            {
                                let d = code as usize - KeyCode::Digit0 as usize;
                                let line = self.front.as_ref().and_then(|f| f.user().profile.messages.get(d).cloned());
                                if let Some(line) = line.filter(|l| !l.is_empty()) {
                                    self.send_chat(&line);
                                }
                            }
                            KeyCode::Slash if self.net.is_some() => {
                                let shift = self.keys.contains(&KeyCode::ShiftLeft) || self.keys.contains(&KeyCode::ShiftRight);
                                self.chat_key(shift);
                            }
                            KeyCode::BracketLeft | KeyCode::PageUp | KeyCode::BracketRight | KeyCode::PageDown => {
                                if let Some(r) = &mut self.race {
                                    r.watch_next(matches!(code, KeyCode::BracketRight | KeyCode::PageDown));
                                }
                            }
                            // The race's switches (0x4e54c0).
                            KeyCode::F4 => self.flip_cues(),
                            KeyCode::F11 | KeyCode::F12 => self.step_quality(if code == KeyCode::F12 { 1 } else { -1 }),
                            KeyCode::F5 | KeyCode::F6 | KeyCode::F7 | KeyCode::F8 | KeyCode::F9 | KeyCode::F10 => {
                                self.flip_switch(code);
                            }
                            KeyCode::KeyN if self.ctrl() => {
                                if let Some((ui, hud)) = &mut self.hud {
                                    let (what, on) = hud.show.cycle_labels();
                                    hud.say_switch(ui, what, on);
                                }
                                self.keep_switches();
                            }
                            KeyCode::Home | KeyCode::End => {
                                if let Some(r) = &mut self.race {
                                    r.zoom(if code == KeyCode::Home { -1.0 } else { 1.0 });
                                }
                            }
                            KeyCode::KeyP => {
                                if let Some(r) = &mut self.race {
                                    r.autopilot = !r.autopilot;
                                }
                            }
                            _ => {}
                        }
                    }
                    match event.state {
                        ElementState::Pressed => self.keys.insert(code),
                        ElementState::Released => self.keys.remove(&code),
                    };
                }
            }
            WindowEvent::MouseInput {
                button: MouseButton::Left,
                state,
                ..
            } if self.replay.is_some() => {
                let (x, y) = self.pointer;
                let (Some(rp), Some((ui, _))) = (&mut self.replay, &mut self.hud) else {
                    return;
                };
                if state == ElementState::Pressed {
                    rp.mouse_down(ui, x, y);
                    return;
                }
                match rp.mouse_up(ui, x, y) {
                    replay::Ask::Stay => {}
                    replay::Ask::Camera => {
                        if let Some(r) = &mut self.race {
                            r.next_view();
                        }
                    }
                    replay::Ask::Exit => self.back_to_menu(),
                    replay::Ask::Save => {
                        let dir = self.front.as_ref().map(|f| f.user().store.dir.clone());
                        let track = self.track.as_ref().map_or("", |t| t.name.as_str());
                        if let Some(dir) = dir {
                            match replay::save(&rp.path, &dir, track) {
                                Ok(p) => eprintln!("saved the replay as {}", p.display()),
                                Err(e) => eprintln!("saving the replay: {e}"),
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseInput {
                button: MouseButton::Right,
                state,
                ..
            } if self.track.is_some() => {
                self.looking = state == ElementState::Pressed;
                if let Some(s) = &self.state {
                    let mode = if self.looking {
                        CursorGrabMode::Locked
                    } else {
                        CursorGrabMode::None
                    };
                    // Some platforms only support Confined; looking still works without a grab.
                    let _ = s
                        .window
                        .set_cursor_grab(mode)
                        .or_else(|_| s.window.set_cursor_grab(CursorGrabMode::Confined));
                    s.window.set_cursor_visible(!self.looking);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(n) = self.front.as_mut().and_then(|f| f.take_next()) {
                    self.next(n, event_loop);
                }
                let now = Instant::now();
                let dt = (now - self.last).as_secs_f32().min(0.1);
                self.last = now;
                self.frame = dt;
                self.poll_pads(event_loop, dt);
                self.update(dt);
                self.redraw();
                if let Some((since, frames, slowest)) = &mut self.fps {
                    *frames += 1;
                    *slowest = slowest.max(dt);
                    let span = since.elapsed().as_secs_f32();
                    if span >= 1.0 {
                        let text = format!(
                            "{:.0} fps ({:.1} ms a frame, slowest {:.1} ms)",
                            *frames as f32 / span,
                            span * 1000.0 / *frames as f32,
                            *slowest * 1000.0
                        );
                        eprintln!("{text}");
                        if let Some(s) = &self.state {
                            s.window.set_title(&format!("Motocross Madness 2 - {text}"));
                        }
                        self.fps = Some((Instant::now(), 0, 0.0));
                    }
                }
                if let Some(s) = &self.state {
                    s.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        if let (true, DeviceEvent::MouseMotion { delta }, Some(s)) =
            (self.looking, event, &mut self.state)
        {
            s.camera
                .look(delta.0 as f32 * 0.003, delta.1 as f32 * 0.003);
        }
    }
}

/// The OpenMCM2 tyre for the title bar and taskbar (`assets/icon`).
fn window_icon() -> Option<winit::window::Icon> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(&include_bytes!("../../../assets/icon/icon-256.png")[..]));
    decoder.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    winit::window::Icon::from_rgba(buf, info.width, info.height).ok()
}
