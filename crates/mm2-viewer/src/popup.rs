//! The stunt score that flies up from the bike after a scoring jump
//! (0x48d2e0 builds it, 0x48e190 sets the number, 0x48dc70 moves it): the
//! base points in 3D digits (`0.slt` to `9.slt` in `RES/MODELS.RES`), the
//! last two always zeros, and `×m.m` after them (`x.slt`, `DecimalPoint.slt`)
//! when the multiplier is 1.05 or more. It swells, holds facing the camera,
//! then spins away upwards over 6 s, kept the same size on screen.

use glam::{Mat4, Quat, Vec3, Vec4};
use mm2_formats::scn::Placement;

/// The keys (0x5728a4): time, scale, height above the bike, and the turn
/// about the up axis; while a key and the next both have no turn, the
/// score faces the camera.
const KEYS: [(f32, f32, f32, f32); 7] = [
    (0.0, 0.0275, 8.0, 0.0),
    (0.4, 2.78, 8.0, 0.0),
    (0.5333, 2.09, 8.0, 0.0),
    (1.6667, 2.09, 8.0, 0.0),
    (2.2, 1.99, 18.0, std::f32::consts::PI),
    (2.6667, 1.18, 29.0, 4.607_669),
    (6.0, 0.05, 117.0, 15.079_643),
];
/// The score is gone after this long (0x5729c4).
pub const LIFE: f32 = 6.0;
/// Where each digit sits across the score: the two zeros, then the base
/// points over 100 from the units leftwards; and the multiplier's sign,
/// point, and digits (0x48d2e0).
const ZEROS: [f32; 2] = [0.74, 0.37];
const DIGITS: [f32; 3] = [0.0, -0.37, -0.74];
const TIMES: f32 = 1.18;
const POINT: f32 = 1.67;
const MULTIPLIER: [f32; 2] = [1.87, 1.87 - 0.41];
/// Ahead of the bike along its rear tyre (0x48dc70).
const AHEAD: f32 = 5.0;
/// The size on screen holds as the distance grows: the scale is multiplied
/// by distance × 60 / (focal length × 1.8) (+0x1e0, +0x1e4).
const SIZE: f32 = 60.0 / 1.8;

/// The score's models, as animated objects.
#[derive(Debug, Clone, Default)]
pub struct Popup {
    zeros: [usize; 2],
    digits: [[usize; 10]; 3],
    multiplier: [[usize; 10]; 2],
    times: usize,
    point: usize,
}

impl Popup {
    /// The models to load, in the order [`Popup::new`] takes them.
    pub fn placements() -> Vec<Placement> {
        let model = |m: String| Placement {
            model: m,
            position: [0.0; 3],
            look: [0.0, 0.0, 1.0],
            up: [0.0, 1.0, 0.0],
            scale: [0.0; 3],
            lighting: Default::default(),
        };
        let mut out: Vec<Placement> = (0..2).map(|_| model("0.slt".into())).collect();
        for _ in 0..DIGITS.len() + MULTIPLIER.len() {
            out.extend((0..10).map(|d| model(format!("{d}.slt"))));
        }
        out.push(model("x.slt".into()));
        out.push(model("DecimalPoint.slt".into()));
        out
    }

    /// The score from its models' objects, in [`Popup::placements`] order;
    /// none if any is missing.
    pub fn new(objects: &[Option<usize>]) -> Option<Popup> {
        if objects.len() != 2 + 50 + 2 {
            return None;
        }
        let o: Vec<usize> = objects.iter().copied().collect::<Option<_>>()?;
        let ten = |at: usize| -> [usize; 10] { std::array::from_fn(|d| o[at + d]) };
        Some(Popup {
            zeros: [o[0], o[1]],
            digits: [ten(2), ten(12), ten(22)],
            multiplier: [ten(32), ten(42)],
            times: o[52],
            point: o[53],
        })
    }

    /// Every model's place: `age` seconds after a jump of `base` points ×
    /// `multiplier`, over a bike at `bike` heading `forward`, seen from
    /// `eye` with a lens of `focal` pixels on the 640×480 screen. With no
    /// score, all are hidden.
    pub fn places(
        &self,
        score: Option<(f32, f32, f32)>,
        bike: Vec3,
        forward: Vec3,
        eye: Vec3,
        focal: f32,
    ) -> Vec<(usize, Mat4)> {
        let mut shown: Vec<(usize, f32)> = Vec::new();
        let frame = score.and_then(|(age, base, multiplier)| {
            if !(0.0..LIFE).contains(&age) {
                return None;
            }
            shown.extend(self.zeros.into_iter().zip(ZEROS));
            let mut n = base.round() as u32 / 100;
            let count = if base >= 10000.0 { 3 } else { 2 };
            for (models, x) in self.digits.iter().zip(DIGITS).take(count) {
                shown.push((models[(n % 10) as usize], x));
                n /= 10;
            }
            if multiplier >= 1.05 {
                let mut m = (multiplier * 10.0).round() as u32;
                for (models, x) in self.multiplier.iter().zip(MULTIPLIER) {
                    shown.push((models[(m % 10) as usize], x));
                    m /= 10;
                }
                shown.push((self.times, TIMES));
                shown.push((self.point, POINT));
            }
            Some(frame(age, bike, forward, eye, focal))
        });
        let all = self
            .zeros
            .iter()
            .chain(self.digits.iter().flatten())
            .chain(self.multiplier.iter().flatten())
            .chain([&self.times, &self.point]);
        all.map(|&o| {
            let place = match (frame, shown.iter().find(|s| s.0 == o)) {
                (Some(f), Some(&(_, x))) => f * Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
                _ => Mat4::from_cols(Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, Vec4::W),
            };
            (o, place)
        })
        .collect()
    }
}

/// The score's frame at `age` (0x48dc70): keys blended in a straight line.
fn frame(age: f32, bike: Vec3, forward: Vec3, eye: Vec3, focal: f32) -> Mat4 {
    let k = KEYS
        .windows(2)
        .position(|w| age < w[1].0)
        .unwrap_or(KEYS.len() - 2);
    let (a, b) = (KEYS[k], KEYS[k + 1]);
    let t = ((age - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let centre = bike + Vec3::Y * lerp(a.2, b.2) + forward * AHEAD;
    let turn = if a.3 == 0.0 && b.3 == 0.0 {
        // Facing the camera, level.
        let look = Vec3::new(centre.x - eye.x, 0.0, centre.z - eye.z).normalize_or(Vec3::Z);
        Quat::from_rotation_arc(Vec3::Z, look)
    } else {
        Quat::from_rotation_y(lerp(a.3, b.3))
    };
    let size = lerp(a.1, b.1) * centre.distance(eye) * SIZE / focal;
    Mat4::from_scale_rotation_translation(Vec3::splat(size), turn, centre)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_score() {
        let objects: Vec<_> = (0..54).map(Some).collect();
        let p = Popup::new(&objects).unwrap();
        let hidden = |m: &Mat4| m.x_axis == Vec4::ZERO;
        // 4500 × 1.5: zeros, 5, 4, and ×1.5.
        let places = p.places(
            Some((1.0, 4500.0, 1.5)),
            Vec3::ZERO,
            Vec3::Z,
            Vec3::new(0.0, 8.0, -20.0),
            415.0,
        );
        let shown: Vec<usize> = places
            .iter()
            .filter(|(_, m)| !hidden(m))
            .map(|p| p.0)
            .collect();
        assert_eq!(shown, [0, 1, 2 + 5, 12 + 4, 32 + 5, 42 + 1, 52, 53]);
        // Gone after its time, or with no score.
        assert!(p
            .places(
                Some((6.5, 4500.0, 1.0)),
                Vec3::ZERO,
                Vec3::Z,
                Vec3::Z,
                415.0
            )
            .iter()
            .all(|(_, m)| hidden(m)));
        assert!(p
            .places(None, Vec3::ZERO, Vec3::Z, Vec3::Z, 415.0)
            .iter()
            .all(|(_, m)| hidden(m)));
        // 12300 shows 1, 2, 3 and the zeros; no ×1.0.
        let places = p.places(
            Some((1.0, 12300.0, 1.0)),
            Vec3::ZERO,
            Vec3::Z,
            Vec3::new(0.0, 8.0, -20.0),
            415.0,
        );
        let shown: Vec<usize> = places
            .iter()
            .filter(|(_, m)| !hidden(m))
            .map(|p| p.0)
            .collect();
        assert_eq!(shown, [0, 1, 2 + 3, 12 + 2, 22 + 1]);
    }
}
