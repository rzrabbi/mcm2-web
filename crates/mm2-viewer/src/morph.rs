//! The rider's morphs (`MorphBastardModifier.cpp`): where a joint bends,
//! the skin around it is pulled into shape instead of folding through
//! itself.
//!
//! `RES/BIKE.RES` `ridermorph.mbf` names, for some of the rider's parts
//! (the "object"), the neighbouring part whose turn drives them (the
//! "controller"), the angle about one axis at which a target shape is
//! reached, and that shape as offsets for the part's vertices. The loader
//! (0x4a49a0) gathers the targets of each controller and axis into one
//! channel with a rest key at 0 degrees, sorted by angle. Each frame
//! (0x4a5270) the controller's turn in the object's frame is split into
//! three twists, and every channel blends the two keys either side of its
//! angle; past the last key the last shape holds (0x4a6250).

use glam::{Mat3, Mat4, Vec3};

/// One controller and axis, and its shapes in angle order.
#[derive(Debug, Clone)]
struct Channel {
    controller: usize,
    axis: usize,
    /// Degrees, and offsets for vertex indices; the first is the rest key.
    keys: Vec<(f32, Vec<(usize, Vec3)>)>,
}

#[derive(Debug, Clone)]
struct Bastard {
    object: usize,
    channels: Vec<Channel>,
}

/// A model's morphs, by frame index.
#[derive(Debug, Clone, Default)]
pub struct Morphs {
    bastards: Vec<Bastard>,
}

impl Morphs {
    /// Reads `ridermorph.mbf` for a model with these frame names.
    pub fn parse(text: &str, frames: &[&str]) -> Morphs {
        let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
        for line in text.lines().map(str::trim) {
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                sections.push((name.to_string(), Vec::new()));
            } else if !line.is_empty() {
                if let Some(s) = sections.last_mut() {
                    s.1.push(line);
                }
            }
        }
        let section = |name: &str| {
            sections
                .iter()
                .find(|s| s.0.eq_ignore_ascii_case(name))
                .map(|s| s.1.as_slice())
                .unwrap_or(&[])
        };
        let value = |name: &str, key: &str| {
            section(name).iter().find_map(|l| {
                let (k, v) = l.split_once('=')?;
                k.trim()
                    .eq_ignore_ascii_case(key)
                    .then(|| v.trim().to_string())
            })
        };
        let frame = |name: &str| frames.iter().position(|f| f.eq_ignore_ascii_case(name));
        let count = |name: &str, key: &str| -> usize {
            value(name, key).and_then(|v| v.parse().ok()).unwrap_or(0)
        };

        let mut bastards = Vec::new();
        for b in 0..count("MorphBastard Information", "NumberOfMorphBastards") {
            let head = format!("MorphBastard {b}");
            let Some(object) = value(&head, "ObjectName").and_then(|n| frame(&n)) else {
                continue;
            };
            let mut channels: Vec<Channel> = Vec::new();
            for t in 0..count(&head, "NumberOfTargets") {
                let name = format!("MorphBastard {b} Target {t}");
                let Some(controller) = value(&name, "ControllerName").and_then(|n| frame(&n))
                else {
                    continue;
                };
                let deltas: Vec<(usize, Vec3)> = section(&format!("{name} Deltas"))
                    .iter()
                    .filter_map(|l| {
                        let f: Vec<&str> = l.split(',').map(str::trim).collect();
                        let n = |i: usize| f.get(i)?.parse::<f32>().ok();
                        Some((f.get(1)?.parse().ok()?, Vec3::new(n(2)?, n(3)?, n(4)?)))
                    })
                    .collect();
                for (axis, key) in ["XAxis", "YAxis", "ZAxis"].iter().enumerate() {
                    let Some(angle) = value(&name, key).and_then(|v| v.parse::<f32>().ok()) else {
                        continue;
                    };
                    let i = match channels
                        .iter()
                        .position(|c| c.controller == controller && c.axis == axis)
                    {
                        Some(i) => i,
                        None => {
                            channels.push(Channel {
                                controller,
                                axis,
                                keys: vec![(0.0, Vec::new())],
                            });
                            channels.len() - 1
                        }
                    };
                    channels[i].keys.push((angle, deltas.clone()));
                }
            }
            for c in &mut channels {
                c.keys.sort_by(|a, b| a.0.total_cmp(&b.0));
            }
            bastards.push(Bastard { object, channels });
        }
        Morphs { bastards }
    }

    pub fn is_empty(&self) -> bool {
        self.bastards.is_empty()
    }

    /// The offsets for the model's vertices, with each frame's posed
    /// transform in `world`; `offsets` has one per vertex of the morphed
    /// surface and is overwritten.
    pub fn apply(&self, world: &[Mat4], offsets: &mut [Vec3]) {
        offsets.fill(Vec3::ZERO);
        for b in &self.bastards {
            let Some(object) = world.get(b.object) else {
                continue;
            };
            let to_object = Mat3::from_mat4(*object).transpose();
            for c in &b.channels {
                let Some(controller) = world.get(c.controller) else {
                    continue;
                };
                let turn = to_object * Mat3::from_mat4(*controller);
                let angle = angles(turn)[c.axis];
                for (key, weight) in blend(&c.keys, angle) {
                    for &(v, d) in &c.keys[key].1 {
                        if let Some(o) = offsets.get_mut(v) {
                            *o += d * weight;
                        }
                    }
                }
            }
        }
    }
}

/// The keys either side of `angle` and their weights (0x4a6250): at or
/// below the first key, the first; at or past the last, the last.
fn blend(keys: &[(f32, Vec<(usize, Vec3)>)], angle: f32) -> Vec<(usize, f32)> {
    let (first, last) = (keys[0].0, keys[keys.len() - 1].0);
    if angle <= first {
        return vec![(0, 1.0)];
    }
    if angle >= last {
        return vec![(keys.len() - 1, 1.0)];
    }
    let i = keys
        .windows(2)
        .position(|w| w[0].0 < angle && angle <= w[1].0)
        .unwrap_or(0);
    let t = (angle - keys[i].0) / (keys[i + 1].0 - keys[i].0);
    vec![(i, 1.0 - t), (i + 1, t)]
}

/// The controller's turn in the object's frame as the game splits it
/// (0x4a5270), in degrees: for each axis, the twist left about it once the
/// shortest swing has lined it up. The game files the twist about z under
/// y and the twist about y under z, and negates them all.
pub fn angles(turn: Mat3) -> [f32; 3] {
    let twist = |a: usize| -> f32 {
        let b = (a + 1) % 3;
        let axis = turn.col(a);
        let swing = glam::Quat::from_rotation_arc(Vec3::AXES[a], axis.normalize_or(Vec3::AXES[a]));
        let swung = swing * Vec3::AXES[b];
        let target = turn.col(b);
        let angle = swung.angle_between(target).to_degrees();
        if swung.cross(target).dot(axis) < 0.0 {
            -angle
        } else {
            angle
        }
    };
    [-twist(0), -twist(2), -twist(1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    const MBF: &str = "[MorphBastard Information]\nNumberOfMorphBastards=1\n\
        [MorphBastard 0]\nObjectName=LwrArmL\nNumberOfTargets=2\n\
        [MorphBastard 0 Target 0]\nControllerName=UprArmL\nTargetName=Mid\nXAxis=-40.0\n\
        [MorphBastard 0 Target 1]\nControllerName=UprArmL\nTargetName=TS\nXAxis=-80.0\n\
        [MorphBastard 0 Target 0 Deltas]\n0,1,1.0,0.0,0.0\n\
        [MorphBastard 0 Target 1 Deltas]\n0,1,0.0,2.0,0.0\n";

    #[test]
    fn channels_blend_their_keys() {
        let m = Morphs::parse(MBF, &["UprArmL", "LwrArmL"]);
        assert_eq!(m.bastards.len(), 1);
        let c = &m.bastards[0].channels;
        assert_eq!(c.len(), 1);
        assert_eq!(
            c[0].keys.iter().map(|k| k.0).collect::<Vec<_>>(),
            [-80.0, -40.0, 0.0]
        );
        // Halfway between rest and the first shape.
        assert_eq!(blend(&c[0].keys, -20.0), [(1, 0.5), (2, 0.5)]);
        // Past the last shape, it holds.
        assert_eq!(blend(&c[0].keys, -120.0), [(0, 1.0)]);
        assert_eq!(blend(&c[0].keys, 30.0), [(2, 1.0)]);
    }

    #[test]
    fn twists_split_a_turn() {
        let a = angles(Mat3::from_rotation_x(0.5));
        assert!((a[0] + 0.5f32.to_degrees()).abs() < 1e-3, "{a:?}");
        assert!(a[1].abs() < 1e-3 && a[2].abs() < 1e-3, "{a:?}");
        let a = angles(Mat3::IDENTITY);
        assert_eq!(a, [0.0; 3]);
    }

    /// With the game's files beside the engine, every morph finds its
    /// parts.
    #[test]
    fn rider_morphs_load() {
        let bike =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../GAME/RES/BIKE.RES");
        let Ok(bytes) = crate::read_archive(&bike) else {
            return;
        };
        let ar = mm2_formats::rs2::Archive::parse(&bytes).unwrap();
        let model = mm2_formats::slb::parse(ar.get("rider02.slb").unwrap()).unwrap();
        let names: Vec<&str> = model.frames.iter().map(|f| f.name.as_str()).collect();
        let text = String::from_utf8_lossy(ar.get("ridermorph.mbf").unwrap());
        let m = Morphs::parse(&text, &names);
        assert_eq!(m.bastards.len(), 11);
        let keys: usize = m
            .bastards
            .iter()
            .flat_map(|b| &b.channels)
            .map(|c| c.keys.len() - 1)
            .sum();
        assert!(keys > 20, "{keys}");
    }
}
