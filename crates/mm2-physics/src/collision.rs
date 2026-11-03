//! Solid objects from `.col` collision models: queries the bike and rider
//! make against the world (the game's `CollisionObject`, 0x432d60).
//!
//! An object is a `.col` body placed in the world. A point or sphere is
//! tested against its triangle tree: the tree's boxes cull, and each
//! triangle's plane and edges give the nearest point.

use glam::{Mat3, Vec3};
use mm2_formats::col::{self, Node};

/// How far behind a face a sphere's centre may get in one step and still
/// be pushed back out (our pick: a bike at full speed moves 3.7 ft a step).
const BEHIND: f32 = 4.0;

/// One `.col` body placed in the world.
#[derive(Debug, Clone)]
pub struct Solid {
    /// World from the body's own space: rotation (columns) and position.
    pub rotation: Mat3,
    pub position: Vec3,
    vertices: Vec<Vec3>,
    nodes: Vec<Node<col::Triangle>>,
    /// The body's bounds in world space, for a quick reject.
    centre: Vec3,
    radius: f32,
    /// The body's own matrix and the middle of its bounds, in its space.
    own: (Mat3, Vec3),
    local_centre: Vec3,
    /// Where a moving solid was the step before, so a point it sweeps
    /// over is caught, and how fast it goes (a scripted vehicle's +0x40).
    was: (Mat3, Vec3),
    pub velocity: Vec3,
}

/// Where a sphere touches a solid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Out of the solid, unit length.
    pub normal: Vec3,
    /// How far the sphere is inside.
    pub depth: f32,
    pub point: Vec3,
}

/// Where a moving point passed through a solid's face from its front
/// (0x428ae0): the face's normal, how far behind it the point ended, where
/// it crossed, and how far along its way (0 to 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossing {
    pub normal: Vec3,
    pub depth: f32,
    pub point: Vec3,
    pub along: f32,
    /// How fast the solid itself moves (zero but for scripted vehicles).
    pub velocity: Vec3,
}

impl Solid {
    /// Places `body` with `rotation` and `position`, after the body's own
    /// matrix.
    pub fn new(body: &col::Body, rotation: Mat3, position: Vec3) -> Self {
        let m = &body.matrix;
        let own = Mat3::from_cols_array(&[m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8]]);
        let own_pos = Vec3::new(m[9], m[10], m[11]);
        let vertices: Vec<Vec3> = body.vertices.iter().map(|&v| Vec3::from(v)).collect();
        let (lo, hi) = vertices.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), &v| (lo.min(v), hi.max(v)),
        );
        let local_centre = if vertices.is_empty() {
            Vec3::ZERO
        } else {
            (lo + hi) / 2.0
        };
        let radius = vertices
            .iter()
            .map(|v| v.distance(local_centre))
            .fold(0.0, f32::max);
        let mut solid = Solid {
            rotation,
            position,
            nodes: body.triangles.clone(),
            vertices,
            centre: Vec3::ZERO,
            radius,
            own: (own, own_pos),
            local_centre,
            was: (Mat3::IDENTITY, Vec3::ZERO),
            velocity: Vec3::ZERO,
        };
        solid.place(rotation, position, Vec3::ZERO);
        solid.was = (solid.rotation, solid.position);
        solid
    }

    /// Moves the solid to `rotation` and `position` (before the body's own
    /// matrix), going at `velocity`; its last place is kept for
    /// [`Solid::crossing`].
    pub fn place(&mut self, rotation: Mat3, position: Vec3, velocity: Vec3) {
        self.was = (self.rotation, self.position);
        self.rotation = rotation * self.own.0;
        self.position = self.rotation * self.own.1 + position;
        self.centre = self.rotation * self.local_centre + self.position;
        self.velocity = velocity;
    }

    /// The deepest touch of a sphere at `centre` with `radius`, if any.
    pub fn sphere(&self, centre: Vec3, radius: f32) -> Option<Hit> {
        if self.nodes.is_empty() || centre.distance(self.centre) > self.radius + radius {
            return None;
        }
        let local = self.rotation.transpose() * (centre - self.position);
        let mut best: Option<(Hit, bool)> = None;
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            match self.nodes[i] {
                Node::Inner { bounds, right } => {
                    let c = Vec3::from(bounds.center);
                    let h = Vec3::from(bounds.half) + Vec3::splat(radius);
                    let d = (local - c).abs();
                    if d.x <= h.x && d.y <= h.y && d.z <= h.z {
                        stack.push(right as usize);
                        stack.push(i + 1);
                    }
                }
                Node::Leaf(t) => {
                    let [a, b, c] = t.vertices.map(|v| self.vertices[v as usize]);
                    let n = Vec3::from(t.normal);
                    // Only the front of a face pushes, so a thin wall
                    // does not pull a sphere through it; a little behind
                    // it still counts, as a fast bike moves up to 4 ft a
                    // step.
                    let plane = (local - a).dot(n);
                    if plane < -(radius + BEHIND) {
                        continue;
                    }
                    let p = closest_on_triangle(local, a, b, c);
                    let away = local - p;
                    let dist = away.length();
                    let (normal, depth, past) = if plane < 0.0 {
                        // Past the face: back out along it.
                        if dist >= radius + BEHIND {
                            continue;
                        }
                        (n, radius - plane, true)
                    } else {
                        if dist >= radius {
                            continue;
                        }
                        (
                            if dist > 1e-5 { away / dist } else { n },
                            radius - dist,
                            false,
                        )
                    };
                    // A touch from in front beats one from behind; the
                    // deepest touch from in front counts, but a centre
                    // already inside backs out the nearest way, not
                    // through the far side of a small object.
                    let better = match best {
                        None => true,
                        Some((h, was_past)) => match (was_past, past) {
                            (true, false) => true,
                            (false, true) => false,
                            (false, false) => depth > h.depth,
                            (true, true) => depth < h.depth,
                        },
                    };
                    if better {
                        best = Some((
                            Hit {
                                normal,
                                depth,
                                point: p,
                            },
                            past,
                        ));
                    }
                }
            }
        }
        best.map(|(h, _)| Hit {
            normal: self.rotation * h.normal,
            depth: h.depth,
            point: self.rotation * h.point + self.position,
        })
    }
}

impl Solid {
    /// The first face a point moving from `from` to `to` passes through
    /// from its front, if any (0x428fb0 over the triangle tree, 0x428ae0).
    pub fn crossing(&self, from: Vec3, to: Vec3) -> Option<Crossing> {
        if self.nodes.is_empty() {
            return None;
        }
        let mid = (from + to) / 2.0;
        let swept = from.distance(to) / 2.0 + self.position.distance(self.was.1);
        if mid.distance(self.centre) > self.radius + swept {
            return None;
        }
        // Against a moving solid the point's way is taken in the solid's
        // own space: where it was against where the solid was.
        let back = self.rotation.transpose();
        let a = self.was.0.transpose() * (from - self.was.1);
        let b = back * (to - self.position);
        let (lo, hi) = (a.min(b), a.max(b));
        let mut best: Option<Crossing> = None;
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            match self.nodes[i] {
                Node::Inner { bounds, right } => {
                    let c = Vec3::from(bounds.center);
                    let h = Vec3::from(bounds.half);
                    if (lo - (c + h)).max_element() <= 0.0 && ((c - h) - hi).max_element() <= 0.0 {
                        stack.push(right as usize);
                        stack.push(i + 1);
                    }
                }
                Node::Leaf(t) => {
                    let [p, q, r] = t.vertices.map(|v| self.vertices[v as usize]);
                    let n = Vec3::from(t.normal);
                    let (f, e) = ((a - p).dot(n), (b - p).dot(n));
                    if f < 0.0 || e >= 0.0 {
                        continue;
                    }
                    let along = f / (f - e);
                    if best.is_some_and(|c| c.along <= along) {
                        continue;
                    }
                    let at = a + (b - a) * along;
                    if inside_triangle(at, p, q, r, n) {
                        best = Some(Crossing {
                            normal: n,
                            depth: -e,
                            point: at,
                            along,
                            velocity: Vec3::ZERO,
                        });
                    }
                }
            }
        }
        best.map(|c| Crossing {
            normal: self.rotation * c.normal,
            point: self.rotation * c.point + self.position,
            velocity: self.velocity,
            ..c
        })
    }
}

/// Whether `p`, on the plane of triangle `abc` with normal `n`, is inside
/// it (0x4287a0).
fn inside_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3, n: Vec3) -> bool {
    let side = |u: Vec3, v: Vec3| (v - u).cross(p - u).dot(n);
    let (x, y, z) = (side(a, b), side(b, c), side(c, a));
    (x >= 0.0 && y >= 0.0 && z >= 0.0) || (x <= 0.0 && y <= 0.0 && z <= 0.0)
}

/// A tree trunk: an upright cylinder standing on `base` (the eco-system's
/// 16-sided prisms, 0x458c90).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trunk {
    pub base: Vec3,
    pub radius: f32,
    pub height: f32,
}

impl Trunk {
    pub fn sphere(&self, centre: Vec3, radius: f32) -> Option<Hit> {
        let y = centre.y.clamp(self.base.y, self.base.y + self.height);
        let axis = Vec3::new(self.base.x, y, self.base.z);
        let out = centre - axis;
        let d = out.length();
        let reach = self.radius + radius;
        if d >= reach || d < 1e-5 {
            return None;
        }
        let normal = out / d;
        Some(Hit {
            normal,
            depth: reach - d,
            point: axis + normal * self.radius,
        })
    }
}

impl Trunk {
    /// A point moving into the trunk's side from outside (the trunk
    /// prisms are collision objects like any other, 0x43b180).
    pub fn crossing(&self, from: Vec3, to: Vec3) -> Option<Crossing> {
        if to.y < self.base.y || to.y > self.base.y + self.height {
            return None;
        }
        let flat = |p: Vec3| Vec3::new(p.x - self.base.x, 0.0, p.z - self.base.z);
        let (a, b) = (flat(from), flat(to));
        let (da, db) = (a.length(), b.length());
        if da < self.radius || db >= self.radius || db < 1e-5 {
            return None;
        }
        let normal = b / db;
        Some(Crossing {
            normal,
            depth: self.radius - db,
            point: Vec3::new(self.base.x, to.y, self.base.z) + normal * self.radius,
            along: (da - self.radius) / (da - db).max(1e-5),
            velocity: Vec3::ZERO,
        })
    }
}

/// Trunks sorted into square cells on the level, for quick lookups.
#[derive(Debug, Clone, Default)]
pub struct Trunks {
    cells: std::collections::HashMap<(i32, i32), Vec<Trunk>>,
}

impl Trunks {
    /// Cell side, ft: wider than any trunk and bike sphere together.
    const CELL: f32 = 32.0;

    pub fn new(trunks: impl IntoIterator<Item = Trunk>) -> Self {
        let mut cells: std::collections::HashMap<(i32, i32), Vec<Trunk>> = Default::default();
        for t in trunks {
            cells.entry(Self::cell(t.base)).or_default().push(t);
        }
        Trunks { cells }
    }

    fn cell(p: Vec3) -> (i32, i32) {
        (
            (p.x / Self::CELL).floor() as i32,
            (p.z / Self::CELL).floor() as i32,
        )
    }

    pub fn len(&self) -> usize {
        self.cells.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn crossing(&self, from: Vec3, to: Vec3) -> Option<Crossing> {
        let (cx, cz) = Self::cell(to);
        let mut best: Option<Crossing> = None;
        for x in cx - 1..=cx + 1 {
            for z in cz - 1..=cz + 1 {
                for t in self.cells.get(&(x, z)).into_iter().flatten() {
                    if let Some(c) = t.crossing(from, to) {
                        if best.is_none_or(|b| c.along < b.along) {
                            best = Some(c);
                        }
                    }
                }
            }
        }
        best
    }

    pub fn sphere(&self, centre: Vec3, radius: f32) -> Option<Hit> {
        let (cx, cz) = Self::cell(centre);
        let mut best: Option<Hit> = None;
        for x in cx - 1..=cx + 1 {
            for z in cz - 1..=cz + 1 {
                for t in self.cells.get(&(x, z)).into_iter().flatten() {
                    if let Some(hit) = t.sphere(centre, radius) {
                        if best.is_none_or(|b| hit.depth > b.depth) {
                            best = Some(hit);
                        }
                    }
                }
            }
        }
        best
    }
}

/// All the solids of a track.
#[derive(Debug, Clone, Default)]
pub struct World {
    pub solids: Vec<Solid>,
    /// Tree trunks, when trees are solid (the race's Tree Collision).
    pub trunks: Trunks,
}

impl World {
    /// The deepest touch of a sphere against any solid.
    pub fn sphere(&self, centre: Vec3, radius: f32) -> Option<Hit> {
        self.solids
            .iter()
            .filter_map(|s| s.sphere(centre, radius))
            .chain(self.trunks.sphere(centre, radius))
            .max_by(|a, b| a.depth.total_cmp(&b.depth))
    }

    /// The first solid face a point moving from `from` to `to` passes
    /// through from its front.
    pub fn crossing(&self, from: Vec3, to: Vec3) -> Option<Crossing> {
        self.solids
            .iter()
            .filter_map(|s| s.crossing(from, to))
            .chain(self.trunks.crossing(from, to))
            .min_by(|a, b| a.along.total_cmp(&b.along))
    }
}

/// The point of triangle `abc` nearest `p` (Ericson, Real-Time Collision
/// Detection, 5.1.5).
fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_formats::col::{Body, Bounds, Triangle};

    #[test]
    fn a_trunk_pushes_out_sideways() {
        let trunks = Trunks::new([Trunk {
            base: Vec3::new(100.0, 10.0, 100.0),
            radius: 0.8,
            height: 8.0,
        }]);
        let hit = trunks.sphere(Vec3::new(101.0, 12.0, 100.0), 1.0).unwrap();
        assert!((hit.normal - Vec3::X).length() < 1e-5);
        assert!((hit.depth - 0.8).abs() < 1e-5);
        assert!(trunks.sphere(Vec3::new(101.0, 20.0, 100.0), 1.0).is_none());
        assert!(trunks.sphere(Vec3::new(103.0, 12.0, 100.0), 1.0).is_none());
    }

    /// A floor square at y = 0, two triangles facing up.
    fn floor() -> Body {
        let tri = |v: [u16; 3]| {
            Node::Leaf(Triangle {
                normal: [0.0, 1.0, 0.0],
                d: 0.0,
                vertices: v,
            })
        };
        Body {
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            flags: 0,
            vertices: vec![
                [-5.0, 0.0, -5.0],
                [5.0, 0.0, -5.0],
                [5.0, 0.0, 5.0],
                [-5.0, 0.0, 5.0],
            ],
            triangles: vec![
                Node::Inner {
                    bounds: Bounds {
                        center: [0.0; 3],
                        half: [5.0, 0.0, 5.0],
                    },
                    right: 2,
                },
                tri([0, 2, 1]),
                tri([0, 3, 2]),
            ],
            points: Vec::new(),
        }
    }

    #[test]
    fn sphere_on_a_placed_floor() {
        let s = Solid::new(&floor(), Mat3::IDENTITY, Vec3::new(100.0, 10.0, 0.0));
        let hit = s.sphere(Vec3::new(101.0, 10.5, 2.0), 1.0).unwrap();
        assert!((hit.depth - 0.5).abs() < 1e-5);
        assert!(hit.normal.abs_diff_eq(Vec3::Y, 1e-5));
        assert!(s.sphere(Vec3::new(101.0, 11.5, 2.0), 1.0).is_none());
        assert!(s.sphere(Vec3::new(110.0, 10.5, 2.0), 1.0).is_none());
        // Below the floor does not count: only the front pushes.
        assert!(s.sphere(Vec3::new(101.0, 8.5, 2.0), 1.0).is_none());
    }

    #[test]
    fn edges_push_sideways() {
        let tilted = Mat3::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let wall = Solid::new(&floor(), tilted, Vec3::ZERO);
        let hit = wall.sphere(Vec3::new(-0.6, 0.0, 0.0), 1.0).unwrap();
        assert!(
            hit.normal.abs_diff_eq(Vec3::NEG_X, 1e-5),
            "{:?}",
            hit.normal
        );
    }

    #[test]
    fn a_point_moving_through_the_front_is_caught() {
        let s = Solid::new(&floor(), Mat3::IDENTITY, Vec3::new(100.0, 10.0, 0.0));
        let c = s
            .crossing(Vec3::new(101.0, 11.0, 2.0), Vec3::new(101.0, 9.5, 2.0))
            .unwrap();
        assert!((c.depth - 0.5).abs() < 1e-5);
        assert!((c.along - 2.0 / 3.0).abs() < 1e-5);
        assert!(c.normal.abs_diff_eq(Vec3::Y, 1e-5));
        assert!(c.point.abs_diff_eq(Vec3::new(101.0, 10.0, 2.0), 1e-4));
        // Up through the back, beside the square, or short of it: no.
        assert!(s
            .crossing(Vec3::new(101.0, 9.5, 2.0), Vec3::new(101.0, 11.0, 2.0))
            .is_none());
        assert!(s
            .crossing(Vec3::new(107.0, 11.0, 2.0), Vec3::new(107.0, 9.5, 2.0))
            .is_none());
        assert!(s
            .crossing(Vec3::new(101.0, 11.0, 2.0), Vec3::new(101.0, 10.5, 2.0))
            .is_none());
    }
}
