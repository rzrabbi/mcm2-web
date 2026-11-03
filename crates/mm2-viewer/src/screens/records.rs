//! Track records and the High Scores screen (`UI/HISCORES.DAT`,
//! `0x524c50`).
//!
//! The game keeps ten records per track for each of three tables: the
//! fastest lap and the best five and ten lap races (for the Stunt Quarry
//! the best single stunt and five and ten minute sessions, for Moto-Tag
//! the time spent "it"), each a name and a time or score
//! (`TrackRecords`, `0x524630`: ten 20 byte entries of a 16 letter name
//! and a float). Here each track's tables are a small text file in the
//! user's data folder, `record/<event>/<track>.txt`, which [`add`]
//! updates after a race.

use super::{Cx, Go, Screen};
use crate::ui::{Dlg, Event};
use std::path::{Path, PathBuf};

/// Records per table (`0x524630`).
pub const KEEP: usize = 10;

/// The event tabs, as the game numbers the events, with their text
/// (`0x524c50` open, strings 5153 to 5158).
const TABS: [(&str, u32, u32); 6] = [
    ("TabQuarry", 0, 5155),
    ("TabBaja", 1, 5153),
    ("TabNationals", 2, 5157),
    ("TabSupercross", 3, 5156),
    ("TabTag", 4, 5158),
    ("TabEnduro", 5, 5154),
];

const TABLES: [&str; 3] = ["TabLeft", "TabMiddle", "TabRight"];

/// One record: who, and their time in seconds or their score.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub name: String,
    pub value: f32,
}

/// One track's records.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackRecords {
    /// The track's name as the lists show it.
    pub title: String,
    pub tables: [Vec<Record>; 3],
}

impl TrackRecords {
    pub fn parse(text: &str) -> TrackRecords {
        let mut r = TrackRecords::default();
        let mut table = None;
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(t) = line.strip_prefix("title=") {
                r.title = t.to_string();
            } else if let Some(n) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                table = n.parse::<usize>().ok().filter(|&n| n < 3);
            } else if let (Some(t), Some((name, value))) = (table, line.rsplit_once('\t')) {
                if let Ok(value) = value.trim().parse() {
                    r.tables[t].push(Record {
                        name: name.to_string(),
                        value,
                    });
                }
            }
        }
        r
    }

    pub fn text(&self) -> String {
        let mut out = format!("title={}\n", self.title);
        for (i, t) in self.tables.iter().enumerate() {
            out += &format!("[{i}]\n");
            for r in t {
                out += &format!("{}\t{}\n", r.name, r.value);
            }
        }
        out
    }
}

/// Whether a bigger value is the better one: scores, and Moto-Tag's
/// time spent "it".
pub fn higher_wins(event: u32) -> bool {
    matches!(event, 0 | 4)
}

fn folder(user: &Path, event: u32) -> PathBuf {
    user.join("record").join(event.to_string())
}

/// A file name for a track (and course, for Baja and Enduro).
fn key(track: &Path, course: usize) -> String {
    let stem = track
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{stem}_{course}.txt")
}

/// Every track of an event that has records, in name order.
pub fn tracks(user: &Path, event: u32) -> Vec<TrackRecords> {
    let mut v: Vec<TrackRecords> = crate::platform::fs::read_dir(folder(user, event))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter_map(|e| crate::platform::fs::read_to_string(e.path()).ok())
                .map(|t| TrackRecords::parse(&t))
                .filter(|r| r.tables.iter().any(|t| !t.is_empty()))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|r| r.title.to_ascii_lowercase());
    v
}

/// Enters a result in a table if it makes the top ten; its place from 0
/// if it did. `table` is 0 for a lap or single stunt, 1 and 2 for the
/// five and ten lap (or minute) events.
pub fn add(
    user: &Path,
    event: u32,
    track: &Path,
    course: usize,
    title: &str,
    table: usize,
    record: Record,
) -> std::io::Result<Option<usize>> {
    let dir = folder(user, event);
    let path = dir.join(key(track, course));
    let mut r = crate::platform::fs::read_to_string(&path)
        .map(|t| TrackRecords::parse(&t))
        .unwrap_or_default();
    r.title = title.to_string();
    let t = &mut r.tables[table.min(2)];
    let better = |a: f32, b: f32| if higher_wins(event) { a > b } else { a < b };
    let at = t
        .iter()
        .position(|o| better(record.value, o.value))
        .unwrap_or(t.len());
    if at >= KEEP {
        return Ok(None);
    }
    t.insert(at, record);
    t.truncate(KEEP);
    crate::platform::fs::create_dir_all(&dir)?;
    crate::platform::fs::write(path, r.text())?;
    Ok(Some(at))
}

/// A time as the records show it: minutes and hundredths
/// (`0x51dd10`, "%2d:%05.2f").
pub fn time(t: f32) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u32;
    format!("{}:{:02}.{:02}", cs / 6000, cs / 100 % 60, cs % 100)
}

pub struct HighScores {
    dlg: Dlg,
    event: u32,
    table: usize,
    tracks: Vec<TrackRecords>,
}

impl HighScores {
    pub fn open(cx: &mut Cx) -> Option<HighScores> {
        let mut dlg = Dlg::open(cx.ui, "HISCORES", (0.0, 0.0))?;
        dlg.set_text("TxtTitle", cx.ui.lang(2392));
        for (tab, _, id) in TABS {
            dlg.set_text(tab, cx.ui.lang(id));
        }
        dlg.set_selectable("LstTrack", true);
        let mut h = HighScores {
            dlg,
            event: 0,
            table: 0,
            tracks: Vec::new(),
        };
        h.show(cx);
        Some(h)
    }

    /// The event's tracks and its tables' names (`0x5259e0`, `0x525290`).
    fn show(&mut self, cx: &mut Cx) {
        let d = &mut self.dlg;
        for (tab, event, _) in TABS {
            d.set_on(tab, event == self.event);
        }
        let points = matches!(self.event, 0 | 4);
        let names = if points {
            let left = if self.event == 4 { 5163 } else { 5162 };
            [cx.ui.lang(left), cx.ui.lang(5168), cx.ui.lang(5168)]
        } else {
            [cx.ui.lang(5159), cx.ui.lang(5167), cx.ui.lang(5167)]
        };
        for (i, (tab, name)) in TABLES.iter().zip(names).enumerate() {
            let n = [0, 5, 10][i];
            d.set_text(tab, name.replace("%d", &n.to_string()));
            d.set_on(tab, i == self.table);
        }
        d.set_text("TxtTrack", cx.ui.lang(3014));
        d.set_text("TxtStats1", "");
        d.set_text("TxtStats2", "");
        d.set_items("LstStats1", Vec::new(), 0);
        d.set_items("LstStats2", Vec::new(), 0);
        self.tracks = tracks(&cx.user.store.dir, self.event);
        if self.tracks.is_empty() {
            d.set_items("LstTrack", vec![cx.ui.lang(5166)], 0);
            d.enable("LstTrack", false);
            d.mark("LstTrack", None);
        } else {
            let titles = self.tracks.iter().map(|t| t.title.clone()).collect();
            d.set_items("LstTrack", titles, 0);
            d.enable("LstTrack", true);
            d.mark("LstTrack", None);
        }
    }

    /// A track's table (`0x525b30`): names, and times or scores.
    fn show_track(&mut self, cx: &mut Cx, line: usize) {
        let Some(t) = self.tracks.get(line) else {
            return;
        };
        let d = &mut self.dlg;
        d.set_text("TxtStats1", cx.ui.lang(3012));
        let second = if self.event == 0 { 2360 } else { 3013 };
        d.set_text("TxtStats2", cx.ui.lang(second));
        let rows = &t.tables[self.table];
        let names = rows.iter().map(|r| r.name.clone()).collect();
        let values = rows
            .iter()
            .map(|r| {
                if self.event == 0 {
                    format!("{}", r.value as i64)
                } else {
                    time(r.value)
                }
            })
            .collect();
        d.set_items("LstStats1", names, 0);
        d.set_items("LstStats2", values, 0);
    }
}

impl Screen for HighScores {
    fn layers(&mut self) -> Vec<&mut Dlg> {
        vec![&mut self.dlg]
    }

    fn event(&mut self, cx: &mut Cx, e: Event) -> Go {
        match e {
            Event::Clicked(n) => {
                if n.eq_ignore_ascii_case("Back") {
                    return Go::Back;
                }
                if let Some((_, event, _)) = TABS.iter().find(|t| t.0.eq_ignore_ascii_case(&n)) {
                    self.event = *event;
                    self.show(cx);
                } else if let Some(i) = TABLES.iter().position(|t| t.eq_ignore_ascii_case(&n)) {
                    let line = self.dlg.marked("LstTrack");
                    self.table = i;
                    self.show(cx);
                    if let Some(l) = line {
                        self.dlg.mark("LstTrack", Some(l));
                        self.show_track(cx, l);
                    }
                }
            }
            Event::Selected(n, line) if n.eq_ignore_ascii_case("LstTrack") => {
                self.show_track(cx, line)
            }
            _ => {}
        }
        Go::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables() {
        let dir = std::env::temp_dir().join(format!("mm2-records-{}", std::process::id()));
        let track = Path::new("TERAFORM/SX/SX01.env");
        let rec = |n: &str, v| Record {
            name: n.into(),
            value: v,
        };
        assert_eq!(
            add(&dir, 3, track, 0, "Arena", 0, rec("A", 40.0)).unwrap(),
            Some(0)
        );
        assert_eq!(
            add(&dir, 3, track, 0, "Arena", 0, rec("B", 35.0)).unwrap(),
            Some(0)
        );
        assert_eq!(
            add(&dir, 3, track, 0, "Arena", 0, rec("C", 50.0)).unwrap(),
            Some(2)
        );
        for i in 0..8 {
            add(
                &dir,
                3,
                track,
                0,
                "Arena",
                0,
                rec("D", 45.0 + i as f32 * 0.1),
            )
            .unwrap();
        }
        assert_eq!(
            add(&dir, 3, track, 0, "Arena", 0, rec("E", 60.0)).unwrap(),
            None
        );
        let t = tracks(&dir, 3);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].tables[0].len(), KEEP);
        assert_eq!(t[0].tables[0][0].name, "B");
        // Scores count the other way.
        add(&dir, 0, track, 0, "Quarry", 0, rec("A", 100.0)).unwrap();
        assert_eq!(
            add(&dir, 0, track, 0, "Quarry", 0, rec("B", 900.0)).unwrap(),
            Some(0)
        );
        crate::platform::fs::remove_dir_all(&dir).ok();
        assert_eq!(time(83.456), "1:23.46");
    }
}
