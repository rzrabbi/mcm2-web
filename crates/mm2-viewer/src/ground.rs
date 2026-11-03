//! What the player's bike rides on: the terrain, and the track's solid
//! objects from their `.col` collision models in `RES/COLSEG.RES`.

use crate::ride;
use glam::{Mat3, Vec3};
use mm2_formats::col::{self, Body, Bounds, Node};
use mm2_formats::scn::{self, Placement};
use mm2_formats::trn::Terrain;
use mm2_formats::{rs2, slb};
use mm2_physics::collision::{Crossing, Hit, Solid, World};
use mm2_physics::Ground;

/// The terrain and the solids on it, for [`mm2_physics::Bike`].
#[derive(Clone, Copy)]
pub struct TrackGround<'a> {
    pub terrain: &'a Terrain,
    pub world: &'a World,
    pub zones: &'a [scn::Zone; 8],
    /// Whether riding off the world's edge throws the rider back (all but
    /// Supercross).
    pub edge: bool,
}

impl TrackGround<'_> {
    /// The zone of the terrain sample nearest `at` (0x50d3b0), by its
    /// surface type's low 3 bits (0x51a710).
    pub fn zone(&self, at: Vec3) -> scn::Zone {
        let t = self.terrain;
        let last = t.size - 1;
        let x = ((at.x / t.scale).max(0.0).round() as usize).min(last);
        let z = ((at.z / t.scale).max(0.0).round() as usize).min(last);
        self.zones[(t.surfaces[z * t.size + x] & 7) as usize]
    }
}

impl Ground for TrackGround<'_> {
    fn terrain(&self, x: f32, z: f32) -> (f32, Vec3) {
        let at = Vec3::new(x, 0.0, z);
        (
            ride::ground(self.terrain, x, z),
            ride::normal(self.terrain, at),
        )
    }

    fn edge(&self) -> Option<(f32, f32)> {
        let side = (self.terrain.size - 1) as f32 * self.terrain.scale;
        self.edge.then_some((side, side))
    }

    fn solid(&self, centre: Vec3, radius: f32) -> Option<Hit> {
        self.world.sphere(centre, radius)
    }

    fn crossing(&self, from: Vec3, to: Vec3) -> Option<Crossing> {
        self.world.crossing(from, to)
    }

    fn friction(&self, at: Vec3) -> f32 {
        self.zone(at).friction
    }

    fn traction(&self, at: Vec3) -> f32 {
        self.zone(at).traction
    }
}

/// The corners of a stadium's wall (`COLSEG.RES` segment `seg`, placed by
/// `frame`), on the ground plane.
pub fn stadium_outline(seg: &str, frame: &scn::StadiumFrame) -> Vec<Vec3> {
    let at = Vec3::from(frame.position);
    seg
        .lines()
        .skip_while(|l| !l.trim().eq_ignore_ascii_case("[Segment Data]"))
        .skip(1)
        .filter_map(|line| {
            let mut n = line.split(',').map(|x| x.trim().parse::<f32>());
            Some(Vec3::new(n.next()?.ok()?, 0.0, n.next()?.ok()?) * frame.scale + at)
        })
        .collect()
}

/// A stadium's walls from its `.seg` in `RES/COLSEG.RES` (0x5098c0): an
/// outline of `x,z` corners, scaled and placed by the frame and closed
/// back to the first, each side a wall from `Bottom` up `Top` (0x509b50),
/// facing in.
pub fn stadium_walls(seg: &str, frame: &scn::StadiumFrame) -> Vec<Solid> {
    let corners = stadium_outline(seg, frame);
    if corners.len() < 3 {
        return Vec::new();
    }
    let middle = corners.iter().sum::<Vec3>() / corners.len() as f32;
    // Bottom and Top are heights in the world, not the frame's: a wall
    // stands from Bottom to Bottom + Top, whatever the frame's scale and
    // place (0x509b50's y is its last two arguments only).
    let (bottom, top) = (frame.bottom, frame.bottom + frame.top);
    let mut walls = Vec::new();
    for (i, &a) in corners.iter().enumerate() {
        let b = corners[(i + 1) % corners.len()];
        let along = (b - a).normalize_or_zero();
        let mut normal = Vec3::new(-along.z, 0.0, along.x);
        if normal.dot(middle - a) < 0.0 {
            normal = -normal;
        }
        let up = |p: Vec3, y: f32| [p.x, y, p.z];
        let vertices = vec![up(a, bottom), up(b, bottom), up(b, top), up(a, top)];
        let d = normal.dot(a);
        let triangle = |v: [u16; 3]| {
            col::Node::Leaf(col::Triangle {
                normal: normal.into(),
                d,
                vertices: v,
            })
        };
        let (lo, hi) = vertices.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), &v| (lo.min(Vec3::from(v)), hi.max(Vec3::from(v))),
        );
        let body = col::Body {
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            flags: 0,
            vertices,
            triangles: vec![
                col::Node::Inner {
                    bounds: col::Bounds {
                        center: ((lo + hi) / 2.0).into(),
                        half: ((hi - lo) / 2.0).into(),
                    },
                    right: 2,
                },
                triangle([0, 1, 2]),
                triangle([0, 2, 3]),
            ],
            points: Vec::new(),
        };
        walls.push(Solid::new(&body, Mat3::IDENTITY, Vec3::ZERO));
    }
    walls
}

/// The solids of a scene's placed objects (0x4f1c60). Only a model of one
/// frame is solid, and not the stadium (`skip`); its collision is the
/// `.col` of the same name, or else its own faces (0x4336e0).
pub fn solids(
    placements: &[Placement],
    colseg: &rs2::Archive,
    models: &[rs2::Archive],
    skip: Option<&str>,
) -> World {
    let mut world = World::default();
    let mut cache: Vec<(String, Option<col::Collision>)> = Vec::new();
    let stem_of = |name: &str| {
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        stem.trim().to_ascii_lowercase()
    };
    let skip = skip.map(stem_of);
    for p in placements {
        let stem = stem_of(&p.model);
        if skip.as_deref() == Some(&*stem) {
            continue;
        }
        let i = match cache.iter().position(|(n, _)| *n == stem) {
            Some(i) => i,
            None => {
                let model = models
                    .iter()
                    .find_map(|m| m.get(&format!("{stem}.slb")))
                    .and_then(|data| slb::parse(data).ok());
                let parsed = model.filter(|m| m.frames.len() == 1).and_then(|model| {
                    let name = format!("{stem}.col");
                    colseg
                        .entries()
                        .iter()
                        .find(|e| e.name.to_ascii_lowercase() == name)
                        .and_then(|e| col::parse(colseg.data(e)).ok())
                        .or_else(|| mesh_collision(&model))
                });
                cache.push((stem, parsed));
                cache.len() - 1
            }
        };
        let Some(collision) = &cache[i].1 else {
            continue;
        };
        let look = Vec3::from(p.look).normalize_or(Vec3::Z);
        let up = Vec3::from(p.up).normalize_or(Vec3::Y);
        let right = up.cross(look).normalize_or(Vec3::X);
        let rotation = Mat3::from_cols(right, look.cross(right), look);
        let scale = Vec3::from(p.scale);
        // Parts hanging from a model's frames belong to animated models.
        for part in collision.parts.iter().filter(|part| part.frame.is_none()) {
            let body = scaled(&part.body, scale);
            world
                .solids
                .push(Solid::new(&body, rotation, Vec3::from(p.position)));
        }
    }
    world
}

/// A model's own faces as its collision (0x4336e0, 0x42d7e0): the first
/// level of detail, each face facing the way its vertices' normals do.
fn mesh_collision(model: &slb::Model) -> Option<col::Collision> {
    let lod = model.lods.first()?;
    let mut vertices: Vec<Vec3> = Vec::new();
    let mut faces = Vec::new();
    for s in &lod.surfaces {
        let base = vertices.len();
        vertices.extend(s.positions.iter().map(|&p| Vec3::from(p)));
        for f in &s.faces {
            let [a, b, c] = f.map(|i| base + i as usize);
            let normal: Vec3 = f
                .iter()
                .filter_map(|&i| s.normals.get(i as usize))
                .map(|&n| Vec3::from(n))
                .sum();
            faces.push(([a, b, c], normal));
        }
    }
    if faces.is_empty() || vertices.len() > u16::MAX as usize {
        return None;
    }
    let mut leaves: Vec<col::Triangle> = Vec::new();
    for ([a, b, c], normal) in faces {
        let (pa, pb, pc) = (vertices[a], vertices[b], vertices[c]);
        let n = (pb - pa).cross(pc - pa).normalize_or_zero();
        if n == Vec3::ZERO {
            continue;
        }
        let n = if n.dot(normal) < 0.0 { -n } else { n };
        leaves.push(col::Triangle {
            normal: n.into(),
            d: -n.dot(pa),
            vertices: [a as u16, b as u16, c as u16],
        });
    }
    if leaves.is_empty() {
        return None;
    }
    let mut triangles = Vec::with_capacity(leaves.len() * 2);
    tree(&mut triangles, &mut leaves, &vertices);
    refit(&mut triangles, 0, &vertices);
    Some(col::Collision {
        parts: vec![col::Part {
            frame: None,
            body: Body {
                matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
                flags: 0,
                vertices: vertices.iter().map(|v| v.to_array()).collect(),
                triangles,
                points: Vec::new(),
            },
        }],
    })
}

/// Lays `leaves` out as a tree, split in half along the longest side of
/// their centres; [`refit`] sizes the boxes after.
fn tree(nodes: &mut Vec<Node<col::Triangle>>, leaves: &mut [col::Triangle], vertices: &[Vec3]) {
    if let [one] = leaves {
        nodes.push(Node::Leaf(*one));
        return;
    }
    let centre = |t: &col::Triangle| {
        t.vertices
            .iter()
            .map(|&v| vertices[v as usize])
            .sum::<Vec3>()
    };
    let (lo, hi) = leaves
        .iter()
        .map(centre)
        .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), c| (lo.min(c), hi.max(c)));
    let size = hi - lo;
    let axis = if size.x >= size.y && size.x >= size.z {
        0
    } else if size.y >= size.z {
        1
    } else {
        2
    };
    leaves.sort_by(|a, b| centre(a)[axis].total_cmp(&centre(b)[axis]));
    let at = nodes.len();
    let empty = Bounds {
        center: [0.0; 3],
        half: [0.0; 3],
    };
    nodes.push(Node::Inner {
        bounds: empty,
        right: 0,
    });
    let (left, right) = leaves.split_at_mut(leaves.len() / 2);
    tree(nodes, left, vertices);
    let r = nodes.len() as u32;
    nodes[at] = Node::Inner {
        bounds: empty,
        right: r,
    };
    tree(nodes, right, vertices);
}

/// `body` with its own matrix and `scale` applied to its vertices, so it
/// can be placed with a plain rotation.
fn scaled(body: &Body, scale: Vec3) -> Body {
    let m = &body.matrix;
    let own = Mat3::from_cols_array(&[m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8]]);
    let own_pos = Vec3::new(m[9], m[10], m[11]);
    let vertices: Vec<Vec3> = body
        .vertices
        .iter()
        .map(|&v| (own * Vec3::from(v) + own_pos) * scale)
        .collect();
    let mut triangles = body.triangles.clone();
    for node in &mut triangles {
        if let Node::Leaf(t) = node {
            let [a, b, c] = t.vertices.map(|v| vertices[v as usize]);
            let n = (b - a).cross(c - a).normalize_or_zero();
            // Keep the face's side: a mirrored scale flips the winding.
            let n = if n.dot(own * Vec3::from(t.normal) * scale.signum()) < 0.0 {
                -n
            } else {
                n
            };
            t.normal = n.into();
            t.d = -n.dot(a);
        }
    }
    if !triangles.is_empty() {
        refit(&mut triangles, 0, &vertices);
    }
    Body {
        matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        flags: body.flags,
        vertices: vertices.iter().map(|v| v.to_array()).collect(),
        triangles,
        points: Vec::new(),
    }
}

/// Sets each box in the tree under node `i` to fit its triangles, and
/// returns the corners of node `i`'s box.
fn refit(nodes: &mut [Node<col::Triangle>], i: usize, vertices: &[Vec3]) -> (Vec3, Vec3) {
    match nodes[i] {
        Node::Leaf(t) => t
            .vertices
            .iter()
            .map(|&v| vertices[v as usize])
            .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), v| (lo.min(v), hi.max(v))),
        Node::Inner { right, .. } => {
            let (alo, ahi) = refit(nodes, i + 1, vertices);
            let (blo, bhi) = refit(nodes, right as usize, vertices);
            let (lo, hi) = (alo.min(blo), ahi.max(bhi));
            nodes[i] = Node::Inner {
                bounds: Bounds {
                    center: ((lo + hi) / 2.0).into(),
                    half: ((hi - lo) / 2.0).into(),
                },
                right,
            };
            (lo, hi)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box model, 8 ft along x, 3 ft tall, 4 ft along z, with outward
    /// vertex normals and faces wound either way.
    fn bale() -> slb::Model {
        let (x, y, z) = (4.0f32, 3.0f32, 2.0f32);
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut faces = Vec::new();
        let sides: [([f32; 3], [[f32; 3]; 4]); 6] = [
            (
                [1.0, 0.0, 0.0],
                [[x, 0.0, -z], [x, y, -z], [x, y, z], [x, 0.0, z]],
            ),
            (
                [-1.0, 0.0, 0.0],
                [[-x, 0.0, -z], [-x, 0.0, z], [-x, y, z], [-x, y, -z]],
            ),
            (
                [0.0, 1.0, 0.0],
                [[-x, y, -z], [-x, y, z], [x, y, z], [x, y, -z]],
            ),
            (
                [0.0, -1.0, 0.0],
                [[-x, 0.0, -z], [x, 0.0, -z], [x, 0.0, z], [-x, 0.0, z]],
            ),
            (
                [0.0, 0.0, 1.0],
                [[-x, 0.0, z], [x, 0.0, z], [x, y, z], [-x, y, z]],
            ),
            (
                [0.0, 0.0, -1.0],
                [[-x, 0.0, -z], [-x, y, -z], [x, y, -z], [x, 0.0, -z]],
            ),
        ];
        for (n, corners) in sides {
            let base = positions.len() as u16;
            positions.extend(corners);
            normals.extend([n; 4]);
            faces.push([base, base + 1, base + 2]);
            faces.push([base, base + 2, base + 3]);
        }
        let count = positions.len();
        slb::Model {
            frames: vec![slb::Frame {
                name: "Bale".into(),
                matrix: [
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                ],
                parent: None,
            }],
            materials: Vec::new(),
            lods: vec![slb::Lod {
                surfaces: vec![slb::Surface {
                    positions,
                    normals,
                    uvs: vec![[0.0; 2]; count],
                    colors: vec![[255; 4]; count],
                    faces,
                    materials: vec![0],
                    groups: vec![(0, 0)],
                }],
            }],
        }
    }

    #[test]
    fn a_model_without_a_col_is_solid_by_its_faces() {
        let collision = mesh_collision(&bale()).unwrap();
        let solid = Solid::new(
            &collision.parts[0].body,
            Mat3::IDENTITY,
            Vec3::new(100.0, 0.0, 100.0),
        );
        // Rolling into the end: pushed back out of it.
        let hit = solid.sphere(Vec3::new(105.0, 1.5, 100.0), 1.13).unwrap();
        assert!(hit.normal.abs_diff_eq(Vec3::X, 1e-4), "{:?}", hit.normal);
        // Already well inside, near the end: out the near end, not
        // through the far side.
        let hit = solid.sphere(Vec3::new(103.0, 1.5, 100.5), 1.5).unwrap();
        assert!(hit.normal.abs_diff_eq(Vec3::X, 1e-4), "{:?}", hit.normal);
        assert!((hit.depth - 2.5).abs() < 1e-4);
        // On top: a floor.
        let hit = solid.sphere(Vec3::new(100.0, 3.5, 100.0), 1.0).unwrap();
        assert!(hit.normal.abs_diff_eq(Vec3::Y, 1e-4));
    }
    use col::Triangle;

    #[test]
    fn scaled_floor_stays_solid() {
        let tri = |v: [u16; 3]| {
            Node::Leaf(Triangle {
                normal: [0.0, 1.0, 0.0],
                d: 0.0,
                vertices: v,
            })
        };
        let body = Body {
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0, 0.0],
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
        };
        // Three times as wide, twice as high: the floor is at y = 4, out to 15.
        let s = Solid::new(
            &scaled(&body, Vec3::new(3.0, 2.0, 3.0)),
            Mat3::IDENTITY,
            Vec3::ZERO,
        );
        let hit = s.sphere(Vec3::new(12.0, 4.5, 0.0), 1.0).unwrap();
        assert!((hit.depth - 0.5).abs() < 1e-5, "{hit:?}");
        assert!(hit.normal.abs_diff_eq(Vec3::Y, 1e-5));
        assert!(s.sphere(Vec3::new(17.0, 4.5, 0.0), 1.0).is_none());
    }
}

#[cfg(test)]
mod wall_tests {
    use super::*;

    #[test]
    fn stadium_walls_hold() {
        let seg = "[Segment Data]\n182.38,75.405\n0.897,-156.282\n-130.336,-163.178\n-150.851,-150.851\n-163.178,-130.336\n";
        let frame = scn::StadiumFrame {
            top: 500.0,
            bottom: 0.0,
            scale: 3.0,
            position: [1152.0, 4.0, 1152.0],
        };
        let world = World {
            solids: stadium_walls(seg, &frame),
            ..Default::default()
        };
        let at = |z: f32| world.sphere(Vec3::new(880.0, 25.0, z), 1.5);
        assert!(at(672.0).is_none());
        // Touching from inside, and just through it, both push back in.
        assert!(at(669.0).unwrap().normal.z > 0.9);
        let past = at(666.0).unwrap();
        assert!(past.normal.z > 0.9 && past.depth > 2.5);
        assert!(at(660.0).is_none());
    }
}
