//! `.col` collision models, packed in `RES/COLSEG.RES` (track objects) and
//! `RES/BIKE.RES` (bikes and `Rider.col`). The game finds a model's
//! collision by swapping its extension for `.col` (0x508c30).
//!
//! Recovered from the loaders in the game (`CollisionObject.cpp`, 0x4337c0,
//! bodies at 0x43adb0 / 0x43ae70 / 0x43aff0, trees at 0x42f010, 0x42eec0
//! and 0x42f0b0). All values are little-endian.
//!
//! ```text
//! u32 kind                    0 = one body, 1 = parts
//! kind 0: Body
//! kind 1: u32 count, u32 (unused), count × { u32 frame, Body }
//!
//! Body:
//!   f32 matrix[12]            3x3 rotation, then translation
//!   u32 flags
//!   u32 has_mesh
//!   if has_mesh: u32 vertex_count, f32 vertex[vertex_count][3], TriNode
//!   u32 has_points
//!   if has_points: PointNode
//!
//! TriNode:   f32 key; key >= 0: f32 box[6], TriNode, TriNode
//!                     key <  0: f32 d, f32 normal[3], u16 vertex[3]
//! PointNode: f32 key; key >= 0: f32 box[6], PointNode, PointNode
//!                     key <  0: f32 point[3]
//! ```
//!
//! A box is a centre and half-extents, and an inner node's key is the
//! box's volume. A triangle lies in the plane `normal · p + d = 0`. Parts
//! hang from the model's frame of that index (`Rider.col` has 6, the bikes
//! 3). The point trees hold the corners the game tests against other
//! objects.

use crate::reader::Reader;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Collision {
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// The model frame this part follows; `None` for a single-body file.
    pub frame: Option<u32>,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    /// 3x3 rotation in rows, then the translation.
    pub matrix: [f32; 12],
    pub flags: u32,
    pub vertices: Vec<[f32; 3]>,
    /// The triangle tree, root first; empty when the body has no mesh.
    pub triangles: Vec<TriNode>,
    /// The point tree, root first; may be empty.
    pub points: Vec<PointNode>,
}

/// An axis-aligned box as the file stores it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub center: [f32; 3],
    pub half: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Triangle {
    pub normal: [f32; 3],
    pub d: f32,
    pub vertices: [u16; 3],
}

/// A node of a tree, flattened: an inner node's children are the next
/// node and the node at `right`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Node<T> {
    Inner { bounds: Bounds, right: u32 },
    Leaf(T),
}

pub type TriNode = Node<Triangle>;
pub type PointNode = Node<[f32; 3]>;

/// Deeper than any shipped tree; stops a corrupt file recursing forever.
const MAX_DEPTH: u32 = 64;
const MAX_VERTICES: u32 = 1 << 16;

pub fn parse(bytes: &[u8]) -> Result<Collision> {
    let mut r = Reader::new(bytes, 0);
    let parts = match r.u32("col kind")? {
        0 => vec![Part {
            frame: None,
            body: body(&mut r)?,
        }],
        1 => {
            let count = r.u32("part count")?;
            if count > 256 {
                return Err(Error::Corrupt("col part count"));
            }
            r.u32("col unused")?;
            (0..count)
                .map(|_| {
                    let frame = Some(r.u32("part frame")?);
                    Ok(Part {
                        frame,
                        body: body(&mut r)?,
                    })
                })
                .collect::<Result<_>>()?
        }
        _ => return Err(Error::Corrupt("col kind")),
    };
    if r.pos != bytes.len() {
        return Err(Error::Corrupt("col has trailing bytes"));
    }
    Ok(Collision { parts })
}

fn body(r: &mut Reader) -> Result<Body> {
    let mut matrix = [0.0; 12];
    for m in &mut matrix {
        *m = r.f32("col matrix")?;
    }
    let flags = r.u32("col flags")?;
    let (mut vertices, mut triangles) = (Vec::new(), Vec::new());
    if r.u32("has mesh")? != 0 {
        let count = r.u32("vertex count")?;
        if count > MAX_VERTICES {
            return Err(Error::Corrupt("col vertex count"));
        }
        vertices = (0..count).map(|_| vec3(r)).collect::<Result<_>>()?;
        tree(r, &mut triangles, 0, &mut |r| {
            let d = r.f32("triangle plane")?;
            let normal = vec3(r)?;
            let v = [r.u16("triangle")?, r.u16("triangle")?, r.u16("triangle")?];
            if v.iter().any(|&i| u32::from(i) >= count) {
                return Err(Error::Corrupt("col triangle vertex"));
            }
            Ok(Triangle {
                normal,
                d,
                vertices: v,
            })
        })?;
    }
    let mut points = Vec::new();
    if r.u32("has points")? != 0 {
        tree(r, &mut points, 0, &mut vec3)?;
    }
    Ok(Body {
        matrix,
        flags,
        vertices,
        triangles,
        points,
    })
}

fn tree<T>(
    r: &mut Reader,
    out: &mut Vec<Node<T>>,
    depth: u32,
    leaf: &mut impl FnMut(&mut Reader) -> Result<T>,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::Corrupt("col tree too deep"));
    }
    if r.f32("tree key")? < 0.0 {
        out.push(Node::Leaf(leaf(r)?));
        return Ok(());
    }
    let bounds = Bounds {
        center: vec3(r)?,
        half: vec3(r)?,
    };
    let at = out.len();
    out.push(Node::Inner { bounds, right: 0 });
    tree(r, out, depth + 1, leaf)?;
    let right = out.len() as u32;
    out[at] = Node::Inner { bounds, right };
    tree(r, out, depth + 1, leaf)
}

fn vec3(r: &mut Reader) -> Result<[f32; 3]> {
    Ok([r.f32("vector")?, r.f32("vector")?, r.f32("vector")?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(out: &mut Vec<u8>, values: &[f32]) {
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn u32s(out: &mut Vec<u8>, values: &[u32]) {
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    /// One triangle under one inner node, and a single point.
    fn sample_body(out: &mut Vec<u8>) {
        put(
            out,
            &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        );
        u32s(out, &[0, 1, 3]);
        put(out, &[0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0]);
        put(out, &[1.0, 0.5, 1.0, 0.5, 0.5, 0.0, 0.5]);
        put(out, &[-1.0, -1.0, 0.0, 1.0, 0.0]);
        for i in [0u16, 1, 2] {
            out.extend_from_slice(&i.to_le_bytes());
        }
        put(out, &[-1.0]); // the right child: a second copy of the leaf
        put(out, &[-1.0, 0.0, 1.0, 0.0]);
        for i in [0u16, 1, 2] {
            out.extend_from_slice(&i.to_le_bytes());
        }
        u32s(out, &[1]);
        put(out, &[-1.0, 0.25, 0.5, 0.75]);
    }

    #[test]
    fn parses_single_body() {
        let mut b = Vec::new();
        u32s(&mut b, &[0]);
        sample_body(&mut b);
        let col = parse(&b).unwrap();
        let body = &col.parts[0].body;
        assert_eq!(col.parts[0].frame, None);
        assert_eq!(body.vertices.len(), 3);
        assert_eq!(body.triangles.len(), 3);
        assert!(matches!(body.triangles[0], Node::Inner { right: 2, .. }));
        let Node::Leaf(t) = body.triangles[1] else {
            panic!("expected a leaf")
        };
        assert_eq!(
            (t.d, t.normal, t.vertices),
            (-1.0, [0.0, 1.0, 0.0], [0, 1, 2])
        );
        assert_eq!(body.points, vec![Node::Leaf([0.25, 0.5, 0.75])]);
    }

    #[test]
    fn parses_parts_and_rejects_bad_data() {
        let mut b = Vec::new();
        u32s(&mut b, &[1, 2, 0, 7]);
        sample_body(&mut b);
        u32s(&mut b, &[9]);
        sample_body(&mut b);
        let col = parse(&b).unwrap();
        assert_eq!(
            col.parts.iter().map(|p| p.frame).collect::<Vec<_>>(),
            [Some(7), Some(9)]
        );

        assert!(matches!(
            parse(&b[..b.len() - 1]),
            Err(Error::Truncated { .. })
        ));
        b.push(0);
        assert_eq!(
            parse(&b).unwrap_err(),
            Error::Corrupt("col has trailing bytes")
        );
        assert_eq!(
            parse(&[5, 0, 0, 0]).unwrap_err(),
            Error::Corrupt("col kind")
        );
    }
}
