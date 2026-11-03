//! `mm2-tools terrain`: export a track's terrain as images.

use mm2_formats::trn::Terrain;
use std::path::Path;

/// Writes `heightmap.pgm` (16-bit) and `map.tga` (one pixel per terrain
/// unit, built from the terrain textures) into `out`.
pub fn export(terrain: &Terrain, out: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out)?;
    let n = terrain.size;
    let max = terrain
        .heights
        .iter()
        .cloned()
        .fold(f32::MIN, f32::max)
        .max(1e-6);
    let mut pgm = format!("P5\n{n} {n}\n65535\n").into_bytes();
    for &h in &terrain.heights {
        pgm.extend_from_slice(&((h / max * 65535.0) as u16).to_be_bytes());
    }
    std::fs::write(out.join("heightmap.pgm"), pgm)?;

    let mut rgb = vec![0u8; n * n * 3];
    for tile in &terrain.tiles {
        let tex = &terrain.textures[tile.texture as usize];
        // Pick the mip level with one texel per terrain unit.
        let level = (tex.size / tile.size).trailing_zeros() as usize;
        let side = (tex.size >> level) as usize;
        let rgba = tex.rgba8(level);
        for v in 0..side {
            for u in 0..side {
                let (x, z) = (tile.x as usize + u, tile.z as usize + v);
                if x < n && z < n {
                    let s = &rgba[(v * side + u) * 4..][..3];
                    rgb[(z * n + x) * 3..][..3].copy_from_slice(s);
                }
            }
        }
    }
    std::fs::write(out.join("map.tga"), tga(n as u16, n as u16, &rgb))
}

/// An uncompressed 24-bit top-down TGA.
fn tga(w: u16, h: u16, rgb: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&[24, 0x20]);
    out.extend(rgb.chunks_exact(3).flat_map(|p| [p[2], p[1], p[0]]));
    out
}
