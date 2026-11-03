//! The finish line's people and arch in a Nationals or Supercross race
//! (bikerace.cpp: 0x418cd0 puts them up, 0x41e0b0 runs them, 0x421f90
//! starts the countdown):
//!
//! * `MetalArch.slt` over the finish line, scaled to its width.
//! * The flag girl (`FlagGirl.mcf`) beside it on `GirlBox.slt`, who waves
//!   the white flag as the leader comes round to the last lap and the
//!   checkered flag at the end.
//! * The 30-second board man (the scene's `30SecMan.mcf`), who stands
//!   15 ft in front of the player's bike and walks in during the
//!   countdown, gone when the gate drops.
//!
//! The girl and the man are only out in a race, not in practice or a
//! ghost race (`0x2d70` 0 and 4).

use glam::Vec3;
use mm2_formats::{scn, tdf};

pub const ARCH: &str = "MetalArch.slt";
pub const GIRL_BOX: &str = "GirlBox.slt";
pub const GIRL: &str = "FlagGirl.mcf";
pub const MAN: &str = "30SecMan.mcf";
/// The girl's flag is her second material, the checkered flag; the white
/// one is the third (0x41f860 swaps the faces between them).
pub const WHITE: &str = "Flag_White01_06.tga";

/// The arch is 1.111 times the line's width (0x555fcc); the girl stands
/// 9.2 widths out and 1.8 back (0x555fc8, 0x555fc4), 8 ft up, and her
/// box 1.22 ft in and 1.2 ft forward of her, 4 ft up (0x555fc0, 0x5555e8).
const ARCH_SPAN: f32 = 1.111;
const GIRL_OUT: f32 = 9.2;
const GIRL_BACK: f32 = -1.8;
const GIRL_UP: f32 = 8.0;
const BOX_IN: f32 = 1.22;
const BOX_FORWARD: f32 = 1.2;
const BOX_UP: f32 = 4.0;

/// How long the man has been walking when the gate drops, s (0x421f90's
/// 8.2 s countdown with him there).
pub const MAN_LEAD: f32 = 8.2;
/// How far in front of the player's bike he stands, ft (0x555800).
pub const MAN_AHEAD: f32 = 15.0;

/// Where the arch, the girl's box and the girl go for finish `line`
/// (`arch_width`, the arch model's width; `ground`, the height at x, z).
pub fn layout(
    line: &tdf::Line,
    arch_width: f32,
    ground: impl Fn(f32, f32) -> f32,
) -> (scn::Placement, scn::Placement, scn::Animation) {
    let dir = Vec3::new(line.direction[0], 0.0, line.direction[2]).normalize_or(Vec3::Z);
    let right = Vec3::Y.cross(dir);
    let mut at = Vec3::from(line.position);
    at.y = ground(at.x, at.z);
    let s = line.width / arch_width.max(1e-3) * ARCH_SPAN;
    let girl = at + right * (GIRL_OUT * s) + dir * (GIRL_BACK * s);
    let floor = ground(girl.x, girl.z);
    let girl = Vec3::new(girl.x, floor + GIRL_UP, girl.z);
    let girl_box = at + right * (GIRL_OUT * s - BOX_IN) + dir * (GIRL_BACK * s + BOX_FORWARD);
    let girl_box = Vec3::new(girl_box.x, floor + BOX_UP, girl_box.z);
    let placement = |model: &str, position: Vec3, scale: f32| scn::Placement {
        model: model.into(),
        position: position.into(),
        look: dir.into(),
        up: [0.0, 1.0, 0.0],
        scale: [scale; 3],
        lighting: Default::default(),
    };
    (
        placement(ARCH, at, s),
        placement(GIRL_BOX, girl_box, 1.0),
        scn::Animation {
            model: GIRL.into(),
            offset: girl.into(),
            look: dir.into(),
            up: [0.0, 1.0, 0.0],
            scale: [1.0; 3],
            motion: Some("Stand".into()),
            lighting: Default::default(),
        },
    )
}

/// What the flag girl is doing (0x41e0b0, her state in `+0x78`).
#[derive(Debug, Clone, Copy)]
pub struct FlagGirl {
    state: u8,
    /// The white flag out, else the checkered one.
    pub white: bool,
    pub motion: &'static str,
    /// When the motion started, race time.
    pub start: f32,
}

impl Default for FlagGirl {
    fn default() -> Self {
        FlagGirl {
            state: 0,
            white: true,
            motion: "Stand",
            start: 0.0,
        }
    }
}

/// The motions' lengths, s (`FlagGirl.mcf`, 30 fps).
fn length(motion: &str) -> f32 {
    match motion {
        "Stand" => 111.0 / 30.0,
        "FlagStart" => 185.0 / 30.0,
        _ => f32::INFINITY,
    }
}

/// She starts waving when the player is this close before her, and stops
/// this far past, ft (0x555b7c, 0x5555ec).
const WAVE_BEFORE: f32 = 300.0;
const WAVE_PAST: f32 = 100.0;

impl FlagGirl {
    fn play(&mut self, motion: &'static str, time: f32) {
        self.motion = motion;
        self.start = time;
    }

    /// One frame at race `time`: the player has done `laps` of `total`,
    /// may have `finished`, and is `ahead` ft before the finish line along
    /// the course and `behind` ft past it.
    pub fn update(
        &mut self,
        time: f32,
        laps: u32,
        total: u32,
        finished: bool,
        ahead: f32,
        behind: f32,
    ) {
        let second = laps + 2 == total;
        let last = laps + 1 == total;
        if !second && !last {
            return;
        }
        let done = time - self.start >= length(self.motion);
        let checkered = 'wave: {
            if !(self.state == 0 || !finished || behind <= WAVE_PAST) {
                self.white = true;
                self.play("Stand", time);
                self.state = 0;
                return;
            }
            if (second && ahead < WAVE_BEFORE) || (last && behind < WAVE_PAST) {
                if self.state == 1 {
                    if done {
                        self.play("FlagLoop", time);
                    }
                } else {
                    self.white = true;
                    self.state = 1;
                    self.play("FlagStart", time);
                }
                return;
            }
            if self.state == 0 || self.state > 2 {
                break 'wave true;
            }
            if !second || ahead <= WAVE_PAST {
                if !last {
                    break 'wave false;
                }
                if behind <= WAVE_PAST {
                    break 'wave true;
                }
            }
            self.play("Stand", time);
            self.state = 0;
            return;
        };
        // The checkered flag, coming to the line on the last lap or just
        // over it at the end.
        let near = (checkered && last && ahead < WAVE_BEFORE) || (finished && behind < WAVE_PAST);
        if !near {
            return;
        }
        if self.state < 3 {
            self.white = false;
            self.state = 3;
            self.play("FlagStart", time);
        } else if done {
            self.play("FlagLoop", time);
            self.state = 4;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_then_checkered() {
        let mut g = FlagGirl::default();
        // Two laps to go, far off: she stands.
        g.update(1.0, 1, 3, false, 900.0, 50.0);
        assert_eq!((g.motion, g.white), ("Stand", true));
        // Coming round to the last lap.
        g.update(2.0, 1, 3, false, 250.0, 700.0);
        assert_eq!((g.motion, g.white), ("FlagStart", true));
        g.update(9.0, 1, 3, false, 10.0, 900.0);
        assert_eq!(g.motion, "FlagLoop");
        // Over the line into the last lap she keeps waving, then stops.
        g.update(10.0, 2, 3, false, 900.0, 50.0);
        assert_eq!(g.motion, "FlagLoop");
        g.update(12.0, 2, 3, false, 700.0, 200.0);
        assert_eq!(g.motion, "Stand");
        // The checkered flag for the finish.
        g.update(50.0, 2, 3, false, 250.0, 700.0);
        assert_eq!((g.motion, g.white), ("FlagStart", false));
    }
}
