//! Track objects: the static models a scene places, baked into one mesh,
//! and its animated models, whose parts move with their motions. Drawn in
//! one call per texture.

use crate::hires::{self, HiRes, Image};
use crate::morph::Morphs;
use crate::renderer::from_game;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use mm2_formats::anim::{Mcf, Motion};
use mm2_formats::scn::{Animation, Lighting, Placement};
use mm2_formats::trn::Texture;
use mm2_formats::{rs2, slb, texture};
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    color: [u8; 4],
    /// Which transform in the node buffer moves this vertex; 0 is the
    /// identity used by static objects. The top two bits say how it is
    /// lit: [`UNLIT`] or [`PLANT`].
    node: u32,
}

/// A vertex of a model with `UseLighting=F`: its stored colour only.
const UNLIT: u32 = 1 << 31;
/// A vertex of an eco-system plant: lit with the sun level (0x4577d0).
const PLANT: u32 = 1 << 30;
const NODE: u32 = PLANT - 1;

/// A block's bounds and indices.
type Block = (Vec3, Vec3, std::ops::Range<u32>);

/// The still objects' blocks are this many ft a side.
const BLOCK: f32 = 160.0;

/// All of a track's objects, ready to upload.
#[derive(Default)]
pub struct ObjectMesh {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    /// One draw per texture: its index range and the texture (if found).
    draws: Vec<(std::ops::Range<u32>, Option<Image>)>,
    /// Each draw's texture name.
    names: Vec<String>,
    /// Whether the enhanced drawing lays photo materials over the objects
    /// (materials::surfaced).
    pub surfaced: bool,
    /// Each draw's triangles in blocks of the track (crate::cull): each
    /// block's bounds (drawing coordinates) and its indices. The moving
    /// models' triangles are a block that is always drawn.
    blocks: Vec<Vec<Block>>,
    pub animated: Vec<AnimatedModel>,
    /// For each animated model, its index in the animations it was built
    /// from.
    pub sources: Vec<usize>,
    node_count: u32,
    /// The node the enhanced drawing's dressing hangs on, if any.
    extra_node: Option<u32>,
    pub placed: usize,
    pub missing_models: Vec<String>,
}

/// An animated model on the track and the motion it plays.
#[derive(Clone)]
pub struct AnimatedModel {
    /// Each model frame's rest transform relative to its parent (game
    /// coordinates) and its parent.
    frames: Vec<(Mat4, Option<usize>)>,
    /// Each frame's name, for what hangs on it (a scene sound's
    /// `SoundAttachToObject`).
    names: Vec<String>,
    /// Vertices are stored in model space, posed at rest; this undoes each
    /// frame's rest transform before applying the animated one.
    inverse_rest: Vec<Mat4>,
    /// Where the root frames go on the track.
    place: Mat4,
    /// The model's motions by name, with their frame rate and whether
    /// they loop, and the one playing.
    motions: Vec<(String, Motion, f32, bool)>,
    motion: Option<usize>,
    /// When the motion starts playing, in seconds; it holds its first frame
    /// until then.
    motion_start: f32,
    /// Motions mixed at a phase instead (0x4aa2b0, 0x4aa6b0): each one's
    /// index, the phase through it, 0 to 1, its weight and its layer.
    blend: Vec<(usize, f32, f32, u8)>,
    /// Fading in from what played before (0x4aa1e0): that motion, its
    /// start and mix, when the change came and over how long, s. The old
    /// pose is held where it was at the change.
    from: Option<(Option<usize>, f32, Vec<(usize, f32, f32, u8)>, f32, f32)>,
    first_node: u32,
    /// Drawn see-through, as a ghost race's ghost is.
    ghost: bool,
    /// How dirty it is, 0 to 1, for the enhanced drawing.
    mud: f32,
    /// The rider's morphs, the first of its vertices in the mesh and those
    /// vertices at rest.
    morph: Option<(Arc<Morphs>, u32, Arc<Vec<Vertex>>)>,
    /// Frames turned about their own x axis on top of the motion, rad: a
    /// bike's wheels (0x518c40).
    spins: Vec<(usize, f32)>,
}

impl AnimatedModel {
    /// Where the model stands on the track, in game coordinates.
    pub fn position(&self) -> Vec3 {
        self.place.w_axis.truncate()
    }

    /// The frame named `name`, ignoring case.
    pub fn frame_named(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n.eq_ignore_ascii_case(name))
    }

    /// Where `frame` (or the model's root) is at `time`, in game
    /// coordinates, with the motion it plays (0x5017e0 on the frame).
    pub fn frame_position(&self, frame: Option<usize>, time: f32) -> Vec3 {
        let Some(frame) = frame.filter(|&f| f < self.frames.len()) else {
            return self.position();
        };
        let local = self.locals(self.motion, self.motion_start, &self.blend, time);
        let mut world = vec![None; local.len()];
        self.resolve(frame, &local, &mut world, 0).w_axis.truncate()
    }

    /// Writes this model's frame transforms at `time` seconds into `nodes`,
    /// and returns them before the rest pose is undone.
    /// Each frame's transform relative to its parent, with `motion` (from
    /// `motion_start`) or `blend` playing at `time`.
    fn locals(&self, motion: Option<usize>, motion_start: f32, blend: &[(usize, f32, f32, u8)], time: f32) -> Vec<Mat4> {
        let mut local: Vec<Mat4> = self.frames.iter().map(|f| f.0).collect();
        if !blend.is_empty() {
            // Each layer's keys, summed by weight and squared up again; a
            // later layer replaces the frames it moves.
            let layers = blend.iter().map(|b| b.3).max().unwrap_or(0);
            for layer in 0..=layers {
                let mut sums: Vec<Option<([Vec3; 3], f32)>> = vec![None; local.len()];
                for &(m, phase, weight, _) in blend.iter().filter(|b| b.3 == layer) {
                    let motion = &self.motions[m].1;
                    let last = motion.frames.len().saturating_sub(1);
                    let at = phase.clamp(0.0, 1.0) * last as f32;
                    let (a, t) = (at.floor() as usize, at.fract());
                    let b = (a + 1).min(last);
                    for (ka, kb) in motion.frames[a].iter().zip(&motion.frames[b]) {
                        let mix = |p: [f32; 3], q: [f32; 3]| Vec3::from(p).lerp(Vec3::from(q), t) * weight;
                        if let Some(sum) = sums.get_mut(ka.frame) {
                            let (s, w) = sum.get_or_insert(([Vec3::ZERO; 3], 0.0));
                            s[0] += mix(ka.look, kb.look);
                            s[1] += mix(ka.up, kb.up);
                            s[2] += mix(ka.position, kb.position);
                            *w += weight;
                        }
                    }
                }
                for (m, sum) in local.iter_mut().zip(sums) {
                    if let Some(([look, up, position], w)) = sum {
                        *m = orientation(look.into(), up.into(), Vec3::ONE, (position / w.max(1e-6)).into());
                    }
                }
            }
        } else if let Some((_, motion, fps, looping)) = motion.map(|m| &self.motions[m]) {
            // Between the two keys either side of the time (the motion
            // player 0x4a8720 finds both with 0x4a6db0 and mixes them).
            let count = motion.frames.len();
            let at = ((time - motion_start) * fps).max(0.0);
            let (frame, t) = (at as usize, at.fract());
            let (a, b) = if *looping {
                (frame % count, (frame + 1) % count)
            } else if frame + 1 < count {
                (frame, frame + 1)
            } else {
                (count - 1, count - 1)
            };
            for (ka, kb) in motion.frames[a].iter().zip(&motion.frames[b]) {
                let kb = if kb.frame == ka.frame { kb } else { ka };
                let mix = |p: [f32; 3], q: [f32; 3]| Vec3::from(p).lerp(Vec3::from(q), t).into();
                if let Some(m) = local.get_mut(ka.frame) {
                    *m = orientation(mix(ka.look, kb.look), mix(ka.up, kb.up), Vec3::ONE, mix(ka.position, kb.position));
                }
            }
        }
        for &(f, angle) in &self.spins {
            if let Some(m) = local.get_mut(f) {
                *m *= Mat4::from_rotation_x(angle);
            }
        }
        local
    }

    fn pose(&self, time: f32, nodes: &mut [Mat4]) -> Vec<Mat4> {
        let world = self.posed(time);
        let flip = Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0));
        for (i, m) in world.iter().enumerate() {
            let mut node = flip * *m * self.inverse_rest[i];
            // The transforms are affine, so the shader takes the unused
            // corners as the see-through mark and how dirty it is.
            node.x_axis.w = if self.ghost { 1.0 } else { 0.0 };
            node.y_axis.w = self.mud;
            nodes[self.first_node as usize + i] = node;
        }
        world
    }

    /// Each frame's transform at `time` on the track, game coordinates,
    /// with any fade from the motion before.
    fn posed(&self, time: f32) -> Vec<Mat4> {
        let mut local = self.locals(self.motion, self.motion_start, &self.blend, time);
        if let Some((motion, start, blend, at, span)) = &self.from {
            let t = (time - at) / span;
            if t < 1.0 {
                let old = self.locals(*motion, *start, blend, *at);
                for (m, o) in local.iter_mut().zip(old) {
                    let mix = |a: Vec4, b: Vec4| b.lerp(a, t.max(0.0)).truncate().into();
                    *m = orientation(mix(m.z_axis, o.z_axis), mix(m.y_axis, o.y_axis), Vec3::ONE, mix(m.w_axis, o.w_axis));
                }
            }
        }
        let mut world: Vec<Option<Mat4>> = vec![None; local.len()];
        for i in 0..local.len() {
            self.resolve(i, &local, &mut world, 0);
        }
        world.into_iter().map(|m| m.unwrap_or_default()).collect()
    }

    fn resolve(&self, i: usize, local: &[Mat4], world: &mut [Option<Mat4>], depth: usize) -> Mat4 {
        if let Some(m) = world[i] {
            return m;
        }
        let m = match self.frames[i].1 {
            Some(p) if p < local.len() && depth < local.len() => {
                self.resolve(p, local, world, depth + 1) * local[i]
            }
            _ => self.place * local[i],
        };
        world[i] = Some(m);
        m
    }
}

impl ObjectMesh {
    /// Bakes every placement whose model is in `models` and sets up every
    /// animation whose `.mcf` is in `objects`, with textures from the first
    /// of `textures` (named by their archive file) that has them, or their
    /// replacements in `hires`.
    pub fn build(
        placements: &[Placement],
        extras: &[Placement],
        movable: &[Placement],
        animations: &[Animation],
        skins: &[Option<String>],
        models: &[rs2::Archive],
        textures: &[(&str, rs2::Archive)],
        hires: Option<&HiRes>,
    ) -> ObjectMesh {
        let mut mesh = ObjectMesh {
            node_count: 1,
            ..Default::default()
        };
        let mut cache: HashMap<String, Option<slb::Model>> = HashMap::new();
        let mut load = |name: &str| {
            // Track makers' scenes may name a model with the folder it
            // was made in (`SLTFiles\ramp_TD_001.slt`); the packs hold
            // bare names.
            let name = bare(name);
            let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
            // One scene has a tab before the extension.
            let key = format!("{}.slb", stem.trim()).to_ascii_lowercase();
            cache
                .entry(key.clone())
                .or_insert_with(|| {
                    // The eco-system's models only come as text, and so do
                    // the bikes players install over the stock ones (a
                    // `yamaha.slt` added beside the built `yamaha.slb`):
                    // the text one wins.
                    let text = format!("{}.slt", stem.trim()).to_ascii_lowercase();
                    if let Some(data) = models.iter().find_map(|m| m.get(&text)) {
                        return mm2_formats::slt::parse(data)
                            .map_err(|err| eprintln!("{name}: {err}"))
                            .ok();
                    }
                    let data = models.iter().find_map(|m| m.get(&key))?;
                    slb::parse(data)
                        .map_err(|err| eprintln!("{name}: {err}"))
                        .ok()
                })
                .clone()
        };
        // Indices for each texture name, merged across every model.
        let mut batches: HashMap<String, Vec<u32>> = HashMap::new();

        for p in placements {
            let Some(model) = load(&p.model) else {
                mesh.missing(&p.model);
                continue;
            };
            mesh.add(&model, Some(placement_matrix(p)), 0, None, p.lighting, &mut batches);
            mesh.placed += 1;
        }

        // The enhanced drawing's dressing (scenery.rs): placed like the
        // rest, but on a node of its own so it can be hidden.
        if !extras.is_empty() {
            let node = mesh.node_count;
            mesh.node_count += 1;
            mesh.extra_node = Some(node);
            for p in extras {
                let Some(model) = load(&p.model) else {
                    mesh.missing(&p.model);
                    continue;
                };
                mesh.add(&model, Some(placement_matrix(p)), node, None, p.lighting, &mut batches);
                mesh.placed += 1;
            }
        }

        let find = |name: &str| textures.iter().find_map(|(_, t)| t.get(bare(name)));
        let mut morphs: Option<Arc<Morphs>> = None;
        for (i, a) in animations.iter().enumerate() {
            let Some(mcf) = find(&a.model).and_then(|b| Mcf::parse(b).ok()) else {
                mesh.missing(&a.model);
                continue;
            };
            let Some(model) = load(&mcf.model) else {
                mesh.missing(&mcf.model);
                continue;
            };
            let motions: Vec<_> = mcf
                .motions
                .iter()
                .filter_map(|info| {
                    let motion = Motion::parse(find(&info.file)?)
                        .map_err(|err| eprintln!("{}: {err}", info.file))
                        .ok()?;
                    (!motion.frames.is_empty())
                        .then(|| (info.name.clone(), motion, info.fps, info.looping))
                })
                .collect();
            let named = |name: &String| motions.iter().position(|m| m.0.eq_ignore_ascii_case(name));
            let motion = a
                .motion
                .as_ref()
                .and_then(named)
                .or((!motions.is_empty()).then_some(0));
            let first_node = mesh.node_count;
            mesh.node_count += model.frames.len() as u32;
            let skin = skins.get(i).and_then(Option::as_deref);
            let first_vertex = mesh.vertices.len();
            mesh.add(&model, None, first_node, skin, a.lighting, &mut batches);
            // The game gives the rider its morphs (0x490c30).
            let morph = mcf
                .model
                .to_ascii_lowercase()
                .starts_with("rider")
                .then(|| find("ridermorph.mbf"))
                .flatten()
                .map(|text| {
                    let names: Vec<&str> = model.frames.iter().map(|f| f.name.as_str()).collect();
                    Morphs::parse(&String::from_utf8_lossy(text), &names)
                })
                .filter(|m| !m.is_empty())
                .map(|m| {
                    // Every rider shares the shapes; each keeps its vertices.
                    let shared = morphs.get_or_insert_with(|| Arc::new(m)).clone();
                    let rest = mesh.vertices[first_vertex..].to_vec();
                    (shared, first_vertex as u32, Arc::new(rest))
                });
            mesh.sources.push(i);
            mesh.animated.push(AnimatedModel {
                frames: model
                    .frames
                    .iter()
                    .map(|f| (Mat4::from_cols_array(&f.matrix), f.parent))
                    .collect(),
                names: model.frames.iter().map(|f| f.name.clone()).collect(),
                inverse_rest: model
                    .world_matrices()
                    .iter()
                    .map(|m| Mat4::from_cols_array(m).inverse())
                    .collect(),
                place: orientation(a.look, a.up, Vec3::from(a.scale), a.offset),
                motions,
                motion,
                motion_start: 0.0,
                blend: Vec::new(),
                from: None,
                first_node,
                ghost: false,
                mud: 0.0,
                morph,
                spins: Vec::new(),
            });
            mesh.placed += 1;
        }

        // Models that stay still but can be moved or hidden (the gates of a
        // Baja course) follow one node each, after the animations.
        for (j, p) in movable.iter().enumerate() {
            let Some(model) = load(&p.model) else {
                mesh.missing(&p.model);
                continue;
            };
            let first_node = mesh.node_count;
            mesh.node_count += model.frames.len() as u32;
            mesh.add(&model, None, first_node, None, p.lighting, &mut batches);
            mesh.sources.push(animations.len() + j);
            mesh.animated.push(AnimatedModel {
                frames: model
                    .frames
                    .iter()
                    .map(|f| (Mat4::from_cols_array(&f.matrix), f.parent))
                    .collect(),
                names: model.frames.iter().map(|f| f.name.clone()).collect(),
                inverse_rest: model
                    .world_matrices()
                    .iter()
                    .map(|m| Mat4::from_cols_array(m).inverse())
                    .collect(),
                place: placement_matrix(p),
                motions: Vec::new(),
                motion: None,
                motion_start: 0.0,
                blend: Vec::new(),
                from: None,
                first_node,
                ghost: false,
                mud: 0.0,
                morph: None,
                spins: Vec::new(),
            });
            mesh.placed += 1;
        }

        let mut names: Vec<_> = batches.keys().cloned().collect();
        names.sort();
        for name in names {
            let start = mesh.indices.len() as u32;
            mesh.add_blocks(&batches[&name]);
            let tex = match name.strip_prefix(PLATE) {
                // Plates are painted from the digits texture, so they stay
                // the game's own.
                Some(n) => find("BikeNumbers128.tga")
                    .and_then(|e| texture::parse(e).ok())
                    .map(|digits| Image::from_texture(&plate(&digits, n.parse().unwrap_or(0)))),
                None => textures
                    .iter()
                    .find_map(|(archive, t)| Some((*archive, t.get(bare(&name))?)))
                    .or_else(|| {
                        // Drawn white; say so, so a player's log shows it.
                        if !name.is_empty() {
                            eprintln!("{name}: texture not found");
                        }
                        None
                    })
                    .and_then(|(archive, e)| {
                        let tex = texture::parse(e)
                            .map_err(|err| eprintln!("{name}: {err}"))
                            .ok()?;
                        Some(hires::image(hires, &hires::model_key(archive, &name), &tex))
                    }),
            };
            mesh.draws.push((start..mesh.indices.len() as u32, tex));
            mesh.names.push(name);
        }
        mesh
    }

    /// The highest upward-facing surface of the still models under each
    /// point (game x, z), at most `top` high: the stands of a stadium, for
    /// the enhanced drawing's crowd.
    pub fn surface_below(&self, points: &[glam::Vec2], top: f32) -> Vec<Option<f32>> {
        const CELL: f32 = 8.0;
        let key = |x: f32, z: f32| ((x / CELL).floor() as i32, (z / CELL).floor() as i32);
        let mut grid: HashMap<(i32, i32), Vec<[Vec3; 3]>> = HashMap::new();
        // Only what the points can stand on: their bounds.
        let (lo, hi) = points.iter().fold((glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        for tri in self.indices.chunks_exact(3) {
            let v = [tri[0], tri[1], tri[2]].map(|i| &self.vertices[i as usize]);
            if v.iter().any(|v| v.node & NODE != 0) {
                continue;
            }
            // Back to game coordinates.
            let p = v.map(|v| Vec3::new(v.position[0], v.position[1], -v.position[2]));
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            if n.length_squared() < 1e-8 || n.normalize().y.abs() < 0.5 {
                continue;
            }
            let (a, b) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
            if b.x < lo.x || a.x > hi.x || b.z < lo.y || a.z > hi.y || a.y > top {
                continue;
            }
            let ((x0, z0), (x1, z1)) = (key(a.x, a.z), key(b.x, b.z));
            if (x1 - x0 + 1) * (z1 - z0 + 1) > 400 {
                continue;
            }
            for x in x0..=x1 {
                for z in z0..=z1 {
                    grid.entry((x, z)).or_default().push(p);
                }
            }
        }
        points
            .iter()
            .map(|q| {
                let mut best: Option<f32> = None;
                for t in grid.get(&key(q.x, q.y)).into_iter().flatten() {
                    let (a, b, c) = (t[0], t[1], t[2]);
                    let d = (b.z - c.z) * (a.x - c.x) + (c.x - b.x) * (a.z - c.z);
                    if d.abs() < 1e-6 {
                        continue;
                    }
                    let w0 = ((b.z - c.z) * (q.x - c.x) + (c.x - b.x) * (q.y - c.z)) / d;
                    let w1 = ((c.z - a.z) * (q.x - c.x) + (a.x - c.x) * (q.y - c.z)) / d;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let y = a.y * w0 + b.y * w1 + c.y * w2;
                    if y <= top && best.is_none_or(|h| y > h) {
                        best = Some(y);
                    }
                }
                best
            })
            .collect()
    }

    /// Adds one draw's triangles, sorted into blocks of the track, each
    /// keeping its triangles in their order, and the moving models' last.
    fn add_blocks(&mut self, indices: &[u32]) {
        let still = |v: &Vertex| {
            let n = v.node & NODE;
            n == 0 || Some(n) == self.extra_node
        };
        let key = |tri: &[u32]| {
            let v = tri.iter().map(|&i| &self.vertices[i as usize]);
            if !v.clone().all(still) {
                return (i32::MAX, i32::MAX);
            }
            let mid = v.map(|v| Vec3::from(v.position)).sum::<Vec3>() / 3.0;
            ((mid.z / BLOCK).floor() as i32, (mid.x / BLOCK).floor() as i32)
        };
        let mut tris: Vec<((i32, i32), &[u32])> = indices.chunks_exact(3).map(|t| (key(t), t)).collect();
        tris.sort_by_key(|t| t.0);
        let mut blocks: Vec<Block> = Vec::new();
        let mut last = None;
        for (k, tri) in tris {
            let at = self.indices.len() as u32;
            if last != Some(k) {
                let (lo, hi) = match k.0 {
                    i32::MAX => (Vec3::splat(f32::MIN), Vec3::splat(f32::MAX)),
                    _ => (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                };
                blocks.push((lo, hi, at..at));
                last = Some(k);
            }
            let b = blocks.last_mut().unwrap();
            if k.0 != i32::MAX {
                for &i in tri {
                    let p = Vec3::from(self.vertices[i as usize].position);
                    b.0 = b.0.min(p);
                    b.1 = b.1.max(p);
                }
            }
            self.indices.extend_from_slice(tri);
            b.2.end = at + 3;
        }
        self.blocks.push(blocks);
    }

    fn missing(&mut self, name: &str) {
        if !self.missing_models.iter().any(|m| m == name) {
            self.missing_models.push(name.to_string());
        }
    }

    /// Adds a model's most detailed LOD. With `place`, the model is static
    /// and baked into track space; without, its vertices stay in model
    /// space and follow their frames' nodes from `first_node`. A `skin`
    /// replaces the texture of the model's first material, as the game does
    /// for rider outfits; [`SECOND`] and a texture replace its second's.
    fn add(
        &mut self,
        model: &slb::Model,
        place: Option<Mat4>,
        first_node: u32,
        skin: Option<&str>,
        lighting: Lighting,
        batches: &mut HashMap<String, Vec<u32>>,
    ) {
        let mode = match lighting {
            Lighting::Lit => 0,
            Lighting::Off => UNLIT,
            Lighting::Plant => PLANT,
        };
        let Some(lod) = model.lods.first() else {
            return;
        };
        for s in &lod.surfaces {
            let base = self.vertices.len() as u32;
            // `#plate:<number>[:<paint>]` or just the paint.
            let (plate, paint) = match skin.and_then(|s| s.strip_prefix(PLATE)) {
                Some(rest) => match rest.split_once(':') {
                    Some((n, paint)) => (Some(n), Some(paint)),
                    None => (Some(rest), None),
                },
                None => (None, skin),
            };
            let (slot, paint) = match paint.and_then(|p| p.strip_prefix(SECOND)) {
                Some(p) => (1, Some(p)),
                None => (0, paint),
            };
            let mut texture = match (s.materials.first(), paint) {
                (Some(&m), Some(paint)) if m == slot => paint.to_ascii_lowercase(),
                (first, _) => first
                    .and_then(|&i| model.materials.get(i as usize))
                    .map(|m| m.texture.to_ascii_lowercase())
                    .unwrap_or_default(),
            };
            // The bike's untextured material is the game's PROCEDURAL one:
            // the rider's number, painted for each bike.
            let numbered = plate.is_some() && texture.is_empty();
            if let (true, Some(n)) = (numbered, plate) {
                texture = format!("{PLATE}{n}");
            }
            // The game lights models itself and overwrites the stored
            // colours, which are leftovers, unless the scene turns its
            // lighting off; untextured ones keep them as their material.
            let textured = !texture.is_empty() && lighting != Lighting::Off;
            for (v, pos) in s.positions.iter().enumerate() {
                let n = s.normals.get(v).copied().unwrap_or([0.0, 1.0, 0.0]);
                let c = s.colors[v];
                let frame = s.frame_of(v);
                let (position, normal, node) = match place {
                    // Vertices are already in model space.
                    Some(place) => (
                        from_game(place.transform_point3(Vec3::from(*pos))),
                        from_game(place.transform_vector3(Vec3::from(n))),
                        first_node,
                    ),
                    None => (Vec3::from(*pos), Vec3::from(n), first_node + frame as u32),
                };
                self.vertices.push(Vertex {
                    position: position.into(),
                    normal: normal.normalize_or_zero().into(),
                    // The front plate shows the numbers smaller, in the
                    // middle (0x418470).
                    uv: if numbered && pos[2] >= -0.5 {
                        [s.uvs[v][0] * 0.65 + 0.175, s.uvs[v][1] * 0.65 + 0.11]
                    } else {
                        s.uvs[v]
                    },
                    // The numbers keep their stored black: white digits
                    // tinted to print dark on the plates.
                    color: if textured && !numbered {
                        [255; 4]
                    } else {
                        [c[2], c[1], c[0], 255]
                    },
                    node: node | mode,
                });
            }
            batches
                .entry(texture)
                .or_default()
                .extend(s.faces.iter().flatten().map(|&i| base + i as u32));
        }
    }
}

/// The game's orientation from look and up vectors: +z along look, +y up.
pub fn orientation(look: [f32; 3], up: [f32; 3], scale: Vec3, position: [f32; 3]) -> Mat4 {
    let look = Vec3::from(look).normalize_or(Vec3::Z);
    let up = Vec3::from(up).normalize_or(Vec3::Y);
    let right = up.cross(look).normalize_or(Vec3::X);
    let up = look.cross(right);
    Mat4::from_cols(
        (right * scale.x).extend(0.0),
        (up * scale.y).extend(0.0),
        (look * scale.z).extend(0.0),
        Vec3::from(position).extend(1.0),
    )
}

fn placement_matrix(p: &Placement) -> Mat4 {
    orientation(p.look, p.up, Vec3::from(p.scale), p.position)
}

/// The shadow's mask: the casters as the sun sees them.
pub const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

pub struct ObjectRenderer {
    pipeline: wgpu::RenderPipeline,
    /// The player's shadow (0x4df2f0 adds the bike and rider to the
    /// ProjectedShadow): their triangles drawn from the sun into the
    /// shadow's mask.
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_globals: wgpu::BindGroup,
    shadow_uniform: wgpu::Buffer,
    /// The casters' triangles, per texture draw.
    shadow: Option<(wgpu::Buffer, Vec<(std::ops::Range<u32>, usize)>)>,
    /// The mesh's indices and each vertex's node, to pick casters from.
    cpu_indices: Vec<u32>,
    vertex_nodes: Vec<u32>,
    draw_ranges: Vec<std::ops::Range<u32>>,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    globals: wgpu::BindGroup,
    draws: Vec<(std::ops::Range<u32>, wgpu::BindGroup)>,
    /// Each draw's blocks (ObjectMesh::blocks).
    blocks: Vec<Vec<Block>>,
    /// What each view sees ([`ObjectRenderer::cull`]): runs of indices and
    /// their draw, for the camera and each sun cascade.
    seen: Vec<Vec<(std::ops::Range<u32>, usize)>>,
    nodes: wgpu::Buffer,
    animated: Vec<AnimatedModel>,
    node_count: usize,
    /// The dressing's node, and whether it shows.
    extra_node: Option<usize>,
    pub extras_shown: bool,
    /// The enhanced drawing ([`crate::graphics`]), when on.
    enhanced: Option<Enhanced>,
}

/// The objects' part of the enhanced drawing: lit per pixel and shadowed,
/// and drawn from the sun into each shadow cascade.
struct Enhanced {
    lit: wgpu::RenderPipeline,
    depth: wgpu::RenderPipeline,
    /// Each cascade's view from the sun, for drawing it.
    cascades: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// The triangles of the moving models (bikes, riders, people), per
    /// texture draw: the only casters indoors, where the "sun" is the
    /// stadium's lights and the roof would shade the whole floor.
    moving: Option<(wgpu::Buffer, Vec<(std::ops::Range<u32>, usize)>)>,
}

impl ObjectRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &ObjectMesh,
        globals: &wgpu::Buffer,
        format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        samples: u32,
        enhanced: Option<&wgpu::BindGroupLayout>,
    ) -> Option<Self> {
        if mesh.indices.is_empty() {
            return None;
        }
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("object vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("object indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let shader = crate::enhance::shader(device, "objects", include_str!("objects.wgsl"));
        let visible = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object globals"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: visible,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // The enhanced drawing's photo material for the texture.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object shadow"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("objects"),
            bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let shadow_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("object shadow"),
            bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout), Some(&shadow_layout)],
            immediate_size: 0,
        });
        let vertex_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Unorm8x4,
                4 => Uint32
            ],
        })];
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow casters"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_caster"),
                compilation_options: Default::default(),
                buffers: &vertex_layout,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_caster"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SHADOW_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let shadow_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("object shadow"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_globals = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("object shadow"),
            layout: &shadow_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_uniform.as_entire_binding(),
            }],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("objects"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Unorm8x4,
                        4 => Uint32
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            // Some models rely on seeing both sides of a face.
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        let enhanced = enhanced.map(|enh| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("objects enhanced"),
                bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout), None, Some(enh)],
                immediate_size: 0,
            });
            let lit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("objects enhanced"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &vertex_layout,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_enhanced"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                // Leaves and fences fade at their edges rather than cut.
                multisample: wgpu::MultisampleState {
                    count: samples,
                    alpha_to_coverage_enabled: samples > 1,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            });
            let depth = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("objects sun depth"),
                layout: Some(&shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_caster"),
                    compilation_options: Default::default(),
                    buffers: &vertex_layout,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_depth"),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::enhance::SHADOW_DEPTH,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 2.0,
                        clamp: 0.0,
                    },
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
            let cascades = (0..crate::enhance::CASCADES)
                .map(|_| {
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("sun cascade"),
                        size: 64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("sun cascade"),
                        layout: &shadow_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        }],
                    });
                    (buffer, group)
                })
                .collect();
            let mut moving = Vec::new();
            let mut moving_draws = Vec::new();
            for (d, (range, _)) in mesh.draws.iter().enumerate() {
                let start = moving.len() as u32;
                for tri in mesh.indices[range.start as usize..range.end as usize].chunks(3) {
                    if tri.len() == 3 && mesh.vertices[tri[0] as usize].node & NODE != 0 {
                        moving.extend_from_slice(tri);
                    }
                }
                if moving.len() as u32 > start {
                    moving_draws.push((start..moving.len() as u32, d));
                }
            }
            let moving = (!moving.is_empty()).then(|| {
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("moving casters"),
                    contents: bytemuck::cast_slice(&moving),
                    usage: wgpu::BufferUsages::INDEX,
                });
                (buffer, moving_draws)
            });
            Enhanced { lit, depth, cascades, moving }
        });
        let node_count = mesh.node_count.max(1) as usize;
        let nodes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("object nodes"),
            contents: bytemuck::cast_slice(&vec![Mat4::IDENTITY.to_cols_array(); node_count]),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let globals = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("object globals"),
            layout: &globals_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: nodes.as_entire_binding(),
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("object sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: crate::graphics::anisotropy(),
            ..Default::default()
        });
        let white = Image {
            size: 1,
            levels: vec![vec![255; 4]],
        };
        let layout = texture_layout;
        let draws = mesh
            .draws
            .iter()
            .zip(&mesh.names)
            .map(|((range, tex), name)| {
                let tex = upload(device, queue, tex.as_ref().unwrap_or(&white));
                // Its photo material's layer, or -1 for none.
                let surface = mesh.surfaced.then(|| crate::materials::surface(name)).flatten();
                let surface = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("object surface"),
                    contents: bytemuck::cast_slice(&[surface.map_or(-1.0, |m| m as f32), 0.0, 0.0, 0.0]),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("object texture"),
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                &tex.create_view(&Default::default()),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: surface.as_entire_binding(),
                        },
                    ],
                });
                (range.clone(), group)
            })
            .collect();
        Some(ObjectRenderer {
            pipeline,
            shadow_pipeline,
            shadow_globals,
            shadow_uniform,
            shadow: None,
            cpu_indices: mesh.indices.clone(),
            vertex_nodes: mesh.vertices.iter().map(|v| v.node).collect(),
            draw_ranges: mesh.draws.iter().map(|(r, _)| r.clone()).collect(),
            vertices,
            indices,
            globals,
            // Everything, till culled.
            seen: vec![mesh.draws.iter().enumerate().map(|(d, (r, _))| (r.clone(), d)).collect(); 1 + crate::enhance::CASCADES],
            draws,
            blocks: mesh.blocks.clone(),
            nodes,
            animated: mesh.animated.clone(),
            node_count,
            extra_node: mesh.extra_node.map(|n| n as usize),
            extras_shown: true,
            enhanced,
        })
    }

    /// Moves animated model `index` to `place` on the track, in game
    /// coordinates.
    pub fn set_place(&mut self, index: usize, place: Mat4) {
        if let Some(a) = self.animated.get_mut(index) {
            a.place = place;
        }
    }

    /// Draws animated model `index` see-through, or solid again.
    pub fn set_ghost(&mut self, index: usize, ghost: bool) {
        if let Some(a) = self.animated.get_mut(index) {
            a.ghost = ghost;
        }
    }

    /// How dirty animated model `index` is drawn, 0 to 1.
    pub fn set_mud(&mut self, index: usize, mud: f32) {
        if let Some(a) = self.animated.get_mut(index) {
            a.mud = mud;
        }
    }

    /// Plays motion `name` on animated model `index` from `start` seconds,
    /// unless it is already playing.
    pub fn set_motion(&mut self, index: usize, name: &str, start: f32) {
        self.fade_to(index, name, start, 0.0);
    }

    /// As [`set_motion`](Self::set_motion), fading in from the pose before
    /// over `span` seconds (0x4aa1e0).
    pub fn fade_to(&mut self, index: usize, name: &str, start: f32, span: f32) {
        if let Some(a) = self.animated.get_mut(index) {
            let found = a
                .motions
                .iter()
                .position(|m| m.0.eq_ignore_ascii_case(name));
            if found.is_some() && (found != a.motion || !a.blend.is_empty()) {
                a.from = (span > 0.0).then(|| (a.motion, a.motion_start, a.blend.clone(), start, span));
                a.motion = found;
                a.motion_start = start;
            }
            if found.is_some() {
                a.blend.clear();
            }
        }
    }

    /// Mixes motions on animated model `index`: each by name, at a phase
    /// (0 to 1 through it), with a weight, and in a layer (a later layer
    /// replaces the frames it moves). Names it lacks are left out; with
    /// none found the motion playing stays.
    pub fn set_blend(&mut self, index: usize, mix: &[(&str, f32, f32, u8)]) {
        if let Some(a) = self.animated.get_mut(index) {
            let blend: Vec<(usize, f32, f32, u8)> = mix
                .iter()
                .filter(|m| m.2 > 0.0)
                .filter_map(|&(name, phase, weight, layer)| {
                    a.motions
                        .iter()
                        .position(|m| m.0.eq_ignore_ascii_case(name))
                        .map(|i| (i, phase, weight, layer))
                })
                .collect();
            if !blend.is_empty() {
                a.blend = blend;
            }
        }
    }

    /// Whether animated model `index`'s motion has played out by `time`
    /// (the player's +0xc); a looping one never does.
    pub fn motion_done(&self, index: usize, time: f32) -> bool {
        self.animated.get(index).is_some_and(|a| {
            a.motion.is_some_and(|m| {
                let (_, motion, fps, looping) = &a.motions[m];
                !looping && (time - a.motion_start) * fps >= motion.frames.len() as f32
            })
        })
    }

    /// Animated model `index`'s frame called `name`.
    pub fn frame_index(&self, index: usize, name: &str) -> Option<usize> {
        self.animated.get(index)?.frame_named(name)
    }

    /// Where animated model `index`'s frame `name` is at `time`, in game
    /// coordinates.
    pub fn frame_at(&self, index: usize, name: &str, time: f32) -> Option<Vec3> {
        let a = self.animated.get(index)?;
        let frame = a.frame_named(name)?;
        Some(a.frame_position(Some(frame), time))
    }

    /// Where animated model `index`'s frames are at `time`, game
    /// coordinates; `None` for a frame the model lacks.
    pub fn frames_at(&self, index: usize, frames: &[usize], time: f32) -> Vec<Option<Mat4>> {
        let Some(a) = self.animated.get(index) else {
            return vec![None; frames.len()];
        };
        let world = a.posed(time);
        frames.iter().map(|&f| world.get(f).copied()).collect()
    }

    /// Turns animated model `index`'s frame `name` by `angle` about its own
    /// x axis, on top of its motion: a bike's wheel.
    pub fn set_spin(&mut self, index: usize, name: &str, angle: f32) {
        let Some(a) = self.animated.get_mut(index) else { return };
        let Some(frame) = a.frame_named(name) else { return };
        match a.spins.iter_mut().find(|s| s.0 == frame) {
            Some(s) => s.1 = angle,
            None => a.spins.push((frame, angle)),
        }
    }

    /// Holds animated model `index` on its first frame until `time`.
    pub fn set_motion_start(&mut self, index: usize, time: f32) {
        if let Some(a) = self.animated.get_mut(index) {
            a.motion_start = time;
        }
    }

    /// Poses the animated models at `time` seconds.
    pub fn update(&self, queue: &wgpu::Queue, time: f32) {
        if self.animated.is_empty() {
            return;
        }
        let mut nodes = vec![Mat4::IDENTITY; self.node_count];
        if let Some(n) = self.extra_node {
            // Marked still (the spare corner), or folded away to nothing.
            nodes[n] = if self.extras_shown {
                let mut m = Mat4::IDENTITY;
                m.z_axis.w = 1.0;
                m
            } else {
                Mat4::ZERO
            };
        }
        let mut offsets = Vec::new();
        for a in &self.animated {
            let world = a.pose(time, &mut nodes);
            if let Some((morphs, first, rest)) = &a.morph {
                offsets.resize(rest.len(), Vec3::ZERO);
                morphs.apply(&world, &mut offsets);
                let moved: Vec<Vertex> = rest
                    .iter()
                    .zip(&offsets)
                    .map(|(v, o)| Vertex {
                        position: (Vec3::from(v.position) + *o).into(),
                        ..*v
                    })
                    .collect();
                let at = *first as u64 * std::mem::size_of::<Vertex>() as u64;
                queue.write_buffer(&self.vertices, at, bytemuck::cast_slice(&moved));
            }
        }
        let data: Vec<[f32; 16]> = nodes.iter().map(Mat4::to_cols_array).collect();
        queue.write_buffer(&self.nodes, 0, bytemuck::cast_slice(&data));
    }

    /// Picks what view `slot` (0 the camera, 1 on each sun cascade) can
    /// see through `frustum`, for the draws after.
    pub fn cull(&mut self, slot: usize, frustum: &crate::cull::Frustum) {
        let Some(seen) = self.seen.get_mut(slot) else { return };
        seen.clear();
        let mut runs = Vec::new();
        for (d, blocks) in self.blocks.iter().enumerate() {
            crate::cull::runs(blocks.iter().filter(|b| frustum.sees(b.0, b.1)).map(|b| b.2.clone()), &mut runs);
            seen.extend(runs.drain(..).map(|r| (r, d)));
        }
    }

    /// The animated models whose shadow falls: their triangles, by the
    /// nodes their vertices hang on.
    pub fn set_casters(&mut self, device: &wgpu::Device, models: &[usize]) {
        let nodes: Vec<std::ops::Range<u32>> = models
            .iter()
            .filter_map(|&m| self.animated.get(m))
            .map(|a| a.first_node..a.first_node + a.frames.len() as u32)
            .collect();
        let casts = |i: u32| {
            let n = self.vertex_nodes[i as usize] & NODE;
            nodes.iter().any(|r| r.contains(&n))
        };
        let mut indices = Vec::new();
        let mut draws = Vec::new();
        for (d, range) in self.draw_ranges.iter().enumerate() {
            let start = indices.len() as u32;
            for tri in self.cpu_indices[range.start as usize..range.end as usize].chunks(3) {
                if tri.len() == 3 && casts(tri[0]) {
                    indices.extend_from_slice(tri);
                }
            }
            if indices.len() as u32 > start {
                draws.push((start..indices.len() as u32, d));
            }
        }
        self.shadow = (!indices.is_empty()).then(|| {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shadow indices"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            (buffer, draws)
        });
    }

    /// The view from the sun the shadow's mask is drawn in, drawing
    /// coordinates.
    pub fn set_shadow_view(&self, queue: &wgpu::Queue, view_proj: Mat4) {
        queue.write_buffer(&self.shadow_uniform, 0, bytemuck::bytes_of(&view_proj.to_cols_array()));
    }

    /// Draws the casters into the shadow's mask.
    pub fn draw_casters(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some((indices, draws)) = &self.shadow else {
            return;
        };
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(0, &self.globals, &[]);
        pass.set_bind_group(2, &self.shadow_globals, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        for (range, d) in draws {
            pass.set_bind_group(1, &self.draws[*d].1, &[]);
            pass.draw_indexed(range.clone(), 0, 0..1);
        }
    }

    /// Sets each sun cascade's view, drawing coordinates.
    pub fn set_cascades(&self, queue: &wgpu::Queue, views: &[Mat4]) {
        if let Some(e) = &self.enhanced {
            for ((buffer, _), view) in e.cascades.iter().zip(views) {
                queue.write_buffer(buffer, 0, bytemuck::bytes_of(&view.to_cols_array()));
            }
        }
    }

    /// Draws every object, or with `moving_only` only the moving models,
    /// into sun cascade `cascade`'s depth.
    pub fn draw_sun_depth(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, moving_only: bool) {
        let Some(e) = &self.enhanced else { return };
        pass.set_pipeline(&e.depth);
        pass.set_bind_group(0, &self.globals, &[]);
        pass.set_bind_group(2, &e.cascades[cascade].1, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        if moving_only {
            let Some((indices, draws)) = &e.moving else { return };
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            for (range, d) in draws {
                pass.set_bind_group(1, &self.draws[*d].1, &[]);
                pass.draw_indexed(range.clone(), 0, 0..1);
            }
            return;
        }
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        self.draw_seen(pass, 1 + cascade);
    }

    /// Draws what view `slot` sees, each run with its texture.
    fn draw_seen(&self, pass: &mut wgpu::RenderPass<'_>, slot: usize) {
        let mut bound = None;
        for (range, d) in &self.seen[slot] {
            if bound != Some(*d) {
                pass.set_bind_group(1, &self.draws[*d].1, &[]);
                bound = Some(*d);
            }
            pass.draw_indexed(range.clone(), 0, 0..1);
        }
    }

    /// Draws the objects lit and shadowed as the enhanced drawing does,
    /// with its shared group; without it, as the game does.
    pub fn draw_enhanced(&self, pass: &mut wgpu::RenderPass<'_>, shared: &wgpu::BindGroup) {
        let Some(e) = &self.enhanced else {
            return self.draw(pass);
        };
        pass.set_pipeline(&e.lit);
        pass.set_bind_group(3, shared, &[]);
        self.draw_with(pass);
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        self.draw_with(pass);
    }

    fn draw_with(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(0, &self.globals, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        self.draw_seen(pass, 0);
    }
}

/// A skin of `#m1:<texture>` gives a model's second material that texture
/// (the flag girl's white flag).
pub const SECOND: &str = "#m1:";

/// A skin of `#plate:<number>` gives a bike its rider's number, and
/// `#plate:<number>:<paint>` its paint too.
pub const PLATE: &str = "#plate:";

/// A number plate's texture as the game paints it (0x418300, 0x418380,
/// 0x418470): a clear 64×64 surface with the digits of `BikeNumbers128.tga`
/// (white, 32 high, four to a row) copied in, centred.
fn plate(digits: &Texture, number: u32) -> Texture {
    const SIDE: usize = 64;
    // Each digit's width; the 1 is narrower.
    let width = |d: u32| if d == 1 { 13 } else { 19 };
    let number = number.min(999);
    let shown: Vec<u32> = number
        .to_string()
        .bytes()
        .map(|b| (b - b'0') as u32)
        .collect();
    let total: i32 = shown.iter().map(|&d| width(d)).sum();
    let mut x = 28 - total / 2;
    let mut level = vec![0u16; SIDE * SIDE];
    let src = &digits.levels[0];
    let src_side = digits.size as usize;
    for d in shown {
        let (col, row) = ((d % 4) as usize, (d / 4) as usize);
        for y in 0..32 {
            for dx in 0..width(d) {
                let (tx, ty) = (x + 4 + dx, 4 + y as i32);
                if !(0..60).contains(&tx) {
                    continue;
                }
                let (sx, sy) = (col * 32 + dx as usize, row * 32 + y);
                if sx < src_side && sy < src_side {
                    level[ty as usize * SIDE + tx as usize] = src[sy * src_side + sx];
                }
            }
        }
        x += width(d);
    }
    // Mips by halving, channel by channel.
    let mut levels = vec![level];
    while levels.last().unwrap().len() > 1 {
        let prev = levels.last().unwrap();
        let side = (prev.len() as f32).sqrt() as usize;
        let half = side / 2;
        let mut next = Vec::with_capacity(half * half);
        for y in 0..half {
            for x in 0..half {
                let mut sum = [0u32; 4];
                for (ox, oy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = prev[(y * 2 + oy) * side + x * 2 + ox];
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += (p >> (c * 4) & 15) as u32;
                    }
                }
                next.push(
                    sum.iter()
                        .enumerate()
                        .map(|(c, s)| ((s / 4) as u16) << (c * 4))
                        .sum(),
                );
            }
        }
        levels.push(next);
    }
    Texture {
        size: SIDE as u32,
        format: mm2_formats::trn::PixelFormat::Argb4444,
        levels,
    }
}

fn upload(device: &wgpu::Device, queue: &wgpu::Queue, tex: &Image) -> wgpu::Texture {
    let data = tex.levels.concat();
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("object texture"),
            size: wgpu::Extent3d {
                width: tex.size,
                height: tex.size,
                depth_or_array_layers: 1,
            },
            mip_level_count: tex.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &data,
    )
}

/// A file name without the folders before it.
fn bare(name: &str) -> &str {
    name.rsplit(['\\', '/']).next().unwrap_or(name)
}
