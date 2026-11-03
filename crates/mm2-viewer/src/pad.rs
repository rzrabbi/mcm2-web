//! Joysticks: game pads (Xbox, PlayStation and the like) and flight
//! sticks, read the way the game's DirectInput joystick reads them
//! (`PCJoystickDevice`, 0x4c6ec0 to 0x4c8aa0).
//!
//! The game takes a `DIJOYSTATE` each frame (0x4c7ee0): six axes (X, Y,
//! Z, RX, RY, RZ) and 32 buttons. The control map (`control.ctl`) names
//! them as DirectInput numbers them: buttons from 0, axes by the codes
//! −3 X, −5 Y, −7 Z, −9 RX, −11 RY, −13 RZ. Here a pad with a known
//! layout (an Xbox or PlayStation pad, through the SDL mappings) gives
//! its buttons in the order DirectInput lists an Xbox pad's (A, B, X, Y,
//! the shoulders, Back, Start, the stick clicks); any other stick gives
//! its own buttons and axes in their HID order, as DirectInput would.
//!
//! Every pad and stick plugged in is read together; the one last touched
//! names the buttons on the controls page.
//!
//! An axis that rests at one end rather than in the middle (a trigger, a
//! pedal, or a device that only looks like a stick) would hold the bike
//! leaned or steered for good, so the first moment a pad is seen its
//! axes are taken at rest: one resting past nine tenths reads from its
//! rest instead, 0 there and 1 at its other end. DirectInput's
//! calibration does as much for the game.
//!
//! Axis values here are DirectInput's way round, −1 to 1 with Y growing
//! towards the rider (stick pulled back).

use std::collections::{HashMap, HashSet};
use web_time::{Duration, Instant};

use gilrs::{Axis, Button, Gamepad, GamepadId, Gilrs, MappingSource};

/// The axes found resting at an end: each one's rest, −1 or 1, by pad
/// and native axis code.
type Rest = HashMap<(GamepadId, u32), f32>;

/// How long after a pad is first seen its axes count as at rest.
const SETTLE: Duration = Duration::from_millis(1500);

/// The share of a stick's travel the pad in use must be pushed for the
/// others to be passed over (stick drift is less).
const IN_USE: f32 = 0.15;

/// A known pad's buttons in DirectInput's order for an Xbox pad.
const PAD_BUTTONS: [Button; 10] = [
    Button::South,
    Button::East,
    Button::West,
    Button::North,
    Button::LeftTrigger,
    Button::RightTrigger,
    Button::Select,
    Button::Start,
    Button::LeftThumb,
    Button::RightThumb,
];

/// The six axis codes, X to RZ.
pub const AXES: [i32; 6] = [-3, -5, -7, -9, -11, -13];

/// How far a stick goes before it counts as pushed, as the game's
/// gamepad reading has it (0x4c74e0: below 16384 or above 49152 of
/// 0 to 65535) and for binding an axis on the controls page.
pub const PUSHED: f32 = 0.5;

/// The steering axis's dead centre (0x449e20 makes the axis with 0.04).
pub const STEER_DEAD: f32 = 0.04;

/// The analog throttle: nothing below 0.2 (0x52e3e0), and the gas counts
/// as open past 0.33 (0x493270).
pub const THROTTLE_FLOOR: f32 = 0.2;
pub const THROTTLE_ON: f32 = 0.33;

/// Something the joystick did this frame, for the controls page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Press {
    /// A button went down (DirectInput's number, from 0).
    Button(i32),
    /// An axis went over half way (its code, −3 X and so on).
    Axis(i32),
}

pub struct Pads {
    gilrs: Gilrs,
    /// The pad last touched: the one that drives the bike.
    active: Option<GamepadId>,
    /// Each axis's side last frame on the active pad, to see one cross
    /// half way.
    pushed: [bool; 7],
    /// The buttons down last frame, a bit each.
    held: u32,
    /// When each pad was first seen, to take its axes at rest.
    first: HashMap<GamepadId, Instant>,
    /// The axes resting at an end, and those seen in the middle while
    /// the pad settles.
    rest: Rest,
    centred: HashSet<(GamepadId, u32)>,
}

impl Pads {
    pub fn new() -> Option<Pads> {
        let gilrs = Gilrs::new()
            .map_err(|err| eprintln!("no gamepads: {err}"))
            .ok()?;
        for (_, pad) in gilrs.gamepads() {
            eprintln!("joystick: {} ({})", pad.name(), kind_name(&pad));
        }
        Some(Pads {
            gilrs,
            active: None,
            pushed: [false; 7],
            held: 0,
            first: HashMap::new(),
            rest: HashMap::new(),
            centred: HashSet::new(),
        })
    }

    /// Takes the joysticks' events; the pad a button or a stick moves on
    /// becomes the one in use. Returns the active pad's new presses.
    pub fn poll(&mut self) -> Vec<Press> {
        use gilrs::EventType::*;
        while let Some(ev) = self.gilrs.next_event() {
            match ev.event {
                ButtonPressed(..) => self.take(ev.id),
                AxisChanged(_, v, code)
                    if v.abs() > PUSHED && !self.rest.contains_key(&(ev.id, code.into_u32())) =>
                {
                    self.take(ev.id)
                }
                Connected => {
                    let pad = self.gilrs.gamepad(ev.id);
                    eprintln!("joystick: {} ({})", pad.name(), kind_name(&pad));
                }
                Disconnected => {
                    if self.active == Some(ev.id) {
                        self.active = None;
                    }
                    self.first.remove(&ev.id);
                    self.rest.retain(|&(id, _), _| id != ev.id);
                    self.centred.retain(|&(id, _)| id != ev.id);
                }
                _ => {}
            }
        }
        self.settle();
        // Every pad and stick plugged in counts together, so a stick and
        // a separate throttle (or a wheel and its pedals) work as one.
        if self.gilrs.gamepads().next().is_none() {
            return Vec::new();
        }
        let held = (0..32).filter(|&n| self.button(n)).fold(0u32, |m, n| m | 1 << n);
        let mut pushed = [false; 7];
        for (i, &code) in AXES.iter().enumerate() {
            // Z only as a throttle lever (below): a game pad's Z is
            // its triggers, which rest half way.
            pushed[i] = i != 2 && self.axis(code).is_some_and(|v| v.abs() > PUSHED);
        }
        pushed[6] = self.gilrs.gamepads().any(|(_, p)| throttle(&p).is_some_and(|v| v > PUSHED));
        let fresh = held & !self.held;
        self.held = held;
        let mut out = Vec::new();
        for (i, &now) in pushed.iter().enumerate() {
            if now && !self.pushed[i] {
                out.push(Press::Axis(if i == 6 { -7 } else { AXES[i] }));
            }
        }
        self.pushed = pushed;
        out.extend((0..32).filter(|n| fresh & 1 << n != 0).map(Press::Button));
        let info = self.info();
        if crate::screens::keys::pad() != info {
            crate::screens::keys::set_pad(info);
        }
        out
    }

    /// What the pad does to the menus' pointer: the left stick or the
    /// D-pad's push (x, y, screen-down positive), and A and B held.
    pub fn pointer(&self) -> ([f32; 2], bool, bool) {
        let Some(pad) = self.pad() else {
            return ([0.0; 2], false, false);
        };
        let mut v = [
            axis(&pad, -3, &self.rest).unwrap_or(0.0),
            axis(&pad, -5, &self.rest).unwrap_or(0.0),
        ];
        for (b, i, s) in [
            (Button::DPadLeft, 0, -1.0),
            (Button::DPadRight, 0, 1.0),
            (Button::DPadUp, 1, -1.0),
            (Button::DPadDown, 1, 1.0),
        ] {
            if pad.is_pressed(b) {
                v[i] = s;
            }
        }
        (v, button(&pad, 0), button(&pad, 1))
    }

    /// Whether DirectInput's button `n` is down on any pad or stick.
    pub fn button(&self, n: i32) -> bool {
        self.gilrs.gamepads().any(|(_, p)| button(&p, n))
    }

    /// An axis (−3 X to −13 RZ): the pad in use's, unless it sits in
    /// the middle, when the one pushed furthest over every pad and stick
    /// (a stick and a separate throttle, say, work as one).
    pub fn axis(&self, code: i32) -> Option<f32> {
        let own = self.pad().and_then(|p| axis(&p, code, &self.rest));
        if own.is_some_and(|v| v.abs() >= IN_USE) {
            return own;
        }
        self.gilrs
            .gamepads()
            .filter_map(|(_, p)| axis(&p, code, &self.rest))
            .reduce(|a, b| if b.abs() > a.abs() { b } else { a })
    }

    /// Accelerate on the Z axis: a stick's throttle lever (a separate
    /// throttle counts), else a pad's Z, as `throttle_axis`.
    pub fn throttle_axis(&self, code: i32) -> Option<f32> {
        let lever = self.gilrs.gamepads().find(|(_, p)| throttle(p).is_some());
        match lever {
            Some((_, p)) => throttle_axis(&p, code, &self.rest),
            None => self.pad().and_then(|p| throttle_axis(&p, code, &self.rest)),
        }
    }

    /// A pad's trigger, the furthest pulled over every pad, 0 to 1.
    pub fn trigger(&self, b: Button) -> f32 {
        self.gilrs
            .gamepads()
            .map(|(_, p)| p.button_data(b).map_or(0.0, |d| d.value()))
            .fold(0.0, f32::max)
    }

    /// The pad in use as the controls page names it.
    pub fn info(&self) -> Option<crate::screens::keys::PadInfo> {
        use crate::screens::keys::{PadFamily, PadInfo};
        let pad = self.pad()?;
        let family = known(&pad).then(|| match pad.vendor_id() {
            Some(0x054c) => PadFamily::PlayStation,
            Some(0x057e) => PadFamily::Nintendo,
            _ => PadFamily::Xbox,
        });
        Some(PadInfo {
            family,
            name: pad.name().to_string(),
            device: self.device(),
        })
    }

    /// Takes each pad's axes at rest in its first moments: one past nine
    /// tenths and not seen in the middle rests at that end.
    fn settle(&mut self) {
        let now = Instant::now();
        for (id, pad) in self.gilrs.gamepads() {
            let since = *self.first.entry(id).or_insert(now);
            if now.duration_since(since) > SETTLE {
                continue;
            }
            for (code, data) in pad.state().axes() {
                let key = (id, code.into_u32());
                let v = data.value();
                if v.abs() < PUSHED {
                    self.centred.insert(key);
                    self.rest.remove(&key);
                } else if v.abs() > 0.9 && !self.centred.contains(&key) && !self.rest.contains_key(&key) {
                    eprintln!("joystick: {} axis {:#x} rests at {v:+.2}", pad.name(), key.1);
                    self.rest.insert(key, v.signum());
                }
            }
        }
    }

    fn take(&mut self, id: GamepadId) {
        if self.active != Some(id) {
            let pad = self.gilrs.gamepad(id);
            eprintln!("joystick in use: {} ({})", pad.name(), kind_name(&pad));
            self.active = Some(id);
        }
    }

    /// The pad in use: the last one touched, else the first connected.
    pub fn pad(&self) -> Option<Gamepad<'_>> {
        match self.active.and_then(|id| self.gilrs.connected_gamepad(id)) {
            Some(p) => Some(p),
            None => self.gilrs.gamepads().next().map(|(_, p)| p),
        }
    }

    /// The device the controls page and `control.ctl` file the pad
    /// under (strings 460 to 467), as 0x44a1b0 picks it from what
    /// DirectInput says: a game pad is 3 (Standard Game Pad), any other
    /// stick 1 (2 Axis, 4+ Button Joystick). With none it is 6, the
    /// keyboard.
    pub fn device(&self) -> usize {
        match self.pad() {
            Some(p) if known(&p) => 3,
            Some(_) => 1,
            None => crate::input::KEYBOARD,
        }
    }
}

/// Whether gilrs knows the pad's layout (an Xbox, PlayStation, Switch or
/// other pad in the SDL list), rather than a stick read as it comes.
fn known(pad: &Gamepad) -> bool {
    pad.mapping_source() != MappingSource::None
}

fn kind_name(pad: &Gamepad) -> &'static str {
    if known(pad) {
        "game pad"
    } else {
        "joystick"
    }
}

/// Whether DirectInput's button `n` is down.
pub fn button(pad: &Gamepad, n: i32) -> bool {
    let Ok(n) = usize::try_from(n) else {
        return false;
    };
    if known(pad) {
        return PAD_BUTTONS.get(n).is_some_and(|&b| pad.is_pressed(b));
    }
    pad.state()
        .buttons()
        .any(|(code, data)| data.is_pressed() && raw_button(code.into_u32()) == Some(n))
}

/// An axis (−3 X to −13 RZ), DirectInput's way round; `None` when the
/// stick has no such axis.
pub fn axis(pad: &Gamepad, code: i32, rest: &Rest) -> Option<f32> {
    let i = AXES.iter().position(|&c| c == code)?;
    if known(pad) {
        // DirectInput gives an Xbox pad's triggers as one Z axis, the
        // left trigger one way and the right the other.
        if i == 2 {
            let t = |b| pad.button_data(b).map_or(0.0, |d| d.value());
            return Some(t(Button::LeftTrigger2) - t(Button::RightTrigger2));
        }
        let (a, sign) = [
            (Axis::LeftStickX, 1.0),
            (Axis::LeftStickY, -1.0),
            (Axis::LeftZ, 1.0),
            (Axis::RightStickX, 1.0),
            (Axis::RightStickY, -1.0),
            (Axis::RightZ, 1.0),
        ][i];
        let code = pad.axis_code(a)?;
        let v = pad.state().axis_data(code)?.value();
        return Some(from_rest(pad, code.into_u32(), v, rest) * sign);
    }
    raw_axis(pad, i, Some(rest))
}

/// An axis's value from its rest: as it is if it rests in the middle,
/// else 0 at rest to ±1 at its other end.
fn from_rest(pad: &Gamepad, code: u32, v: f32, rest: &Rest) -> f32 {
    match rest.get(&(pad.id(), code)) {
        Some(r) => (v - r) * 0.5,
        None => v,
    }
}

/// A stick's throttle lever, 0 pulled back to 1 pushed forward: its Z
/// axis, which the game reads the other way round (0x4c7ee0 takes
/// range − Z), or a slider when it has no Z, as many sticks report
/// their throttle.
pub fn throttle(pad: &Gamepad) -> Option<f32> {
    if known(pad) {
        return None;
    }
    let z = raw_axis(pad, 2, None).or_else(|| raw_axis(pad, 6, None))?;
    Some((1.0 - z) * 0.5)
}

/// Accelerate bound to the Z axis (−7, or its other half −6): how far
/// open, 0 to 1. The game makes this axis 0 to 1 over Z's range
/// (0x449e20), Z read as range − Z, with nothing below 0.2. (Bound to RZ
/// the game's axis is stuck at 1; here RZ reads like Z.)
pub fn throttle_axis(pad: &Gamepad, code: i32, rest: &Rest) -> Option<f32> {
    let v = match code {
        -7 | -6 => throttle(pad).or_else(|| axis(pad, -7, rest).map(|z| (1.0 - z) * 0.5))?,
        -13 | -12 => (1.0 - axis(pad, -13, rest)?) * 0.5,
        _ => return None,
    };
    Some(if v <= THROTTLE_FLOOR { 0.0 } else { v })
}

/// A stick's own axis by DirectInput's place (0 X to 5 RZ, 6 the
/// slider), the way round DirectInput gives it; read from its rest when
/// given the rests (not for a throttle lever, which rests at an end).
fn raw_axis(pad: &Gamepad, i: usize, rest: Option<&Rest>) -> Option<f32> {
    let (code, data) = pad
        .state()
        .axes()
        .find(|(code, _)| raw_axis_place(code.into_u32()) == Some(i))?;
    // gilrs turns the axes it takes for Y round so that up is positive;
    // DirectInput (and the HID report) has down positive.
    let flipped = [Axis::LeftStickY, Axis::RightStickY]
        .into_iter()
        .any(|a| pad.axis_code(a) == Some(code));
    let v = match rest {
        Some(rest) => from_rest(pad, code.into_u32(), data.value(), rest),
        None => data.value(),
    };
    Some(if flipped { -v } else { v })
}

/// The DirectInput button number of a stick's native button code.
fn raw_button(code: u32) -> Option<usize> {
    let (kind, n) = (code >> 16, (code & 0xffff) as usize);
    if cfg!(target_os = "macos") {
        // HID: the button page, usages from 1.
        (kind == 0x09 && n >= 1).then(|| n - 1)
    } else if cfg!(target_os = "linux") {
        // evdev: BTN_JOYSTICK on, then the pad block, then BTN_TRIGGER_HAPPY.
        match (kind, n) {
            (1, 0x120..=0x12f) => Some(n - 0x120),
            (1, 0x130..=0x13f) => Some(n - 0x130),
            (1, 0x2c0..=0x2e7) => Some(n - 0x2c0 + 16),
            _ => None,
        }
    } else {
        // Windows.Gaming.Input: kind 0 buttons, by index.
        (kind == 0).then_some(n)
    }
}

/// DirectInput's place (0 X to 5 RZ, 6 a slider or throttle) of a
/// stick's native axis code.
fn raw_axis_place(code: u32) -> Option<usize> {
    let (kind, n) = (code >> 16, code & 0xffff);
    if cfg!(target_os = "macos") {
        match (kind, n) {
            // Generic desktop X, Y, Z, Rx, Ry, Rz, then the slider and dial.
            (0x01, 0x30..=0x35) => Some((n - 0x30) as usize),
            (0x01, 0x36 | 0x37) => Some(6),
            // Simulation controls: the throttle.
            (0x02, 0xbb) => Some(6),
            _ => None,
        }
    } else if cfg!(target_os = "linux") {
        // evdev ABS_X to ABS_RZ, then ABS_THROTTLE.
        match (kind, n) {
            (3, 0..=5) => Some(n as usize),
            (3, 6) => Some(6),
            _ => None,
        }
    } else {
        // Windows.Gaming.Input: kind 1 axes, by index.
        match (kind, n) {
            (1, 0..=5) => Some(n as usize),
            (1, 6) => Some(6),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_codes() {
        if cfg!(target_os = "linux") {
            assert_eq!(raw_button((1 << 16) | 0x120), Some(0));
            assert_eq!(raw_button((1 << 16) | 0x12b), Some(11));
            assert_eq!(raw_axis_place((3 << 16) | 2), Some(2));
            assert_eq!(raw_axis_place((3 << 16) | 0x10), None);
        }
    }
}
