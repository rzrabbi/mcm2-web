//! The saved replays and ghost races (`UI/GRBASE.DAT`, `0x454c10`): two
//! tabs, Saved Ghost Race Files (`GhostFil`, `0x455030`) and Saved Replay
//! Files (`ReplyFil`, `0x455a50`), each listing the game's files in
//! `Record\` (`*.gho`, `*.vcr`) by track, rider, laps, length and
//! description. Start plays the file picked (a ghost as a ghost race on
//! its track), and shows only when there is one.
//!
//! Description... renames the file's line in the list: the description
//! is kept in the tape's header (31 letters), asked for in a message box
//! (lang 5183).
//!
//! The files are kept in `Record` in the user's folder (the game kept
//! them in its own).

use super::{popup, Cx, Go, Screen};
use crate::ui::{Dlg, Event};
use mm2_formats::vcr::{self, Tape};
use std::path::PathBuf;

/// Where the tabs' pages go (`0x454e90`).
const PAGE: (f32, f32) = (20.0, 35.0);

/// The tabs, their pages, and their names (strings 5178, 5177).
const TABS: [(&str, &str, u32); 2] = [
    ("TabLeft", "GHOSTFIL", 5178),
    ("TabRight", "REPLYFIL", 5177),
];

const LISTS: [&str; 5] = ["LstTrack", "LstRider", "LstLaps", "LstLength", "LstDesc"];

pub struct Replays {
    base: Dlg,
    page: Dlg,
    tab: usize,
    /// The files listed, in the lists' order.
    files: Vec<PathBuf>,
    picked: Option<usize>,
    /// The description box, while it is up.
    describe: Option<Dlg>,
}

/// The tapes of a tab in `Record`: ghosts (`*.gho`) or replays (`*.vcr`),
/// by name, each with its header.
fn tapes(cx: &Cx, tab: usize) -> Vec<(PathBuf, Tape)> {
    let ext = if tab == 0 { "gho" } else { "vcr" };
    let dir = crate::replay::record_dir(&cx.user.store.dir);
    let mut paths: Vec<PathBuf> = crate::platform::fs::read_dir(&dir)
        .map(|d| {
            d.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)))
                .collect()
        })
        .unwrap_or_default();
    paths.sort_by_key(|p| p.file_name().map(|n| n.to_ascii_uppercase()));
    paths
        .into_iter()
        .filter_map(|p| {
            let tape = Tape::parse(&crate::platform::fs::read(&p).ok()?).ok()?;
            Some((p, tape))
        })
        .collect()
}

impl Replays {
    /// Opens on the replays (`0x454c10` open).
    pub fn open(cx: &mut Cx) -> Option<Replays> {
        let mut base = Dlg::open(cx.ui, "GRBASE", (0.0, 0.0))?;
        for (tab, _, id) in TABS {
            base.set_text(tab, cx.ui.lang(id));
        }
        let page = Dlg::open(cx.ui, TABS[1].1, PAGE)?;
        let mut r = Replays {
            base,
            page,
            tab: 1,
            files: Vec::new(),
            picked: None,
            describe: None,
        };
        r.show(cx, 1);
        Some(r)
    }

    fn show(&mut self, cx: &mut Cx, tab: usize) {
        let Some(mut page) = Dlg::open(cx.ui, TABS[tab].1, PAGE) else {
            return;
        };
        for (i, (name, _, _)) in TABS.iter().enumerate() {
            self.base.set_on(name, i == tab);
        }
        let tapes = tapes(cx, tab);
        let column = |f: &dyn Fn(&Tape) -> String| tapes.iter().map(|(_, t)| f(t)).collect();
        page.set_items("LstTrack", column(&|t| t.track()), 0);
        page.set_items(
            "LstRider",
            column(&|t| t.riders.first().map(|r| r.name.clone()).unwrap_or_default()),
            0,
        );
        page.set_items(
            "LstLaps",
            column(&|t| t.setting(vcr::setting::LAPS).to_string()),
            0,
        );
        page.set_items(
            "LstLength",
            column(&|t| crate::hud::clock_text(t.length).trim().to_string()),
            0,
        );
        page.set_items("LstDesc", column(&|t| t.description.clone()), 0);
        for list in LISTS {
            page.set_selectable(list, true);
            // The original shows the picked tape in pink, on no box.
            page.set_select_color(list, 0xff89ff);
            page.mark(list, None);
        }
        self.files = tapes.into_iter().map(|(p, _)| p).collect();
        self.picked = None;
        self.tab = tab;
        // Ghost laps have no lap count (`0x455030`).
        page.show("LstLaps", tab == 1);
        page.show("TxtLaps", tab == 1);
        page.enable("ButDescription", false);
        page.enable("ButDelete", false);
        self.base.show("Start", false);
        self.page = page;
    }
}

impl Replays {
    /// The description box for the file picked, empty.
    fn describe_box(cx: &mut Cx) -> Option<Dlg> {
        let mut d = popup(cx.ui, "MESSBOX1")?;
        d.set_text("TitleText", "");
        d.set_text("OKButton", cx.ui.lang(5097));
        d.set_text("CancelButton", cx.ui.lang(5084));
        d.set_text("TxtPrompt", cx.ui.lang(5183));
        if let Some(c) = d.ctl("EditBox") {
            c.max_len = 31;
            c.text = Some(String::new());
        }
        d.edit(Some("EditBox"));
        Some(d)
    }

    /// Writes the description into the picked file's header.
    fn redescribe(&mut self, text: &str) {
        let Some(p) = self.picked.and_then(|i| self.files.get(i)) else {
            return;
        };
        let written = crate::platform::fs::read(p)
            .ok()
            .and_then(|b| Tape::parse(&b).ok())
            .map(|mut t| {
                t.description = text.chars().take(31).collect();
                crate::platform::fs::write(p, t.write())
            });
        if let Some(Err(e)) = written {
            eprintln!("{}: {e}", p.display());
        }
    }

    fn describe_event(&mut self, cx: &mut Cx, e: Event) -> Go {
        let Some(d) = &mut self.describe else {
            return Go::Stay;
        };
        let clicked = |n: &str| matches!(&e, Event::Clicked(c) if c.eq_ignore_ascii_case(n));
        let done = clicked("OKButton")
            || matches!(&e, Event::Released(n) if n.eq_ignore_ascii_case("EditBox"));
        if done {
            let text = d
                .get("EditBox")
                .and_then(|c| c.text.clone())
                .unwrap_or_default();
            self.describe = None;
            let line = self.picked;
            self.redescribe(text.trim());
            self.show(cx, self.tab);
            if let Some(l) = line.filter(|&l| l < self.files.len()) {
                for list in LISTS {
                    self.page.mark(list, Some(l));
                }
                self.picked = Some(l);
                self.page.enable("ButDescription", true);
                self.page.enable("ButDelete", true);
                self.base.show("Start", true);
            }
        } else if clicked("CancelButton") {
            self.describe = None;
        } else if clicked("EditBox") {
            d.edit(Some("EditBox"));
        }
        Go::Stay
    }
}

impl Screen for Replays {
    fn layers(&mut self) -> Vec<&mut Dlg> {
        match &mut self.describe {
            Some(d) => vec![&mut self.base, &mut self.page, d],
            None => vec![&mut self.base, &mut self.page],
        }
    }

    fn modal(&self) -> bool {
        self.describe.is_some()
    }

    fn escape(&mut self, _cx: &mut Cx) -> Go {
        if self.describe.take().is_some() {
            return Go::Stay;
        }
        Go::Back
    }

    fn event(&mut self, cx: &mut Cx, e: Event) -> Go {
        if self.describe.is_some() {
            return self.describe_event(cx, e);
        }
        if let Event::Selected(n, line) = &e {
            // A line picked in one list picks it in them all.
            if LISTS.iter().any(|l| l.eq_ignore_ascii_case(n)) && *line < self.files.len() {
                for list in LISTS {
                    self.page.mark(list, Some(*line));
                }
                self.picked = Some(*line);
                self.page.enable("ButDelete", true);
                self.page.enable("ButDescription", true);
                self.base.show("Start", true);
            }
            return Go::Stay;
        }
        let Event::Clicked(n) = e else {
            return Go::Stay;
        };
        if n.eq_ignore_ascii_case("Back") {
            return Go::Back;
        }
        let picked = self.picked.and_then(|i| self.files.get(i)).cloned();
        if n.eq_ignore_ascii_case("Start") {
            if let Some(p) = picked {
                return if self.tab == 0 {
                    Go::Ghost(p)
                } else {
                    Go::Replay(p)
                };
            }
            return Go::Stay;
        }
        if n.eq_ignore_ascii_case("ButDescription") {
            self.describe = Self::describe_box(cx);
            return Go::Stay;
        }
        if n.eq_ignore_ascii_case("ButDelete") {
            if let Some(p) = picked {
                if let Err(err) = crate::platform::fs::remove_file(&p) {
                    eprintln!("{}: {err}", p.display());
                }
                self.show(cx, self.tab);
            }
            return Go::Stay;
        }
        if let Some(tab) = TABS.iter().position(|t| t.0.eq_ignore_ascii_case(&n)) {
            self.show(cx, tab);
        }
        Go::Stay
    }
}
