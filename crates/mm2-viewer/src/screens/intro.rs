//! The screens before the main menu (`Intro1Dlg` `0x499850`, `Intro2Dlg`
//! `0x454240`, `Intro3Dlg` `0x4543b0`): the publisher's and the
//! developer's logos and the legal notice, each for a few seconds. A
//! click or a key skips them all (`0x4543a0`, `0x454570`). Then the intro
//! movie (`IntroVid1Dlg`, `0x4540d0`): `UI/INTRO.AVI` over the whole
//! screen, and the main menu when it ends or is skipped (`0x44c1c0`).

use super::{Cx, Go, Screen};
use crate::movie::Movie;
use crate::ui::{Dlg, Event, Gfx, Ui};

const PAGES: [&str; 3] = ["INTRO1", "INTRO2", "INTRO3"];

/// How long each shows.
const SECONDS: f32 = 3.0;

pub struct Intro {
    page: usize,
    dlg: Dlg,
    since: Option<f32>,
    movie: Option<Movie>,
}

impl Intro {
    pub fn open(ui: &mut Ui) -> Option<Intro> {
        Some(Intro {
            page: 0,
            dlg: Dlg::open(ui, PAGES[0], (0.0, 0.0))?,
            since: None,
            movie: None,
        })
    }
}

impl Screen for Intro {
    fn layers(&mut self) -> Vec<&mut Dlg> {
        vec![&mut self.dlg]
    }

    fn event(&mut self, _cx: &mut Cx, _e: Event) -> Go {
        Go::Stay
    }

    fn escape(&mut self, _cx: &mut Cx) -> Go {
        Go::Back
    }

    fn pressed(&mut self, _cx: &mut Cx) -> Go {
        Go::Back
    }

    fn key(&mut self, _cx: &mut Cx, _code: u32) -> Go {
        Go::Back
    }

    fn tick(&mut self, cx: &mut Cx) -> Go {
        if let Some(m) = &self.movie {
            return if m.done(cx.now) { Go::Back } else { Go::Stay };
        }
        let since = *self.since.get_or_insert(cx.now);
        if cx.now - since < SECONDS {
            return Go::Stay;
        }
        self.page += 1;
        let Some(dlg) = PAGES
            .get(self.page)
            .and_then(|p| Dlg::open(cx.ui, p, (0.0, 0.0)))
        else {
            let path = cx.game().join("UI/INTRO.AVI");
            let Some(mut movie) = Movie::open(&path, false) else {
                return Go::Back;
            };
            movie.play(cx.ui.audio.as_ref(), cx.now);
            self.movie = Some(movie);
            return Go::Stay;
        };
        self.dlg = dlg;
        self.since = Some(cx.now);
        Go::Stay
    }

    fn draw(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32) {
        match &mut self.movie {
            // The game shows it stretched to the screen (`0x4540d0`).
            Some(m) => m.draw(gfx, now, [0.0, 0.0, 640.0, 480.0]),
            None => self.dlg.draw(ui, gfx, now),
        }
    }
}
