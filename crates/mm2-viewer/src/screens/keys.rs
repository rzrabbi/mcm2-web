//! Keys as the game codes them: DirectInput scan codes (`DIK_*`), the
//! codes `control.ctl` stores, and the names the controls page shows
//! (`0x44a310`: "Keyboard" and Windows' `GetKeyNameText`, the shift,
//! control and alt keys by strings 5059 to 5064).

use std::sync::Mutex;
use winit::keyboard::KeyCode;

/// The pad in use, for the controls page to name its buttons the way
/// the pad itself is labelled (the game only said "Joy Button 1").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadFamily {
    Xbox,
    PlayStation,
    Nintendo,
}

/// The pad in use: its family (none for a stick read as it comes), its
/// name, and the device the controls page files it under.
#[derive(Debug, Clone, PartialEq)]
pub struct PadInfo {
    pub family: Option<PadFamily>,
    pub name: String,
    pub device: usize,
}

static PAD: Mutex<Option<PadInfo>> = Mutex::new(None);

/// Sets (or clears) the pad in use.
pub fn set_pad(info: Option<PadInfo>) {
    if let Ok(mut p) = PAD.lock() {
        *p = info;
    }
}

/// The pad in use.
pub fn pad() -> Option<PadInfo> {
    PAD.lock().ok().and_then(|p| p.clone())
}

/// A pad button's own label, by DirectInput's order for an Xbox pad
/// (the south, east, west and north buttons, the shoulders, the two
/// middle buttons, the stick clicks).
fn pad_button(family: PadFamily, code: i32) -> Option<String> {
    let (brand, names): (&str, [&str; 10]) = match family {
        PadFamily::Xbox => ("Xbox", ["A", "B", "X", "Y", "LB", "RB", "View", "Menu", "LS", "RS"]),
        PadFamily::PlayStation => (
            "PS",
            ["Cross", "Circle", "Square", "Triangle", "L1", "R1", "Share", "Options", "L3", "R3"],
        ),
        PadFamily::Nintendo => ("Switch", ["B", "A", "Y", "X", "L", "R", "Minus", "Plus", "LS", "RS"]),
    };
    let name = names.get(usize::try_from(code).ok()?)?;
    Some(format!("{brand} {name}"))
}

/// A pad axis's own name: the sticks, and Z as the two triggers.
fn pad_axis(family: PadFamily, code: i32) -> Option<String> {
    let brand = match family {
        PadFamily::Xbox => "Xbox",
        PadFamily::PlayStation => "PS",
        PadFamily::Nintendo => "Switch",
    };
    let name = match code {
        -3 => "Left Stick X",
        -5 => "Left Stick Y",
        -7 => "Triggers",
        -9 => "Right Stick X",
        -11 => "Right Stick Y",
        _ => return None,
    };
    Some(format!("{brand} {name}"))
}

/// The scan code of a key.
pub fn scan_code(key: KeyCode) -> Option<u32> {
    use KeyCode::*;
    Some(match key {
        Escape => 0x01,
        Digit1 => 0x02,
        Digit2 => 0x03,
        Digit3 => 0x04,
        Digit4 => 0x05,
        Digit5 => 0x06,
        Digit6 => 0x07,
        Digit7 => 0x08,
        Digit8 => 0x09,
        Digit9 => 0x0a,
        Digit0 => 0x0b,
        Minus => 0x0c,
        Equal => 0x0d,
        Backspace => 0x0e,
        Tab => 0x0f,
        KeyQ => 0x10,
        KeyW => 0x11,
        KeyE => 0x12,
        KeyR => 0x13,
        KeyT => 0x14,
        KeyY => 0x15,
        KeyU => 0x16,
        KeyI => 0x17,
        KeyO => 0x18,
        KeyP => 0x19,
        BracketLeft => 0x1a,
        BracketRight => 0x1b,
        Enter => 0x1c,
        ControlLeft => 0x1d,
        KeyA => 0x1e,
        KeyS => 0x1f,
        KeyD => 0x20,
        KeyF => 0x21,
        KeyG => 0x22,
        KeyH => 0x23,
        KeyJ => 0x24,
        KeyK => 0x25,
        KeyL => 0x26,
        Semicolon => 0x27,
        Quote => 0x28,
        Backquote => 0x29,
        ShiftLeft => 0x2a,
        Backslash => 0x2b,
        KeyZ => 0x2c,
        KeyX => 0x2d,
        KeyC => 0x2e,
        KeyV => 0x2f,
        KeyB => 0x30,
        KeyN => 0x31,
        KeyM => 0x32,
        Comma => 0x33,
        Period => 0x34,
        Slash => 0x35,
        ShiftRight => 0x36,
        NumpadMultiply => 0x37,
        AltLeft => 0x38,
        Space => 0x39,
        CapsLock => 0x3a,
        F1 => 0x3b,
        F2 => 0x3c,
        F3 => 0x3d,
        F4 => 0x3e,
        F5 => 0x3f,
        F6 => 0x40,
        F7 => 0x41,
        F8 => 0x42,
        F9 => 0x43,
        F10 => 0x44,
        NumLock => 0x45,
        ScrollLock => 0x46,
        Numpad7 => 0x47,
        Numpad8 => 0x48,
        Numpad9 => 0x49,
        NumpadSubtract => 0x4a,
        Numpad4 => 0x4b,
        Numpad5 => 0x4c,
        Numpad6 => 0x4d,
        NumpadAdd => 0x4e,
        Numpad1 => 0x4f,
        Numpad2 => 0x50,
        Numpad3 => 0x51,
        Numpad0 => 0x52,
        NumpadDecimal => 0x53,
        F11 => 0x57,
        F12 => 0x58,
        NumpadEnter => 0x9c,
        ControlRight => 0x9d,
        NumpadDivide => 0xb5,
        AltRight => 0xb8,
        Home => 0xc7,
        ArrowUp => 0xc8,
        PageUp => 0xc9,
        ArrowLeft => 0xcb,
        ArrowRight => 0xcd,
        End => 0xcf,
        ArrowDown => 0xd0,
        PageDown => 0xd1,
        Insert => 0xd2,
        Delete => 0xd3,
        _ => return None,
    })
}

/// Windows' name of a scan code (US English).
fn key_name(code: u32) -> Option<&'static str> {
    const LOW: [&str; 0x59] = [
        "",
        "Esc",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        "0",
        "-",
        "=",
        "Backspace",
        "Tab",
        "Q",
        "W",
        "E",
        "R",
        "T",
        "Y",
        "U",
        "I",
        "O",
        "P",
        "[",
        "]",
        "Enter",
        "Ctrl",
        "A",
        "S",
        "D",
        "F",
        "G",
        "H",
        "J",
        "K",
        "L",
        ";",
        "'",
        "`",
        "Shift",
        "\\",
        "Z",
        "X",
        "C",
        "V",
        "B",
        "N",
        "M",
        ",",
        ".",
        "/",
        "Right Shift",
        "Num *",
        "Alt",
        "Space",
        "Caps Lock",
        "F1",
        "F2",
        "F3",
        "F4",
        "F5",
        "F6",
        "F7",
        "F8",
        "F9",
        "F10",
        "Pause",
        "Scroll Lock",
        "Num 7",
        "Num 8",
        "Num 9",
        "Num -",
        "Num 4",
        "Num 5",
        "Num 6",
        "Num +",
        "Num 1",
        "Num 2",
        "Num 3",
        "Num 0",
        "Num Del",
        "",
        "",
        "",
        "F11",
        "F12",
    ];
    if let Some(n) = LOW.get(code as usize).filter(|n| !n.is_empty()) {
        return Some(n);
    }
    Some(match code {
        0x9c => "Num Enter",
        0x9d => "Right Ctrl",
        0xb5 => "Num /",
        0xb8 => "Right Alt",
        0xc7 => "Home",
        0xc8 => "Up",
        0xc9 => "Page Up",
        0xcb => "Left",
        0xcd => "Right",
        0xcf => "End",
        0xd0 => "Down",
        0xd1 => "Page Down",
        0xd2 => "Insert",
        0xd3 => "Delete",
        _ => return None,
    })
}

/// An input's name as the controls page lists it (`0x44a310`): `kind` 0
/// a key, 1 the mouse, 2 a joystick; negative codes are axes. `lang`
/// gives the game's strings.
pub fn input_name(kind: i32, code: i32, lang: impl Fn(u32) -> String) -> String {
    match kind {
        0 => {
            let special = match code {
                0x1d => Some(5063),
                0x2a => Some(5061),
                0x36 => Some(5062),
                0x38 => Some(5059),
                0x9d => Some(5064),
                0xb8 => Some(5060),
                _ => None,
            };
            if let Some(id) = special {
                return lang(id);
            }
            match key_name(code.max(0) as u32) {
                Some(n) => format!("{} {n}", lang(5126)),
                None => String::new(),
            }
        }
        1 => match code {
            -3 => lang(4065),
            -5 => lang(4066),
            c if c >= 0 => format!("{} {}", lang(4067), c + 1),
            _ => String::new(),
        },
        2 if pad().and_then(|p| p.family).is_some() => {
            let family = pad().and_then(|p| p.family).unwrap_or(PadFamily::Xbox);
            let own = if code >= 0 {
                pad_button(family, code)
            } else {
                pad_axis(family, code)
            };
            own.unwrap_or_else(|| input_name_joy(code, &lang))
        }
        2 => input_name_joy(code, &lang),
        _ => String::new(),
    }
}

/// A joystick input's name as the game gives it.
fn input_name_joy(code: i32, lang: &impl Fn(u32) -> String) -> String {
    {
        match code {
            -3 => lang(4068),
            -5 => lang(4069),
            -7 => lang(4070),
            -9 => lang(4071),
            -11 => lang(4072),
            -13 => lang(4073),
            c if c >= 0 => format!("{} {}", lang(4074), c + 1),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let lang = |id: u32| match id {
            5126 => "Keyboard".to_string(),
            5063 => "Keyboard Left CTRL".to_string(),
            4068 => "Joy X Axis".to_string(),
            4074 => "Joy Button".to_string(),
            _ => String::new(),
        };
        assert_eq!(scan_code(KeyCode::KeyZ), Some(0x2c));
        assert_eq!(input_name(0, 0x2c, lang), "Keyboard Z");
        assert_eq!(input_name(0, 0x1d, lang), "Keyboard Left CTRL");
        assert_eq!(input_name(0, 0xc8, lang), "Keyboard Up");
        assert_eq!(input_name(2, -3, lang), "Joy X Axis");
        assert_eq!(input_name(2, 1, lang), "Joy Button 2");
        set_pad(Some(PadInfo {
            family: Some(PadFamily::PlayStation),
            name: "DualShock 4".into(),
            device: 3,
        }));
        assert_eq!(input_name(2, 1, lang), "PS Circle");
        assert_eq!(input_name(2, -9, lang), "PS Right Stick X");
        assert_eq!(input_name(2, 12, lang), "Joy Button 13");
        set_pad(None);
    }
}
