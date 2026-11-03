//! `.slb` static models, packed in `RES/MODELS.RES`. Scenes name them as
//! `.slt` (the text source format); the game loads the `.slb` build of the
//! same name when there is one.
//!
//! Recovered from the model loader in the game (0x4408f0, frames at
//! 0x502a90, materials at 0x5042e0). The file is a dump of the loader's
//! structures, so some fields hold leftover pointers.
//!
//! ```text
//! u32 frame_count
//! frames[n]      name[128], f32 matrix[16], u32, u32, f32[3], f32[3],
//!                u32, f32[3], f32[3]
//! i32 parent[n]  // -1 for a root frame
//! u32 material_count
//! materials[n]   texture[64], u32[15], u16, u32
//! u32 lod_count, u32 has_distances, f32 distances[lod_count - 1] if set
//! lods[n]
//!     u32 surface_count
//!     surfaces[n]
//!         u32 groups, u32 vertices, u32 faces, u32 materials
//!         vertex[vertices]      x, y, z, u32, u32 diffuse, u32, u, v
//!         vertex[vertices]      a second copy
//!         f32 normal[vertices][3]
//!         u16 face[faces][3]
//!         f32 uv[vertices][2]
//!         u32 material[materials]
//!         u8 [groups][20]       // pointers
//!         groups[n]             u32 frame, u32 first_vertex
//! ```

use crate::reader::Reader;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct Model {
    pub frames: Vec<Frame>,
    pub materials: Vec<Material>,
    /// Levels of detail, most detailed first.
    pub lods: Vec<Lod>,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub name: String,
    /// Transform relative to the parent, row-vector convention
    /// (translation in elements 12..15).
    pub matrix: [f32; 16],
    pub parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Material {
    /// Texture file name, empty when untextured.
    pub texture: String,
}

#[derive(Debug, Clone)]
pub struct Lod {
    pub surfaces: Vec<Surface>,
}

#[derive(Debug, Clone)]
pub struct Surface {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Vertex colours as BGRA bytes.
    pub colors: Vec<[u8; 4]>,
    pub faces: Vec<[u16; 3]>,
    /// Indices into [`Model::materials`].
    pub materials: Vec<u32>,
    /// Vertex ranges and the frame each one is attached to, as
    /// `(frame, first_vertex)` in ascending vertex order.
    pub groups: Vec<(u32, u32)>,
}

impl Surface {
    /// The frame that vertex `v` belongs to.
    pub fn frame_of(&self, v: usize) -> usize {
        self.groups
            .iter()
            .rev()
            .find(|g| g.1 as usize <= v)
            .or(self.groups.first())
            .map_or(0, |g| g.0 as usize)
    }
}

impl Model {
    /// Each frame's transform to model space.
    pub fn world_matrices(&self) -> Vec<[f32; 16]> {
        let mut out: Vec<Option<[f32; 16]>> = vec![None; self.frames.len()];
        for i in 0..self.frames.len() {
            self.resolve(i, &mut out, 0);
        }
        out.into_iter().map(|m| m.unwrap_or(IDENTITY)).collect()
    }

    fn resolve(&self, i: usize, out: &mut [Option<[f32; 16]>], depth: usize) -> [f32; 16] {
        if let Some(m) = out[i] {
            return m;
        }
        let local = self.frames[i].matrix;
        let m = match self.frames[i].parent {
            Some(p) if p < self.frames.len() && depth < self.frames.len() => {
                mul(&local, &self.resolve(p, out, depth + 1))
            }
            _ => local,
        };
        out[i] = Some(m);
        m
    }
}

const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Row-major `a * b`.
pub(crate) fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut m = [0.0; 16];
    for r in 0..4 {
        for c in 0..4 {
            m[r * 4 + c] = (0..4).map(|k| a[r * 4 + k] * b[k * 4 + c]).sum();
        }
    }
    m
}

fn c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Upper bounds that keep a corrupt count from allocating wildly.
const MAX_ITEMS: u32 = 1 << 20;

fn count(r: &mut Reader, what: &'static str) -> Result<usize> {
    let n = r.u32(what)?;
    if n > MAX_ITEMS {
        return Err(Error::Corrupt(what));
    }
    Ok(n as usize)
}

pub fn parse(bytes: &[u8]) -> Result<Model> {
    let mut r = Reader::new(bytes, 0);

    let frame_count = count(&mut r, "frame count")?;
    let mut frames = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        let name = c_string(r.take(128, "frame name")?);
        let mut matrix = [0.0; 16];
        for m in &mut matrix {
            *m = r.f32("frame matrix")?;
        }
        r.take(4 + 4 + 12 + 12 + 4 + 12 + 12, "frame bounds")?;
        frames.push(Frame {
            name,
            matrix,
            parent: None,
        });
    }
    for f in &mut frames {
        let parent = r.u32("frame parent")?;
        f.parent = (parent != u32::MAX).then_some(parent as usize);
    }

    let material_count = count(&mut r, "material count")?;
    let mut materials = Vec::with_capacity(material_count);
    for _ in 0..material_count {
        let texture = c_string(r.take(64, "material texture")?);
        r.take(15 * 4 + 2 + 4, "material fields")?;
        let texture = if texture.eq_ignore_ascii_case("PROCEDURAL") {
            String::new()
        } else {
            texture
        };
        materials.push(Material { texture });
    }

    let lod_count = count(&mut r, "LOD count")?;
    if r.u32("LOD distance flag")? != 0 {
        r.take(lod_count.saturating_sub(1) * 4, "LOD distances")?;
    }
    let mut lods = Vec::with_capacity(lod_count);
    for _ in 0..lod_count {
        let surface_count = count(&mut r, "surface count")?;
        let mut surfaces = Vec::with_capacity(surface_count);
        for _ in 0..surface_count {
            surfaces.push(read_surface(&mut r)?);
        }
        lods.push(Lod { surfaces });
    }

    Ok(Model {
        frames,
        materials,
        lods,
    })
}

fn read_surface(r: &mut Reader) -> Result<Surface> {
    let group_count = count(r, "group count")?;
    let vertex_count = count(r, "vertex count")?;
    let face_count = count(r, "face count")?;
    let material_count = count(r, "surface material count")?;

    let mut positions = Vec::with_capacity(vertex_count);
    let mut colors = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        let v = r.take(32, "vertex")?;
        let f = |i: usize| f32::from_le_bytes([v[i], v[i + 1], v[i + 2], v[i + 3]]);
        positions.push([f(0), f(4), f(8)]);
        colors.push([v[16], v[17], v[18], v[19]]);
    }
    r.take(vertex_count * 32, "vertex copy")?;
    let mut normals = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        normals.push([r.f32("normal")?, r.f32("normal")?, r.f32("normal")?]);
    }
    let mut faces = Vec::with_capacity(face_count);
    for _ in 0..face_count {
        let face = [r.u16("face")?, r.u16("face")?, r.u16("face")?];
        if face.iter().any(|&i| i as usize >= vertex_count) {
            return Err(Error::Corrupt("face index out of range"));
        }
        faces.push(face);
    }
    let mut uvs = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        uvs.push([r.f32("uv")?, r.f32("uv")?]);
    }
    let mut materials = Vec::with_capacity(material_count);
    for _ in 0..material_count {
        materials.push(r.u32("surface material")?);
    }
    r.take(group_count * 20, "group pointers")?;
    let mut groups = Vec::with_capacity(group_count);
    for _ in 0..group_count {
        groups.push((r.u32("group frame")?, r.u32("group first vertex")?));
    }

    Ok(Surface {
        positions,
        normals,
        uvs,
        colors,
        faces,
        materials,
        groups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(out: &mut Vec<u8>, name: &str) {
        let mut n = [0u8; 128];
        n[..name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&n);
        for v in IDENTITY {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&[0; 60]);
    }

    #[test]
    fn parses_triangle() {
        let mut b = Vec::new();
        b.extend_from_slice(&1u32.to_le_bytes());
        frame(&mut b, "Tri");
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        let mut tex = [0u8; 64];
        tex[..7].copy_from_slice(b"tri.tga");
        b.extend_from_slice(&tex);
        b.extend_from_slice(&[0; 66]);
        // One LOD with no distances, one surface.
        for v in [1u32, 0, 1, 1, 3, 1, 1] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for copy in 0..2 {
            for i in 0..3 {
                for f in [i as f32, 1.0, 2.0] {
                    b.extend_from_slice(&f.to_le_bytes());
                }
                b.extend_from_slice(&[0xcd; 4]);
                b.extend_from_slice(&[255; 4]);
                b.extend_from_slice(&[copy; 4]);
                b.extend_from_slice(&[0; 8]);
            }
        }
        for _ in 0..3 {
            for f in [0.0f32, 1.0, 0.0] {
                b.extend_from_slice(&f.to_le_bytes());
            }
        }
        for i in [0u16, 1, 2] {
            b.extend_from_slice(&i.to_le_bytes());
        }
        b.extend_from_slice(&[0; 24]);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&[0; 20]);
        b.extend_from_slice(&[0; 8]);

        let m = parse(&b).unwrap();
        assert_eq!(m.frames[0].name, "Tri");
        assert_eq!(m.frames[0].parent, None);
        assert_eq!(m.materials[0].texture, "tri.tga");
        let s = &m.lods[0].surfaces[0];
        assert_eq!(s.positions[2], [2.0, 1.0, 2.0]);
        assert_eq!(s.faces, vec![[0, 1, 2]]);
        assert_eq!(s.groups, vec![(0, 0)]);
        assert_eq!(s.frame_of(2), 0);
        assert_eq!(m.world_matrices()[0], IDENTITY);
    }

    #[test]
    fn rejects_bad_face() {
        assert!(parse(&[0xff; 8]).is_err());
    }
}
