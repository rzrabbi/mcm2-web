//! The credits (`CreditsVidDlg`, `credits.dtm`, `0x453f80`): `UI/CREDITS.AVI`
//! over the whole screen with the Back arrow on top; the main menu again
//! when it ends (`0x44c1c0`) or Back is clicked.

use super::{Cx, Go, Screen};
use crate::movie::Movie;
use crate::ui::{Dlg, Event, Gfx, Ui};

pub struct Credits {
    dlg: Dlg,
    movie: Movie,
    started: bool,
}

impl Credits {
    pub fn open(cx: &mut Cx) -> Option<Credits> {
        let dlg = Dlg::open(cx.ui, "CREDITS", (0.0, 0.0))?;
        let movie = Movie::open(&cx.game().join("UI/CREDITS.AVI"), false)?;
        Some(Credits {
            dlg,
            movie,
            started: false,
        })
    }
}

impl Screen for Credits {
    fn layers(&mut self) -> Vec<&mut Dlg> {
        vec![&mut self.dlg]
    }

    fn event(&mut self, _cx: &mut Cx, e: Event) -> Go {
        match e {
            Event::Clicked(n) if n.eq_ignore_ascii_case("Back") => Go::Back,
            _ => Go::Stay,
        }
    }

    fn tick(&mut self, cx: &mut Cx) -> Go {
        if !self.started {
            self.started = true;
            self.movie.play(cx.ui.audio.as_ref(), cx.now);
        }
        if self.movie.done(cx.now) {
            return Go::Back;
        }
        Go::Stay
    }

    fn draw(&mut self, ui: &mut Ui, gfx: &mut Gfx, now: f32) {
        self.movie.draw(gfx, now, [0.0, 0.0, 640.0, 480.0]);
        self.dlg.tick(now);
        self.dlg.draw(ui, gfx, now);
    }
}
