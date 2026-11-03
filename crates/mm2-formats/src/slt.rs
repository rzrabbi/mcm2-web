//! `.slt` static models in text: the source form of `.slb`, which the
//! game reads when no `.slb` was built (the eco-system models in
//! `RES/ECO.RES`). An ini file:
//!
//! ```text
//! [Materials]              NumberOfMaterials
//! [Material - i]           TextureMap
//! [Object Hierarchy]       name, parent, 3×4 matrix rows (one line a frame)
//! [LOD Information]        NumberOfLOD
//! [LOD l - Surface s]      NumberOfVertices, NumberOfFaces, Material#k
//! [... - Vertices]         x, y, z, nx, ny, nz, u, v, w, f32[3], r, g, b
//! [... - Faces]            a, b, c
//! [... - Object Pointer List]  first vertex, vertex count (a line a frame)
//! ```

use crate::slb::{Frame, Lod, Material, Model, Surface};
use crate::{Error, Result};

const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

pub fn parse(bytes: &[u8]) -> Result<Model> {
    let text = String::from_utf8_lossy(bytes);
    let mut materials = Vec::new();
    let mut frames: Vec<(Frame, String)> = Vec::new();
    let mut lods: Vec<Lod> = Vec::new();
    let mut section = String::new();
    let numbers = |line: &str| -> Vec<f32> {
        line.split(',')
            .filter_map(|x| x.trim().parse::<f32>().ok())
            .collect()
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_string();
            if let Some(rest) = name.strip_prefix("LOD ") {
                let mut words = rest.split(" - ");
                let lod: usize = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                while lods.len() <= lod {
                    lods.push(Lod {
                        surfaces: Vec::new(),
                    });
                }
                if let Some(s) = words.next().and_then(|w| w.strip_prefix("Surface ")) {
                    let s: usize = s.parse().unwrap_or(0);
                    let surfaces = &mut lods[lod].surfaces;
                    while surfaces.len() <= s {
                        surfaces.push(Surface {
                            positions: Vec::new(),
                            normals: Vec::new(),
                            uvs: Vec::new(),
                            colors: Vec::new(),
                            faces: Vec::new(),
                            materials: Vec::new(),
                            groups: Vec::new(),
                        });
                    }
                }
            }
            continue;
        }
        let surface = || -> Option<(usize, usize)> {
            let rest = section.strip_prefix("LOD ")?;
            let mut words = rest.split(" - ");
            let lod = words.next()?.parse().ok()?;
            let s = words.next()?.strip_prefix("Surface ")?.parse().ok()?;
            Some((lod, s))
        };
        if section.starts_with("Material - ") {
            if let Some(t) = line.strip_prefix("TextureMap=") {
                // As in `.slb`: the bike's number plate is PROCEDURAL.
                let t = t.trim();
                materials.push(Material {
                    texture: if t.eq_ignore_ascii_case("PROCEDURAL") { String::new() } else { t.to_string() },
                });
            }
        } else if section == "Object Hierarchy" {
            let mut parts = line.splitn(3, ',');
            let (Some(name), Some(parent), Some(rest)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let m = numbers(rest);
            if m.len() < 12 {
                continue;
            }
            // Rows of the rotation, then the translation.
            let matrix = [
                m[0], m[1], m[2], 0.0, m[3], m[4], m[5], 0.0, m[6], m[7], m[8], 0.0, m[9], m[10],
                m[11], 1.0,
            ];
            frames.push((
                Frame {
                    name: name.trim().to_string(),
                    matrix,
                    parent: None,
                },
                parent.trim().to_string(),
            ));
        } else if let Some((lod, s)) = surface() {
            let target = &mut lods[lod].surfaces[s];
            if section.ends_with("- Vertices") {
                let v = numbers(line);
                if v.len() < 8 {
                    return Err(Error::Corrupt("slt vertex too short"));
                }
                target.positions.push([v[0], v[1], v[2]]);
                target.normals.push([v[3], v[4], v[5]]);
                target.uvs.push([v[6], v[7]]);
                let c = |i: usize| (v.get(i).copied().unwrap_or(1.0).clamp(0.0, 1.0) * 255.0) as u8;
                target.colors.push([c(14), c(13), c(12), 255]);
            } else if section.ends_with("- Faces") {
                let f = numbers(line);
                if f.len() >= 3 {
                    target.faces.push([f[0] as u16, f[1] as u16, f[2] as u16]);
                }
            } else if section.ends_with("- Object Pointer List") {
                let g = numbers(line);
                // One line a frame, in the hierarchy's order: its first
                // vertex and how many there are.
                if g.len() >= 2 {
                    let frame = target.groups.len() as u32;
                    target.groups.push((frame, g[0] as u32));
                }
            } else if let Some(k) = line.strip_prefix("Material#") {
                if let Some((_, m)) = k.split_once('=') {
                    target.materials.push(m.trim().parse().unwrap_or(0));
                }
            }
        }
    }
    let names: Vec<String> = frames.iter().map(|(f, _)| f.name.clone()).collect();
    let frames = frames
        .into_iter()
        .map(|(mut f, parent)| {
            f.parent = names.iter().position(|n| n.eq_ignore_ascii_case(&parent));
            f
        })
        .collect();
    let frames: Vec<Frame> = frames;
    // Text vertices sit in their own frame; `.slb` keeps them in the model's
    // space. Children of the root are moved out to it (a bike's bars and
    // wheels); the root's own place stays the placement's business.
    let relative: Vec<[f32; 16]> = (0..frames.len())
        .map(|i| {
            let mut m = IDENTITY;
            let mut at = Some(i);
            for _ in 0..frames.len() {
                let Some(f) = at.and_then(|f| frames.get(f)) else { break };
                if f.parent.is_none() {
                    break;
                }
                m = crate::slb::mul(&m, &f.matrix);
                at = f.parent;
            }
            m
        })
        .collect();
    for lod in &mut lods {
        for s in &mut lod.surfaces {
            if s.groups.is_empty() {
                s.groups.push((0, 0));
            }
            for v in 0..s.positions.len() {
                let Some(m) = relative.get(s.frame_of(v)).filter(|m| **m != IDENTITY) else {
                    continue;
                };
                let apply = |p: [f32; 3], w: f32| {
                    std::array::from_fn(|c| p[0] * m[c] + p[1] * m[4 + c] + p[2] * m[8 + c] + w * m[12 + c])
                };
                s.positions[v] = apply(s.positions[v], 1.0);
                if let Some(n) = s.normals.get_mut(v) {
                    *n = apply(*n, 0.0);
                }
            }
        }
    }
    Ok(Model {
        frames,
        materials,
        lods,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_small_model() {
        let text = b"[Materials]\r\nNumberOfMaterials=1\r\n[Material - 0]\r\nTextureMap=Tropics03.tga\r\n\
[Object Hierarchy]\r\nPalm01,NONE,1,0,0,0,1,0,0,0,1,0,0,0\r\n[LOD Information]\r\nNumberOfLOD=1\r\n\
[LOD 0]\r\nNumberOfSurfaces=1\r\n[LOD 0 - Surface 0]\r\nNumberOfVertices=3\r\nNumberOfFaces=1\r\n\
NumberOfMaterials=1\r\nMaterial#0=0\r\n[LOD 0 - Surface 0 - Vertices]\r\n\
0,0,0,0,1,0,0.25,0.5,0.5,0,1,0,1,1,1\r\n1,0,0,0,1,0,1,0.5,0.5,0,1,0,1,1,1\r\n0,2,0,0,1,0,0,1,0.5,0,1,0,1,1,1\r\n\
[LOD 0 - Surface 0 - Faces]\r\n0,2,1\r\n[LOD 0 - Surface 0 - Object Pointer List]\r\n0,3\r\n";
        let m = super::parse(text).unwrap();
        assert_eq!(m.materials[0].texture, "Tropics03.tga");
        assert_eq!(m.frames.len(), 1);
        let s = &m.lods[0].surfaces[0];
        assert_eq!(s.positions.len(), 3);
        assert_eq!(s.uvs[0], [0.25, 0.5]);
        assert_eq!(s.faces, vec![[0, 2, 1]]);
        assert_eq!(s.materials, vec![0]);
        assert_eq!(s.groups, vec![(0, 0)]);
    }
}
