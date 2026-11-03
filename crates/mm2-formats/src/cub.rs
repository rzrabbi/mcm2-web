//! Sky boxes (`RES/SKIES.RES`, `*.cub`), named by a scene's `CubeFile`.
//!
//! ```text
//! u32 palettes, u32 cubes           // 1 and 1
//! u32 offset × palettes             // from the file's start: the 8-bit
//! u32 offset × cubes                // cards' palette (not needed here)
//! cube (0x43e1d0):
//!   u32 pixel format (22 = RGB555), u16 flags, u16
//!   u16 tiles present × 6           // bit t: tile t of the face
//!   u32 tile sizes × 6              // 2 bits a tile: 512, 64, 128, 256
//!   f32 × 9                         // origin, look and up
//!   [flags & 4: 32 bytes × 6]
//!   [flags & 1: u32 tile offsets, 16 × 6]
//!   tiles, face by face: u32 (2), mip chain as in model textures
//! ```
//!
//! Each face is four tiles by four, tile `t` at column `t & 3` and row
//! `t >> 2` from the top left; the side faces keep only their top two rows
//! (the sky above the horizon). The faces, as the game lays them out
//! (0x43ec00), look along +z, −x, −z, +x, then up and down.

use crate::reader::Reader;
use crate::trn::{read_mips, PixelFormat, Texture};
use crate::{Error, Result};

/// Tiles across a face.
pub const TILES: usize = 4;

#[derive(Debug, Clone)]
pub struct Cube {
    /// The six faces' tiles, 16 each, `None` where absent.
    pub faces: Vec<Vec<Option<Texture>>>,
}

impl Cube {
    pub fn parse(bytes: &[u8]) -> Result<Cube> {
        let mut r = Reader::new(bytes, 0);
        let palettes = r.u32("palette count")? as usize;
        let cubes = r.u32("cube count")?;
        if cubes == 0 {
            return Err(Error::Corrupt("no sky cube in the file"));
        }
        r.take(4 * palettes, "palette offsets")?;
        let at = r.u32("cube offset")? as usize;
        r.seek(at, "cube")?;
        let format = PixelFormat::from_code(r.u32("cube pixel format")?)
            .ok_or(Error::Corrupt("unknown sky pixel format"))?;
        let flags = r.u16("cube flags")?;
        r.u16("cube field")?;
        let mut present = [0u16; 6];
        for p in &mut present {
            *p = r.u16("cube tiles")?;
        }
        let mut sizes = [0u32; 6];
        for s in &mut sizes {
            *s = r.u32("cube tile sizes")?;
        }
        r.take(36, "cube frame")?;
        if flags & 4 != 0 {
            r.take(32 * 6, "cube face data")?;
        }
        let offsets = if flags & 1 != 0 {
            let mut o = Vec::with_capacity(96);
            for _ in 0..96 {
                o.push(r.u32("cube tile offset")? as usize);
            }
            Some(o)
        } else {
            None
        };
        let mut faces = Vec::with_capacity(6);
        for face in 0..6 {
            let mut tiles = Vec::with_capacity(TILES * TILES);
            for t in 0..TILES * TILES {
                if present[face] & 1 << t == 0 {
                    tiles.push(None);
                    continue;
                }
                if let Some(o) = &offsets {
                    r.seek(at + o[face * 16 + t], "sky tile")?;
                }
                let size = tile_size(sizes[face], t);
                r.u32("sky tile version")?;
                tiles.push(Some(read_mips(&mut r, size, format)?));
            }
            faces.push(tiles);
        }
        Ok(Cube { faces })
    }

    /// One face as an RGBA image, `side` pixels square, tiles stretched to
    /// fit and absent ones clear.
    pub fn face_rgba(&self, face: usize, side: u32) -> Vec<u8> {
        let side = side as usize;
        let tile_side = side / TILES;
        let mut out = vec![0u8; side * side * 4];
        for (t, tile) in self.faces[face].iter().enumerate() {
            let Some(tile) = tile else { continue };
            let src = tile.rgba8(0);
            let n = tile.size as usize;
            let (tx, ty) = ((t % TILES) * tile_side, (t / TILES) * tile_side);
            for y in 0..tile_side {
                for x in 0..tile_side {
                    let s = ((y * n / tile_side) * n + x * n / tile_side) * 4;
                    let d = ((ty + y) * side + tx + x) * 4;
                    out[d..d + 3].copy_from_slice(&src[s..s + 3]);
                    out[d + 3] = 255;
                }
            }
        }
        out
    }

    /// The largest tile, for the face images' size.
    pub fn tile_size(&self) -> u32 {
        self.faces
            .iter()
            .flatten()
            .flatten()
            .map(|t| t.size)
            .max()
            .unwrap_or(1)
    }
}

/// A tile's size from its face's size codes (0x43e020).
fn tile_size(codes: u32, tile: usize) -> u32 {
    let (col, row) = (tile % TILES, tile / TILES);
    match (codes >> (col * 2)) >> (row * 8) & 3 {
        0 => 512,
        1 => 64,
        2 => 128,
        _ => 256,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_sizes() {
        assert_eq!(tile_size(0xaaaa, 0), 128);
        assert_eq!(tile_size(0xaaaa, 7), 128);
        assert_eq!(tile_size(0x0001_0000, 8), 64);
        assert_eq!(tile_size(0, 15), 512);
    }

    /// With the game's files beside the engine, every sky reads.
    #[test]
    fn game_skies() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../GAME/RES/SKIES.RES");
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let bytes = match crate::Container::detect(&bytes) {
            crate::Container::Faoe => crate::faoe::decode(&bytes, "SKIES.RES").unwrap(),
            _ => bytes,
        };
        let ar = crate::rs2::Archive::parse(&bytes).unwrap();
        for e in ar.entries() {
            let cube = Cube::parse(ar.data(e)).unwrap_or_else(|err| panic!("{}: {err}", e.name));
            // Four sides of two rows and a full top.
            let count = |f: usize| cube.faces[f].iter().flatten().count();
            assert_eq!((0..4).map(count).collect::<Vec<_>>(), [8; 4], "{}", e.name);
            assert_eq!(count(4), 16, "{}", e.name);
        }
    }
}
