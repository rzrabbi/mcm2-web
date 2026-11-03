//! The original's screens off the main menu, beside the single player
//! screen in `frontend.rs`: the options screen and its pages, the user
//! profiles, the high scores and the credits; and the small message boxes
//! (`UI/MESSBOX1-3.DAT`) they all use.
//!
//! Each screen is a stack of the game's dialogs ([`Dlg`]) and reacts to
//! what they report ([`Event`]) as the dialog's handler in `Mcm2.exe`
//! does; the addresses are given with each.

pub mod credits;
pub mod intro;
pub mod keys;
pub mod multi;
pub mod options;
pub mod pro;
pub mod profiles;
pub mod records;
pub mod replays;

use crate::audio::Audio;
use crate::frontend::{RaceResults, Setup};
use crate::profile::{Controls, Profile, Settings, Store};
use crate::roster::Ride;
use crate::ui::{Dlg, Event, Gfx, Ui};
use std::path::Path;

/// The player, their profile and the machine's settings.
pub struct User {
    pub store: Store,
    pub profile: Profile,
    pub settings: Settings,
}

impl User {
    /// The last profile used (`MRUProfile`), or none yet.
    pub fn load(ui: &Ui) -> User {
        let store = Store::open();
        let settings = store.settings();
        let last = settings.get("MRUProfile").unwrap_or("").to_string();
        let profile = store
            .load(&last, default_messages(ui), Controls::preset(ui.game()))
            .unwrap_or_else(|| Profile::new("", default_messages(ui), Controls::preset(ui.game())));
        User {
            store,
            profile,
            settings,
        }
    }

    /// Saves the profile, if the player has one.
    pub fn save(&mut self) {
        if let Err(e) = self.store.save(&self.profile) {
            eprintln!("saving profile {}: {e}", self.profile.name);
        }
        self.settings = self.store.settings();
    }

    pub fn save_settings(&self) {
        if let Err(e) = self.store.save_settings(&self.settings) {
            eprintln!("saving settings: {e}");
        }
    }

    /// Switches to another profile (`0x44c0c0`).
    pub fn switch(&mut self, ui: &Ui, name: &str) -> bool {
        match self
            .store
            .load(name, default_messages(ui), Controls::preset(ui.game()))
        {
            Some(p) => {
                self.profile = p;
                true
            }
            None => false,
        }
    }
}

/// The chat messages a new profile starts with: strings 5143 to 5152,
/// the first for Control+1 (`0x527e70`).
pub fn default_messages(ui: &Ui) -> [String; 10] {
    std::array::from_fn(|i| ui.lang(5143 + ((i + 9) % 10) as u32))
}

/// What a screen's input needs.
pub struct Cx<'a> {
    pub ui: &'a mut Ui,
    /// None where no sound can start (Esc, typing).
    pub audio: Option<&'a Audio>,
    pub user: &'a mut User,
    pub now: f32,
}

impl Cx<'_> {
    pub fn game(&self) -> &Path {
        self.ui.game()
    }
}

/// What a screen asks of the front end.
pub enum Go {
    Stay,
    /// Back to the main menu.
    Back,
    Quit,
    /// Another screen in this one's place.
    Open(Box<dyn Screen>),
    /// A race, coming back to this screen after.
    Race(Box<Setup>),
    /// A tape to play, coming back to this screen after.
    Replay(std::path::PathBuf),
    /// A ghost race against a saved ghost, on its track.
    Ghost(std::path::PathBuf),
}

pub trait Screen {
    /// Its dialogs, bottom first; drawn in order.
    fn layers(&mut self) -> Vec<&mut Dlg>;

    /// Whether only the top dialog takes input (a message box is up).
    fn modal(&self) -> bool {
        false
    }

    /// What one of its dialogs reported.
    fn event(&mut self, cx: &mut Cx, event: Event) -> Go;

    fn escape(&mut self, _cx: &mut Cx) -> Go {
        Go::Back
    }

    /// A key typed, for its edit boxes.
    fn type_char(&mut self, cx: &mut Cx, c: char) -> Go {
        let events = match self.layers().pop() {
            Some(d) => d.type_char(c),
            None => Vec::new(),
        };
        let mut go = Go::Stay;
        for e in events {
            go = self.event(cx, e);
            if !matches!(go, Go::Stay) {
                break;
            }
        }
        go
    }

    /// The mouse let go anywhere, before its dialogs' events.
    fn pressed(&mut self, _cx: &mut Cx) -> Go {
        Go::Stay
    }

    /// A key pressed, as the game's scan code (for the controls page).
    fn key(&mut self, _cx: &mut Cx, _code: u32) -> Go {
        Go::Stay
    }

    /// A joystick button pressed or axis pushed, as `control.ctl` names
    /// it (kind 2, code), for the controls page.
    fn pad(&mut self, _cx: &mut Cx, _input: (i32, i32)) -> Go {
        Go::Stay
    }

    /// Whether the screen is waiting for a joystick input to bind, so the
    /// pad's buttons should not work the pointer.
    fn wants_pad(&self) -> bool {
        false
    }

    /// Each frame before drawing.
    fn tick(&mut self, _cx: &mut Cx) -> Go {
        Go::Stay
    }

    fn draw(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32) {
        for d in self.layers() {
            d.tick(now);
            d.draw(ui, gfx, now);
        }
    }

    /// Back from a race it started: its results, or none if it was left.
    fn race_over(&mut self, _cx: &mut Cx, _results: Option<&RaceResults>) {}

    /// A bike and rider to turn, whether they turn, and the window on the
    /// screen.
    fn preview(&self) -> Option<(Ride, bool, [f32; 4])> {
        None
    }

    /// The dialog whose archive sounds come from.
    fn pack(&mut self) -> String {
        self.layers()
            .pop()
            .map(|d| d.pack.clone())
            .unwrap_or_default()
    }
}

/// The dialogs a screen's input goes to, topmost last.
pub fn inputs(s: &mut dyn Screen) -> Vec<&mut Dlg> {
    let modal = s.modal();
    let mut layers = s.layers();
    if modal && layers.len() > 1 {
        layers.drain(..layers.len() - 1);
    }
    layers
}

/// Where a popup dialog sits: centred on the 640×480 screen (the game's
/// `DlgAlignH`/`DlgAlignV`, `0x46bd30`).
pub fn popup(ui: &mut Ui, pack: &str) -> Option<Dlg> {
    let dialog = ui.dialog(pack)?;
    let [_, _, w, h] = dialog.rect();
    let origin = (((640 - w) / 2) as f32, ((480 - h) / 2) as f32);
    Dlg::open(ui, pack, origin)
}

/// The message boxes (`UI/MESSBOX1.DAT` with an edit box, `MESSBOX2.DAT`
/// with up to three buttons), worded by the handler that opens them.
pub struct MsgBox {
    pub dlg: Dlg,
}

impl MsgBox {
    /// `messbox2`: a title, a prompt and buttons left, middle and right
    /// (string ids; 0 hides one). Clicks report the button's name.
    pub fn ask(ui: &mut Ui, title: &str, prompt: &str, buttons: [u32; 3]) -> Option<MsgBox> {
        let mut dlg = popup(ui, "MESSBOX2")?;
        dlg.set_text("TitleText", title);
        dlg.set_text("TxtPrompt", prompt);
        for (name, id) in ["ButLeft", "ButMiddle", "ButRight"].iter().zip(buttons) {
            if id == 0 {
                dlg.show(name, false);
            } else {
                dlg.set_text(name, ui.lang(id));
            }
        }
        Some(MsgBox { dlg })
    }

    /// One button, the middle (`0x44de40`).
    pub fn notice(ui: &mut Ui, title: u32, prompt: u32) -> Option<MsgBox> {
        let (t, p) = (ui.lang(title), ui.lang(prompt));
        Self::ask(ui, &t, &p, [0, 5097, 0])
    }
}
