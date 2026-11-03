//! The main menu's player profiles (`0x44c1d0`): the Profiles pull-down's
//! list and its Load, New and Remove buttons, the name box a new profile
//! asks for (`UI/MESSBOX1.DAT`, `0x452e10`) and the questions after
//! (`MESSBOX2.DAT`).

use super::{popup, Cx, MsgBox};
use crate::profile::{valid_name, Controls, Profile, NAME_LENGTH, NAME_SYMBOLS};
use crate::ui::{Dlg, Event};

enum Open {
    /// The name box; `forced` when there is no profile yet, and no
    /// Cancel (`0xbba`).
    Name {
        dlg: Dlg,
        forced: bool,
    },
    /// The name is taken: load that one or type another.
    Exists {
        name: String,
        b: MsgBox,
        forced: bool,
    },
    /// Removing a profile, once asked.
    Remove {
        name: String,
        b: MsgBox,
    },
    Notice(MsgBox),
}

/// The boxes the profile buttons open over the main menu.
#[derive(Default)]
pub struct Profiles {
    open: Option<Open>,
}

/// The pull-down's list of profiles, the current one picked, and the
/// line under the menu (`0x44d7d0`, `0x44d8f0`).
pub fn fill(cx: &mut Cx, main: &mut Dlg) {
    let names = cx.user.store.list();
    let at = names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(&cx.user.profile.name));
    main.set_items("LstProfiles", names, at.unwrap_or(0));
    main.set_selectable("LstProfiles", true);
    main.mark("LstProfiles", at);
    let name = &cx.user.profile.name;
    let line = if name.is_empty() {
        String::new()
    } else {
        format!("{} {name}", cx.ui.lang(5222))
    };
    main.set_text("TxtCurrentProfile", line);
}

impl Profiles {
    /// Opens the name box at once if there is no profile to play with.
    pub fn start(&mut self, cx: &mut Cx) {
        if cx.user.profile.name.is_empty() || cx.user.store.list().is_empty() {
            self.ask_name(cx, true);
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Whether the name box is up: the original darkens the menu under
    /// it, as it does not under its other boxes.
    pub fn dims(&self) -> bool {
        matches!(self.open, Some(Open::Name { .. }))
    }

    pub fn dialog(&mut self) -> Option<&mut Dlg> {
        Some(match self.open.as_mut()? {
            Open::Name { dlg, .. } => dlg,
            Open::Exists { b, .. } | Open::Remove { b, .. } | Open::Notice(b) => &mut b.dlg,
        })
    }

    fn ask_name(&mut self, cx: &mut Cx, forced: bool) {
        let Some(mut d) = popup(cx.ui, "MESSBOX1") else {
            return;
        };
        d.set_text("TitleText", cx.ui.lang(2342));
        d.set_text("OKButton", cx.ui.lang(5097));
        d.set_text("CancelButton", cx.ui.lang(5084));
        d.show("CancelButton", !forced);
        d.set_text("TxtPrompt", cx.ui.lang(5038));
        d.set_wrap("TxtPrompt", true);
        if let Some(c) = d.ctl("EditBox") {
            c.max_len = NAME_LENGTH;
            c.allowed = NAME_SYMBOLS.to_string();
        }
        d.set_text("EditBox", "");
        d.enable("OKButton", false);
        d.edit(Some("EditBox"));
        self.open = Some(Open::Name { dlg: d, forced });
    }

    /// One of the pull-down's buttons; false if it is not one of them.
    pub fn clicked(&mut self, cx: &mut Cx, main: &mut Dlg, name: &str) -> bool {
        match name {
            "butloadprofile" => {
                // The current one is saved before the other is loaded.
                let pick = main.marked("LstProfiles").and_then(|m| {
                    main.get("LstProfiles")
                        .and_then(|c| c.items.get(m).cloned())
                });
                if let Some(p) = pick {
                    if !p.eq_ignore_ascii_case(&cx.user.profile.name) {
                        cx.user.save();
                        if cx.user.switch(cx.ui, &p) {
                            cx.user.save();
                            cx.ui.volume = cx.user.profile.sound.ui_gain();
                        }
                    }
                }
                fill(cx, main);
            }
            "butnewprofile" => self.ask_name(cx, false),
            "butremoveprofile" => {
                let pick = main.marked("LstProfiles").and_then(|m| {
                    main.get("LstProfiles")
                        .and_then(|c| c.items.get(m).cloned())
                });
                let Some(p) = pick else { return true };
                if p.eq_ignore_ascii_case(&cx.user.profile.name) {
                    if let Some(b) = MsgBox::notice(cx.ui, 5229, 5230) {
                        self.open = Some(Open::Notice(b));
                    }
                } else {
                    let title = cx.ui.lang(4079);
                    if let Some(b) =
                        MsgBox::ask(cx.ui, &title, &format!("\"{p}\""), [5097, 0, 5084])
                    {
                        self.open = Some(Open::Remove { name: p, b });
                    }
                }
            }
            _ => return false,
        }
        true
    }

    /// What the open box reported.
    pub fn event(&mut self, cx: &mut Cx, main: &mut Dlg, e: Event) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        match open {
            Open::Name { dlg, forced } => {
                let forced = *forced;
                let text = dlg
                    .get("EditBox")
                    .and_then(|c| c.text.clone())
                    .unwrap_or_default();
                match e {
                    // The OK button waits for a name (`0x452e10` 0x13).
                    Event::Changed(n) if n.eq_ignore_ascii_case("EditBox") => {
                        dlg.enable("OKButton", !text.trim().is_empty());
                    }
                    Event::Released(n) if n.eq_ignore_ascii_case("EditBox") => {
                        if !text.trim().is_empty() {
                            self.create(cx, main, text.trim(), forced);
                        }
                    }
                    Event::Clicked(n) if n.eq_ignore_ascii_case("OKButton") => {
                        self.create(cx, main, text.trim(), forced);
                    }
                    Event::Clicked(n) if n.eq_ignore_ascii_case("CancelButton") => {
                        self.open = None;
                    }
                    Event::Clicked(n) if n.eq_ignore_ascii_case("EditBox") => {
                        dlg.edit(Some("EditBox"));
                    }
                    _ => {}
                }
            }
            Open::Exists { name, forced, .. } => {
                let (name, forced) = (name.clone(), *forced);
                match e {
                    Event::Clicked(n) if n.eq_ignore_ascii_case("ButLeft") => {
                        self.open = None;
                        cx.user.save();
                        if cx.user.switch(cx.ui, &name) {
                            cx.user.save();
                            cx.ui.volume = cx.user.profile.sound.ui_gain();
                        }
                        fill(cx, main);
                    }
                    Event::Clicked(n) if n.eq_ignore_ascii_case("ButRight") => {
                        self.ask_name(cx, forced)
                    }
                    _ => {}
                }
            }
            Open::Remove { name, .. } => match e {
                Event::Clicked(n) if n.eq_ignore_ascii_case("ButLeft") => {
                    if let Err(err) = cx.user.store.remove(name) {
                        eprintln!("removing profile {name}: {err}");
                    }
                    self.open = None;
                    fill(cx, main);
                }
                Event::Clicked(n) if n.eq_ignore_ascii_case("ButRight") => self.open = None,
                _ => {}
            },
            Open::Notice(_) => {
                if matches!(e, Event::Clicked(_)) {
                    self.open = None;
                }
            }
        }
    }

    /// A name typed: a new profile, or the question if it is taken.
    fn create(&mut self, cx: &mut Cx, main: &mut Dlg, name: &str, forced: bool) {
        if !valid_name(name) {
            return;
        }
        if cx.user.store.exists(name) {
            let title = format!("'{name}' {}", cx.ui.lang(5071));
            let prompt = cx.ui.lang(5123);
            if let Some(mut b) = MsgBox::ask(cx.ui, &title, &prompt, [5100, 0, 5099]) {
                b.dlg.set_wrap("TxtPrompt", true);
                self.open = Some(Open::Exists {
                    name: name.to_string(),
                    b,
                    forced,
                });
            }
            return;
        }
        if !cx.user.profile.name.is_empty() {
            cx.user.save();
        }
        let p = Profile::new(
            name,
            super::default_messages(cx.ui),
            Controls::preset(cx.game()),
        );
        match cx.user.store.create(&p) {
            Ok(_) => {
                cx.user.profile = p;
                cx.user.save();
                cx.ui.volume = cx.user.profile.sound.ui_gain();
            }
            Err(e) => eprintln!("creating profile {name}: {e}"),
        }
        self.open = None;
        fill(cx, main);
    }

    /// Esc closes a box, except the one asking for the first name.
    pub fn escape(&mut self) {
        if !matches!(self.open, Some(Open::Name { forced: true, .. })) {
            self.open = None;
        }
    }
}
