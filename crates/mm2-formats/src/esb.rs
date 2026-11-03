//! `.esb` eco-systems: the trees and bushes scattered over an outdoor
//! track, built from its `.est` by the game's editor and shipped in the
//! track's `.ENV`. The models live in `RES/ECO.RES`.
//!
//! Recovered from the loader (0x459d30) and the placement reader
//! (0x459b60):
//!
//! ```text
//! u32 method                       // 2: authored, with the extra fields
//! species[256]
//!     u8 present
//!     if present:
//!         str name, str billboard  // u8 length, then the bytes
//!         f32 height, min, max     // +0x180..0x188
//!         f32 width, min, max      // +0x18c..0x194
//!         f32[5]                   // billboard rectangle in the LOD3 picture
//!         f32[2], u32 colour
//!         if method 2: f32[9], u8 present, str
//!         u32 shapes
//!         shape[n]                 // collision (0x458c90)
//!             u32 kind, f32 a[3], f32 b[3], f32 radius, f32 height
//! u8 present, str placement map
//! if method 2: u32, u8 present, str probability map
//! u32 count
//! object[count]
//!     u8 species, u16 x, u16 y, u16 z, u8 height, u8 width
//! ```
//!
//! An object's position is its three `u16`s times the terrain's larger
//! span over 65536 (0x45a290); its height and width bytes pick between
//! the species' sizes (0x456d50, 0x456db0).

use crate::reader::Reader;
use crate::Result;

#[derive(Debug, Clone)]
pub struct EcoSystem {
    pub species: Vec<Species>,
    pub objects: Vec<Object>,
}

#[derive(Debug, Clone)]
pub struct Species {
    /// Its slot, which objects name.
    pub slot: u8,
    /// The model in `ECO.RES`, without its extension.
    pub name: String,
    pub billboard: String,
    /// Height: middle, at byte 0 and 255; smallest, at 128 going down;
    /// largest, at 128 going up.
    pub height: [f32; 3],
    pub width: [f32; 3],
    pub shapes: Vec<Shape>,
}

/// One of a species' collision shapes, at its middle height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// A 16-sided upright prism from the ground (kind 1): solid.
    Trunk { radius: f32, height: f32 },
    /// A ball (kind 2).
    Ball { centre: [f32; 3], radius: f32 },
    /// A capsule between two points (kind 3).
    Capsule { a: [f32; 3], b: [f32; 3], radius: f32 },
    /// The model's own faces (kind 0).
    Mesh,
}

impl Shape {
    /// Whether the bikes hit it (the game marks prisms and meshes 1001,
    /// balls and capsules 1000; only trunks stop a bike).
    pub fn solid(&self) -> bool {
        matches!(self, Shape::Trunk { .. } | Shape::Mesh)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Object {
    pub species: u8,
    /// Position in units of the terrain's larger span / 65536.
    pub position: [u16; 3],
    pub height: u8,
    pub width: u8,
}

impl Species {
    /// The height an object's byte picks (0x456d50).
    pub fn height_of(&self, b: u8) -> f32 {
        pick(self.height, b)
    }

    /// The width an object's byte picks (0x456db0).
    pub fn width_of(&self, b: u8) -> f32 {
        pick(self.width, b)
    }
}

fn pick([mid, low, high]: [f32; 3], b: u8) -> f32 {
    const STEP: f32 = 1.0 / 128.0;
    if b < 0x80 {
        mid - (mid - low) * b as f32 * STEP
    } else {
        (high - mid) * (0xff - b as u32) as f32 * STEP + mid
    }
}

fn string(r: &mut Reader) -> Result<String> {
    let n = r.u8("esb string length")? as usize;
    let bytes = r.take(n, "esb string")?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(n);
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

pub fn parse(bytes: &[u8]) -> Result<EcoSystem> {
    let mut r = Reader::new(bytes, 0);
    let method = r.u32("esb method")?;
    let mut species = Vec::new();
    for slot in 0..=255u8 {
        if r.u8("esb species present")? == 0 {
            continue;
        }
        let name = string(&mut r)?;
        let billboard = string(&mut r)?;
        let mut f = [0.0f32; 13];
        for x in &mut f {
            *x = r.f32("esb species")?;
        }
        r.u32("esb colour")?;
        if method == 2 {
            for _ in 0..9 {
                r.f32("esb species extra")?;
            }
            if r.u8("esb species texture present")? != 0 {
                string(&mut r)?;
            }
        }
        let count = r.u32("esb shapes")?;
        let mut shapes = Vec::new();
        for _ in 0..count {
            let kind = r.u32("esb shape kind")?;
            let mut v = [0.0f32; 8];
            for x in &mut v {
                *x = r.f32("esb shape")?;
            }
            let (a, b) = ([v[0], v[1], v[2]], [v[3], v[4], v[5]]);
            shapes.push(match kind {
                1 => Shape::Trunk {
                    radius: v[6],
                    height: v[7],
                },
                2 => Shape::Ball {
                    centre: a,
                    radius: v[6],
                },
                3 => Shape::Capsule { a, b, radius: v[6] },
                _ => Shape::Mesh,
            });
        }
        species.push(Species {
            slot,
            name,
            billboard,
            height: [f[0], f[1], f[2]],
            width: [f[3], f[4], f[5]],
            shapes,
        });
    }
    if r.u8("esb placement map present")? != 0 {
        string(&mut r)?;
    }
    if method == 2 {
        r.u32("esb")?;
        if r.u8("esb probability map present")? != 0 {
            string(&mut r)?;
        }
    }
    let count = r.u32("esb objects")?;
    let mut objects = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let species = r.u8("esb object")?;
        let position = [
            r.u16("esb object")?,
            r.u16("esb object")?,
            r.u16("esb object")?,
        ];
        objects.push(Object {
            species,
            position,
            height: r.u8("esb object")?,
            width: r.u8("esb object")?,
        });
    }
    Ok(EcoSystem { species, objects })
}

impl EcoSystem {
    pub fn species(&self, slot: u8) -> Option<&Species> {
        self.species.iter().find(|s| s.slot == slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_pick_between_the_three() {
        let s = [35.0, 30.0, 40.0];
        assert_eq!(pick(s, 0), 35.0);
        assert_eq!(pick(s, 0xff), 35.0);
        assert!((pick(s, 0x80) - 40.0).abs() < 0.3);
        assert!((pick(s, 0x7f) - 30.0).abs() < 0.1);
    }
}
