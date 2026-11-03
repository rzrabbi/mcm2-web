//! What a camera can see, to leave the rest undrawn: the ground and the
//! still objects are cut into blocks, and a block wholly outside the view
//! is skipped. Only what could not reach the screen is left out, so the
//! picture is the same.

use glam::{Mat4, Vec3, Vec4};

/// The six sides of what a view and projection take in (drawing
/// coordinates), each a plane with the inside where it is positive.
#[derive(Clone, Copy)]
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// From a view and projection (wgpu's depth, 0 to 1).
    pub fn new(view_proj: Mat4) -> Frustum {
        let m = view_proj;
        let [r0, r1, r2, r3] = [m.row(0), m.row(1), m.row(2), m.row(3)];
        Frustum {
            planes: [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2],
        }
    }

    /// Whether any of the box from `lo` to `hi` might be seen.
    pub fn sees(&self, lo: Vec3, hi: Vec3) -> bool {
        self.planes.iter().all(|p| {
            // The box's corner furthest inside this side.
            let c = Vec3::select(p.truncate().cmpge(Vec3::ZERO), hi, lo);
            p.truncate().dot(c) + p.w >= 0.0
        })
    }
}

/// Runs of indices to draw, `start..end`, joined where they meet.
pub fn runs(ranges: impl Iterator<Item = std::ops::Range<u32>>, out: &mut Vec<std::ops::Range<u32>>) {
    out.clear();
    for r in ranges {
        match out.last_mut() {
            Some(last) if last.end == r.start => last.end = r.end,
            _ => out.push(r),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_ahead_not_behind() {
        let proj = Mat4::perspective_rh(1.0, 1.5, 1.0, 1000.0);
        let view = Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y);
        let f = Frustum::new(proj * view);
        assert!(f.sees(Vec3::new(-1.0, -1.0, -20.0), Vec3::new(1.0, 1.0, -10.0)));
        assert!(!f.sees(Vec3::new(-1.0, -1.0, 10.0), Vec3::new(1.0, 1.0, 20.0)));
        assert!(!f.sees(Vec3::new(-1.0, -1.0, -2000.0), Vec3::new(1.0, 1.0, -1500.0)));
        assert!(!f.sees(Vec3::new(500.0, -1.0, -20.0), Vec3::new(600.0, 1.0, -10.0)));
    }

    #[test]
    fn joins_runs() {
        let mut out = Vec::new();
        runs([0..3, 3..6, 9..12].into_iter(), &mut out);
        assert_eq!(out, vec![0..6, 9..12]);
    }
}
