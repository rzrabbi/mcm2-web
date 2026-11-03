//! Trackside dressing for the enhanced drawing ([`crate::graphics`]),
//! none of it the game's placing: the game's own props (hay bales or oil
//! drums, banners, camera towers, trucks, chairs and tables) set along an
//! outdoor course and parked in a paddock by the start. Only drawn: nothing
//! here is solid, so it all stands well clear of the course.

use glam::{Vec2, Vec3};
use mm2_formats::scn::{Lighting, Placement};
use mm2_formats::tdf::TrackDef;
use mm2_formats::trn::Terrain;
use std::collections::HashMap;

/// Course length between two trackside spots, ft.
const SPACING: f32 = 60.0;
/// Side of the cells the taken ground is kept in, ft.
const CELL: f32 = 16.0;

struct Ground<'a> {
    terrain: &'a Terrain,
    /// The course: each point, its half width, and the point it runs on to.
    course: Vec<(Vec2, f32, Option<usize>)>,
    /// Ground already taken: where, and how far round it.
    taken: HashMap<(i32, i32), Vec<(Vec2, f32)>>,
    random: u32,
}

impl Ground<'_> {
    fn random(&mut self) -> f32 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        (self.random >> 8) as f32 / (1u32 << 24) as f32
    }

    fn height(&self, p: Vec2) -> f32 {
        crate::ride::ground(self.terrain, p.x, p.y)
    }

    /// How far `p` is from the course's edge (less than 0 on it).
    fn off_course(&self, p: Vec2) -> f32 {
        let mut least = f32::MAX;
        for &(a, half, next) in &self.course {
            let (b, half_b) = next.map_or((a, half), |n| (self.course[n].0, self.course[n].1));
            let ab = b - a;
            let t = if ab.length_squared() > 1e-6 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
            let d = p.distance(a + ab * t) - (half + (half_b - half) * t);
            least = least.min(d);
        }
        least
    }

    /// Whether ground of radius `r` at `p` is free, no steeper than
    /// `steepest`, and at least `clear` ft off the course.
    fn fits(&self, p: Vec2, r: f32, clear: f32, steepest: f32) -> bool {
        let side = self.terrain.scale * (self.terrain.size - 1) as f32;
        if p.min_element() < r || p.max_element() > side - r {
            return false;
        }
        let h = [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y].map(|d| self.height(p + d * r));
        let steep = (h[0] - h[1]).abs().max((h[2] - h[3]).abs()) / (2.0 * r);
        if steep > steepest || self.off_course(p) < clear + r {
            return false;
        }
        let (cx, cz) = ((p.x / CELL) as i32, (p.y / CELL) as i32);
        let reach = (r / CELL).ceil() as i32 + 1;
        for x in cx - reach..=cx + reach {
            for z in cz - reach..=cz + reach {
                if self.taken.get(&(x, z)).is_some_and(|v| v.iter().any(|&(q, s)| q.distance(p) < r + s)) {
                    return false;
                }
            }
        }
        true
    }

    fn take(&mut self, p: Vec2, r: f32) {
        self.taken.entry(((p.x / CELL) as i32, (p.y / CELL) as i32)).or_default().push((p, r));
    }

    /// Prop `model` at `p`, facing `look` (level), its base `lift` ft below
    /// its origin.
    fn prop(&mut self, model: &str, p: Vec2, look: Vec2, scale: f32, lift: f32, r: f32) -> Placement {
        self.take(p, r);
        let look = look.normalize_or(Vec2::Y);
        // Set into the slope, not standing off it.
        let low = [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y].iter().fold(self.height(p), |h, d| h.min(self.height(p + *d * r * 0.7)));
        Placement {
            model: model.into(),
            position: [p.x, low + lift * scale, p.y],
            look: [look.x, 0.0, look.y],
            up: [0.0, 1.0, 0.0],
            scale: [scale; 3],
            lighting: Lighting::default(),
        }
    }
}

fn flat(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}

/// The dressing for a track: its course (or else its gates, in order),
/// where the start is and which way it faces, and what already stands
/// there. Desert tracks get oil drums and pallets in place of hay.
pub fn place(
    terrain: &Terrain,
    course: Option<&TrackDef>,
    gates: &[Vec3],
    start: (Vec3, Vec3),
    standing: &[Placement],
    desert: bool,
) -> (Vec<Placement>, Vec<(Vec3, Vec3)>) {
    let mut path = Vec::new();
    // Each run of the course: points with their sides.
    let mut runs: Vec<Vec<(Vec2, Vec2, Vec2)>> = Vec::new();
    match course {
        Some(def) if !def.nodes.is_empty() => {
            for node in &def.nodes {
                let first = path.len();
                let mut run = Vec::new();
                for s in &node.sections {
                    let (c, l, r) = (flat(Vec3::from(s.center)), flat(Vec3::from(s.edges[0])), flat(Vec3::from(s.edges[1])));
                    let half = (l.distance(r) / 2.0).max(8.0);
                    path.push((c, half, None));
                    run.push((c, l, r));
                }
                for i in first..path.len().saturating_sub(1) {
                    path[i].2 = Some(i + 1);
                }
                runs.push(run);
            }
        }
        _ => {
            // Gates only: a course 30 ft wide through them.
            let mut run = Vec::new();
            for (i, g) in gates.iter().enumerate() {
                let c = flat(*g);
                path.push((c, 30.0, (i + 1 < gates.len()).then_some(i + 1)));
                let next = gates.get(i + 1).or(i.checked_sub(1).and_then(|j| gates.get(j))).map_or(c + Vec2::Y, |n| flat(*n));
                let side = (next - c).perp().normalize_or(Vec2::X) * 30.0;
                run.push((c, c + side, c - side));
            }
            runs.push(run);
        }
    }
    if path.len() < 3 {
        return (Vec::new(), Vec::new());
    }
    let mut ground = Ground {
        terrain,
        course: path,
        taken: HashMap::new(),
        random: 0x2545_f491,
    };
    for s in standing {
        ground.take(Vec2::new(s.position[0], s.position[2]), 5.0);
    }
    let mut out = Vec::new();
    // Where spectators stand, and which way they look.
    let mut people = Vec::new();

    // Along the course: bales (or drums) at the edge, now and then a
    // banner or a camera tower further out.
    let banners = ["Billboard_FOX", "Billboard_Dunlop", "Billboard_Oakley", "Billboard_Pirelli", "Billboard_Renthal", "Billboard_Alpinestars", "Billboard_MSR", "Billboard_Arai"];
    let mut spot = 0usize;
    for run in &runs {
        let mut walked = 0.0;
        for w in run.windows(2) {
            let ((c, l, r), (c2, ..)) = (w[0], w[1]);
            walked += c.distance(c2);
            if walked < SPACING {
                continue;
            }
            walked = 0.0;
            spot += 1;
            let along = (c2 - c).normalize_or(Vec2::Y);
            let left = ground.random() < 0.5;
            let (edge, out_dir) = if left { (l, (l - c).normalize_or(along.perp())) } else { (r, (r - c).normalize_or(-along.perp())) };
            if spot % 7 == 3 {
                if let Some(p) = [30.0, 40.0, 52.0].map(|d| edge + out_dir * d).into_iter().find(|&p| ground.fits(p, 9.0, 20.0, 0.3)) {
                    out.push(ground.prop("CameraPlatform01.slb", p, -out_dir, 1.0, 0.0, 9.0));
                }
            } else if spot % 4 == 1 {
                if let Some(p) = [22.0, 30.0, 40.0].map(|d| edge + out_dir * d).into_iter().find(|&p| ground.fits(p, 8.0, 16.0, 0.35)) {
                    let name = banners[(spot / 4) % banners.len()];
                    out.push(ground.prop(&format!("{name}.slb"), p, out_dir, 1.0, 13.2, 8.0));
                }
            } else {
                // A short row of two to four, a little set back.
                let Some(base) = [10.0, 14.0, 19.0].map(|d| edge + out_dir * d).into_iter().find(|&p| ground.fits(p, 2.0, 7.0, 0.5)) else {
                    continue;
                };
                let count = 2 + (ground.random() * 3.0) as usize;
                for k in 0..count {
                    let p = base + along * (k as f32 - (count - 1) as f32 / 2.0) * if desert { 2.6 } else { 4.0 };
                    if !ground.fits(p, 1.8, 7.0, 0.5) {
                        continue;
                    }
                    if desert {
                        out.push(ground.prop("50Gal.slb", p, along, 1.0, 0.0, 1.2));
                    } else {
                        out.push(ground.prop("HayBale.slb", p, along, 0.32, 0.0, 1.8));
                    }
                }
                // Spectators gathered behind, two or three deep.
                let crowd = 3 + (ground.random() * 9.0) as usize;
                for k in 0..crowd {
                    let p = base + out_dir * (4.0 + (k % 3) as f32 * 2.2 + ground.random()) + along * ((k / 3) as f32 * 2.3 - crowd as f32 / 3.0 + ground.random() * 0.8);
                    if ground.fits(p, 0.9, 10.0, 0.45) {
                        ground.take(p, 0.9);
                        let low = [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y].iter().fold(ground.height(p), |h, d| h.min(ground.height(p + *d * 0.5)));
                        people.push((Vec3::new(p.x, low, p.y), Vec3::new(-out_dir.x, 0.0, -out_dir.y)));
                    }
                }
            }
        }
    }

    // A paddock beside the start: trucks parked in rows, with chairs,
    // tables, a trailer and an outhouse.
    let (at, ahead) = (flat(start.0), flat(start.1).normalize_or(Vec2::Y));
    let side = ahead.perp();
    let trucks: &[&str] = if desert {
        &["Jeep.slb", "4Runner_Brown.slb", "Jeep_Grey.slb", "4Runner_White.slb", "MX3_Brown.slb"]
    } else {
        &["4Runner_Blue.slb", "MX3.slb", "4Runner_White.slb", "Jeep_Green.slb", "4Runner_Green.slb", "Jeep.slb"]
    };
    let mut parked = 0;
    'rows: for row in 0..6 {
        for s in [1.0f32, -1.0] {
            for k in 0..10 {
                if parked >= 14 {
                    break 'rows;
                }
                let p = at + side * s * (70.0 + row as f32 * 26.0) + ahead * (k as f32 * 18.0 - 60.0);
                if !ground.fits(p, 8.0, 30.0, 0.2) {
                    continue;
                }
                let truck = trucks[parked % trucks.len()];
                // Nosed in toward the course.
                out.push(ground.prop(truck, p, -side * s, 1.0, 0.0, 8.0));
                parked += 1;
                // Chairs or a table beside it.
                let by = p + ahead * 9.0;
                if ground.random() < 0.6 && ground.fits(by, 2.5, 30.0, 0.2) {
                    out.push(ground.prop("LawnChair01.slb", by, -side * s + ahead * 0.4, 1.0, 0.0, 2.0));
                    let by2 = by + side * s * 3.5;
                    if ground.fits(by2, 2.0, 30.0, 0.2) {
                        out.push(ground.prop("LawnChair01.slb", by2, -side * s - ahead * 0.3, 1.0, 0.0, 2.0));
                    }
                } else if ground.random() < 0.5 && ground.fits(by, 5.5, 30.0, 0.2) {
                    out.push(ground.prop("Picnic_Table01.slb", by, ahead, 1.0, 0.0, 5.5));
                }
                if parked == 5 {
                    let q = p + side * s * 30.0;
                    if ground.fits(q, 20.0, 30.0, 0.2) {
                        out.push(ground.prop("Pit_Trailer.slb", q, ahead, 1.0, 0.0, 20.0));
                    }
                }
                if parked == 9 {
                    let q = p + side * s * 18.0;
                    if ground.fits(q, 3.0, 30.0, 0.2) {
                        out.push(ground.prop("Outhouse01.slb", q, -side * s, 1.0, 0.0, 3.0));
                    }
                }
            }
        }
    }
    (out, people)
}

/// Spectators in a stadium's front rows: the stands just outside its wall
/// (its corners, in order), found by `stands` (the highest surface under
/// each point, game x and z), looking in. Rows `floor` ft up or less are
/// not stands.
pub fn stadium_crowd(outline: &[Vec3], floor: f32, stands: impl Fn(&[Vec2]) -> Vec<Option<f32>>) -> Vec<(Vec3, Vec3)> {
    let mut people = Vec::new();
    if outline.len() < 3 {
        return people;
    }
    let middle = outline.iter().sum::<Vec3>() / outline.len() as f32;
    let mut random = 0x1b87_3593u32;
    let mut next = move || {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        (random >> 8) as f32 / (1u32 << 24) as f32
    };
    let mut spots = Vec::new();
    for (i, &a) in outline.iter().enumerate() {
        let b = outline[(i + 1) % outline.len()];
        let length = flat(b - a).length();
        let along = flat(b - a).normalize_or(Vec2::X);
        let mut inward = along.perp();
        if inward.dot(flat(middle - a)) < 0.0 {
            inward = -inward;
        }
        let steps = (length / 2.4) as usize;
        for k in 0..steps {
            for row in 0..7 {
                // Gaps here and there.
                if next() < 0.3 {
                    continue;
                }
                let p = flat(a) + along * (k as f32 * 2.4 + 1.0 + next() * 0.5) - inward * (3.0 + row as f32 * 3.0 + next() * 0.4);
                spots.push((p, inward));
            }
        }
    }
    let heights = stands(&spots.iter().map(|s| s.0).collect::<Vec<_>>());
    for ((p, inward), h) in spots.into_iter().zip(heights) {
        if let Some(h) = h.filter(|&h| h > floor + 2.0) {
            people.push((Vec3::new(p.x, h, p.y), Vec3::new(inward.x, 0.0, inward.y)));
        }
    }
    people
}
