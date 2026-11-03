//! Bike against bike: 0x405fb0 for an object of type 100 and the
//! two-body bounce 0x52bc20 → 0x505330.
//!
//! Each bike's collision points (the frame's and the wheels', as against
//! the solids) are swept from where they were last step to where they are
//! now against the other bike's `honda.col` meshes, which move too. A
//! point that passed through a face from its front pushes its bike back
//! out, and both bikes take one impulse at the points' middle.

use crate::bike::{axle, Hull, PUSH_MARGIN, RESTITUTION};
use crate::collision::Solid;
use glam::{Mat3, Vec3};

/// Bikes whose origins are further apart than this can't touch, ft (our
/// pick: a bike is about 7 ft long).
const REACH: f32 = 12.0;
/// A bike this far above the one it hits lands on it, and the lower one
/// comes off (0x405fb0's `f_608`, crash 2), ft (0x555840).
const LANDED_ON: f32 = 1.5;

/// One bike as the bounce sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    /// The model's origin, on the ground between the tyres, and its axes
    /// (right, up, look).
    pub origin: Vec3,
    pub axes: Mat3,
    /// Where they were last step.
    pub was: (Vec3, Mat3),
    /// The centre of mass, the velocity and the spin, in the world.
    pub com: Vec3,
    pub velocity: Vec3,
    pub spin: Vec3,
    /// Mass (weight × 0.0310559) and the inertia about the bike's own
    /// axes.
    pub mass: f32,
    pub inertia: Vec3,
}

/// What a bump does to one bike.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Bump {
    pub shift: Vec3,
    pub velocity: Vec3,
    /// Added to the spin, in the world.
    pub spin: Vec3,
    /// Landed on from above: the rider comes off.
    pub landed_on: bool,
}

/// The two bikes' bumps, when they touch.
pub fn bump(hull: &Hull, a: &State, b: &State) -> Option<(Bump, Bump)> {
    if a.origin.distance(b.origin) > REACH {
        return None;
    }
    let (mut a, mut b) = (*a, *b);
    let (mut ba, mut bb) = (Bump::default(), Bump::default());
    let mut touched = false;
    // Each bike's own update sweeps it against the other in turn.
    for first in [true, false] {
        let (this, other, mine, theirs) = if first {
            (&mut a, &mut b, &mut ba, &mut bb)
        } else {
            (&mut b, &mut a, &mut bb, &mut ba)
        };
        let Some((out, point, normal)) = sweep(hull, this, other) else {
            continue;
        };
        touched = true;
        let shift = out + out.normalize_or_zero() * PUSH_MARGIN;
        this.origin += shift;
        this.com += shift;
        mine.shift += shift;
        if other.origin.y - this.origin.y > LANDED_ON {
            mine.landed_on = true;
        } else if this.origin.y - other.origin.y > LANDED_ON {
            theirs.landed_on = true;
        }
        let (dv, dw, odv, odw) = impulse(this, other, point, normal);
        this.velocity += dv;
        this.spin += dw;
        other.velocity += odv;
        other.spin += odw;
        mine.velocity += dv;
        mine.spin += dw;
        theirs.velocity += odv;
        theirs.spin += odw;
    }
    touched.then_some((ba, bb))
}

/// `this` bike's points against `other`'s meshes: how far to push it
/// out, where they touched and the face's normal, out of `other`.
fn sweep(hull: &Hull, this: &State, other: &State) -> Option<(Vec3, Vec3, Vec3)> {
    let offset = |wheel: Option<usize>| wheel.map_or(Vec3::ZERO, |i| axle(i, 0.0));
    let solids: Vec<Solid> = hull
        .meshes
        .iter()
        .map(|(wheel, mesh)| {
            let off = offset(*wheel);
            let (at, axes) = other.was;
            let mut solid = Solid::new(mesh, axes, at + axes * off);
            solid.place(other.axes, other.origin + other.axes * off, other.velocity);
            solid
        })
        .collect();
    let first = |from: Vec3, to: Vec3| {
        solids
            .iter()
            .filter_map(|s| s.crossing(from, to))
            .min_by(|p, q| p.along.total_cmp(&q.along))
    };
    let mut out = Vec3::ZERO;
    let (mut middle, mut count) = (Vec3::ZERO, 0);
    let mut normals: Vec<Vec3> = Vec::new();
    let points = [
        (None, &hull.body),
        (Some(0), &hull.wheels[0]),
        (Some(1), &hull.wheels[1]),
    ];
    for (wheel, points) in points {
        let off = offset(wheel);
        for &p in points {
            let from = this.was.0 + this.was.1 * (p + off);
            let to = this.origin + this.axes * (p + off);
            for _ in 0..4 {
                let Some(c) = first(from, to + out) else {
                    break;
                };
                out += c.normal * c.depth;
                middle += c.point;
                count += 1;
                if !normals.iter().any(|n| n.distance(c.normal) < 1e-4) {
                    normals.push(c.normal);
                }
            }
        }
    }
    (count > 0).then(|| {
        let normal = normals.iter().sum::<Vec3>().normalize_or_zero();
        (out, middle / count as f32, normal)
    })
}

/// The bounce at `point` along `normal` (0x52bc20 → 0x505330): the
/// changes to `this` bike's velocity and spin, then the other's.
#[allow(clippy::approx_constant)] // the game's own 0.5236 (0x55588c)
fn impulse(this: &State, other: &State, point: Vec3, normal: Vec3) -> (Vec3, Vec3, Vec3, Vec3) {
    // Softer the further behind the bike's heading the touch is; past
    // half a turn the game's angle goes negative, which counts as head on.
    let to = point - this.origin;
    let heading = this.axes.z_axis.x.atan2(this.axes.z_axis.z);
    let mut angle = (to.x.atan2(to.z) - heading).abs();
    if angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    let soft = if angle > 2.62 {
        0.2
    } else if angle > 2.09 {
        0.6
    } else if angle > 0.523_6 {
        0.8
    } else {
        1.0
    };
    let (ra, rb) = (point - this.com, point - other.com);
    let va = (this.velocity + this.spin.cross(ra)) * soft;
    let vb = (other.velocity + other.spin.cross(rb)) * soft;
    let twist = |s: &State, r: Vec3| s.axes * (s.axes.transpose() * r.cross(normal) / s.inertia);
    let (ta, tb) = (twist(this, ra), twist(other, rb));
    let turn = normal.dot(ta.cross(ra) + tb.cross(rb));
    let j =
        -(1.0 + RESTITUTION) * (va - vb).dot(normal) / (turn + 1.0 / this.mass + 1.0 / other.mass);
    // The game turns the other bike the same way as this one, not
    // against it (0x505330 adds +j to both spins).
    (
        normal * j / this.mass,
        ta * j,
        -normal * j / other.mass,
        tb * j,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(origin: Vec3, velocity: Vec3) -> State {
        State {
            origin,
            axes: Mat3::IDENTITY,
            was: (origin - velocity * 0.02, Mat3::IDENTITY),
            com: origin + crate::bike::COM,
            velocity,
            spin: Vec3::ZERO,
            mass: 400.0 * 0.031_055_9,
            inertia: crate::bike::INERTIA * 400.0 * 0.031_055_9 / 12.0,
        }
    }

    #[test]
    fn apart_bikes_do_not_touch() {
        let hull = Hull::default();
        let a = state(Vec3::ZERO, Vec3::ZERO);
        let b = state(Vec3::new(20.0, 0.0, 0.0), Vec3::ZERO);
        assert!(bump(&hull, &a, &b).is_none());
    }

    #[test]
    fn head_on_is_full_and_from_behind_is_soft() {
        let s = state(Vec3::ZERO, Vec3::new(0.0, 0.0, 30.0));
        let o = state(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO);
        let front = impulse(&s, &o, Vec3::new(0.0, 2.0, 3.0), Vec3::new(0.0, 0.0, -1.0));
        let back = impulse(&s, &o, Vec3::new(0.0, 2.0, -3.0), Vec3::new(0.0, 0.0, -1.0));
        assert!(front.0.z < 0.0 && back.0.z < 0.0);
        assert!((back.0.z / front.0.z - 0.2).abs() < 1e-3);
    }
}
