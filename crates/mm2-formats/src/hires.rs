//! High-resolution texture packs: the conventions shared by the exporter
//! (`mm2-tools textures`) and the engine that loads the upscaled images.
//!
//! Each texture is exported as a top-down RGBA image. In RGB555 textures
//! the game marks transparent pixels with pure magenta (`0x7c1f`); the
//! export makes those pixels transparent and fills their colour from the
//! nearest opaque pixels, so an AI upscaler does not smear magenta into the
//! edges. Loading reverses this: rows go back to the game's bottom-up order
//! and transparency comes from the image's alpha, or from the original
//! texture when the upscaler dropped the alpha channel.
//!
//! An image is matched to its texture by file name, ignoring case:
//! `<texture name>.png` for model textures (`barrel.tga.png`),
//! `<set>-<texture name>.png` for the stadium sets in `.TEX` files, which
//! reuse each other's names (`euro-wall.tga.png`), and
//! `<track>-terrain-<index>.png` for terrain (`sx01-terrain-007.png`), and
//! `sky-<sky>-<face>.png` for the six faces of each sky box
//! (`sky-terra01-4.png`, 4 being the top).
//! Anything an upscaler appends from `_upscayl` on is ignored.

use crate::trn::{PixelFormat, Texture};

/// RGB555 pixels of this colour are transparent.
pub const COLOR_KEY: u16 = 0x7c1f;

/// The key for model texture `name` from the archive file `archive`.
pub fn model_key(archive: &str, name: &str) -> String {
    match archive.to_ascii_lowercase().strip_suffix(".tex") {
        Some(set) => format!("{set}-{}", name.to_ascii_lowercase()),
        None => name.to_ascii_lowercase(),
    }
}

/// The key for terrain texture `index` of the track whose `.ENV` file is
/// called `track` (with or without the extension).
pub fn terrain_key(track: &str, index: usize) -> String {
    let stem = track.rsplit_once('.').map_or(track, |(s, _)| s);
    format!("{}-terrain-{index:03}", stem.to_ascii_lowercase())
}

/// The key for face `face` (0 to 5) of the sky box in `RES/SKIES.RES`
/// called `sky`, such as `Terra01.cub`.
pub fn sky_key(sky: &str, face: usize) -> String {
    let stem = sky.rsplit_once('.').map_or(sky, |(s, _)| s);
    format!("sky-{}-{face}", stem.to_ascii_lowercase())
}

/// The texture key an image file name stands for, or `None` if it is not
/// a PNG.
pub fn key_from_file_name(file_name: &str) -> Option<String> {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".png")?;
    // Upscayl names its output `<name>_upscayl_4x_<model>.png`.
    let stem = stem.find("_upscayl").map_or(stem, |i| &stem[..i]);
    Some(stem.to_string())
}

/// Whether a texture uses the magenta colour key.
pub fn is_keyed(tex: &Texture) -> bool {
    tex.format == PixelFormat::Rgb555 && tex.levels[0].iter().any(|&p| p & 0x7fff == COLOR_KEY)
}

/// The top level of a texture as a top-down RGBA8 image for export.
pub fn export_rgba(tex: &Texture) -> Vec<u8> {
    let side = tex.size as usize;
    let mut rgba = tex.rgba8(0);
    if is_keyed(tex) {
        for (px, &p) in rgba.chunks_exact_mut(4).zip(&tex.levels[0]) {
            if p & 0x7fff == COLOR_KEY {
                px.copy_from_slice(&[0; 4]);
            }
        }
        bleed(&mut rgba, side, side);
    }
    flip(&mut rgba, side);
    rgba
}

/// Makes pure magenta pixels of a top-down RGBA8 image transparent, with
/// colour bled in from their neighbours, as `export_rgba` does for
/// textures. For front-end art, which uses the same key.
pub fn key_rgba(rgba: &mut [u8], width: usize, height: usize) {
    let mut keyed = false;
    for px in rgba.chunks_exact_mut(4) {
        if px[..3] == [255, 0, 255] {
            px.copy_from_slice(&[0; 4]);
            keyed = true;
        }
    }
    if keyed {
        bleed(rgba, width, height);
    }
}

/// Gives transparent pixels the average colour of their opaque (or already
/// filled) neighbours, spreading outwards, leaving alpha alone.
pub fn bleed(rgba: &mut [u8], width: usize, height: usize) {
    let mut filled: Vec<bool> = rgba.chunks_exact(4).map(|p| p[3] != 0).collect();
    loop {
        let mut next = filled.clone();
        let mut changed = false;
        for y in 0..height {
            for x in 0..width {
                let i = y * width + x;
                if filled[i] {
                    continue;
                }
                let mut sum = [0u32; 3];
                let mut n = 0;
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let j = ny as usize * width + nx as usize;
                    if filled[j] {
                        for c in 0..3 {
                            sum[c] += rgba[j * 4 + c] as u32;
                        }
                        n += 1;
                    }
                }
                if n == 0 {
                    continue;
                }
                for c in 0..3 {
                    rgba[i * 4 + c] = (sum[c] / n) as u8;
                }
                next[i] = true;
                changed = true;
            }
        }
        filled = next;
        if !changed {
            return;
        }
    }
}

/// Turns a top-down image into the game's bottom-up rows, or back.
fn flip(rgba: &mut [u8], side: usize) {
    let row = side * 4;
    for y in 0..side / 2 {
        let (top, bottom) = rgba.split_at_mut((side - 1 - y) * row);
        top[y * row..][..row].swap_with_slice(&mut bottom[..row]);
    }
}

/// Why an upscaled image cannot replace its texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    NotSquare { width: u32, height: u32 },
    NotPowerOfTwo(u32),
    Smaller { image: u32, original: u32 },
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mismatch::NotSquare { width, height } => {
                write!(f, "image is {width}x{height}, not square")
            }
            Mismatch::NotPowerOfTwo(side) => write!(f, "image side {side} is not a power of two"),
            Mismatch::Smaller { image, original } => {
                write!(
                    f,
                    "image is {image} pixels, smaller than the original {original}"
                )
            }
        }
    }
}

/// Turns an upscaled top-down RGBA8 image of `tex` into a bottom-up RGBA8
/// mip chain, largest first, ready to upload in its place.
pub fn import_rgba(
    tex: &Texture,
    mut rgba: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<Vec<Vec<u8>>, Mismatch> {
    if width != height {
        return Err(Mismatch::NotSquare { width, height });
    }
    if !width.is_power_of_two() {
        return Err(Mismatch::NotPowerOfTwo(width));
    }
    if width < tex.size {
        return Err(Mismatch::Smaller {
            image: width,
            original: tex.size,
        });
    }
    let side = width as usize;
    flip(&mut rgba, side);

    let has_alpha = rgba.chunks_exact(4).any(|p| p[3] != 255);
    let keyed = is_keyed(tex);
    if (keyed || tex.has_alpha()) && !has_alpha {
        // The upscaler dropped alpha: take it from the original.
        let original = tex.rgba8(0);
        let scale = side / tex.size as usize;
        for y in 0..side {
            for x in 0..side {
                let j = (y / scale) * tex.size as usize + x / scale;
                rgba[(y * side + x) * 4 + 3] = if keyed {
                    if tex.levels[0][j] & 0x7fff == COLOR_KEY {
                        0
                    } else {
                        255
                    }
                } else {
                    original[j * 4 + 3]
                };
            }
        }
    }
    for px in rgba.chunks_exact_mut(4) {
        if keyed {
            // The game's key is all or nothing.
            px[3] = if px[3] >= 128 { 255 } else { 0 };
        } else if tex.format == PixelFormat::Rgb555 {
            px[3] = 255;
        }
        if px[3] == 0 {
            px.copy_from_slice(&[0; 4]);
        }
    }

    let mut levels = vec![rgba];
    let mut level_side = side / 2;
    while level_side > 0 {
        let next = downsample(levels.last().unwrap(), level_side);
        levels.push(next);
        level_side /= 2;
    }
    Ok(levels)
}

/// Halves an RGBA8 image to `side` pixels square, weighting colour by alpha.
pub fn downsample(src: &[u8], side: usize) -> Vec<u8> {
    let src_side = side * 2;
    let mut out = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            let mut sum = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((y * 2 + dy) * src_side + x * 2 + dx) * 4;
                let a = src[i + 3] as u32;
                for c in 0..3 {
                    sum[c] += src[i + c] as u32 * a;
                }
                sum[3] += a;
            }
            let a = sum[3];
            for c in &sum[..3] {
                out.push(c.checked_div(a).unwrap_or(0) as u8);
            }
            out.push((a / 4) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyed_texture() -> Texture {
        // 2x2, bottom-up: bottom row red + key, top row green + blue.
        Texture {
            size: 2,
            format: PixelFormat::Rgb555,
            levels: vec![vec![0x7c00, COLOR_KEY, 0x03e0, 0x001f], vec![0]],
        }
    }

    #[test]
    fn upscayl_names_map_back_to_keys() {
        assert_eq!(
            key_from_file_name("Barrel.tga_upscayl_4x_realesrgan-x4plus.png"),
            Some("barrel.tga".into())
        );
        assert_eq!(
            key_from_file_name("sx01-terrain-007.PNG"),
            Some("sx01-terrain-007".into())
        );
        assert_eq!(key_from_file_name("notes.txt"), None);
        assert_eq!(terrain_key("SX01.ENV", 7), "sx01-terrain-007");
        assert_eq!(sky_key("Terra01.cub", 4), "sky-terra01-4");
        assert_eq!(model_key("OBJECTS.RES", "Barrel.tga"), "barrel.tga");
        assert_eq!(model_key("EURO.TEX", "Wall.tga"), "euro-wall.tga");
    }

    #[test]
    fn export_flips_and_bleeds_the_key() {
        let rgba = export_rgba(&keyed_texture());
        // Top-down: green, blue, then red, keyed.
        assert_eq!(&rgba[0..4], &[0, 255, 0, 255]);
        assert_eq!(&rgba[4..8], &[0, 0, 255, 255]);
        assert_eq!(&rgba[8..12], &[255, 0, 0, 255]);
        assert_eq!(rgba[15], 0, "key is transparent");
        assert_ne!(&rgba[12..15], &[255, 0, 255], "no magenta left");
    }

    #[test]
    fn import_restores_rows_and_key_without_alpha() {
        let tex = keyed_texture();
        let mut rgba = export_rgba(&tex);
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255; // as if the upscaler dropped alpha
        }
        // A 2x "upscale" by repeating pixels.
        let mut big = vec![0u8; 4 * 4 * 4];
        for y in 0..4 {
            for x in 0..4 {
                let s = ((y / 2) * 2 + x / 2) * 4;
                big[(y * 4 + x) * 4..][..4].copy_from_slice(&rgba[s..s + 4]);
            }
        }
        let levels = import_rgba(&tex, big, 4, 4).unwrap();
        assert_eq!(levels.len(), 3);
        // Bottom-up again: first pixel red, the key's quarter transparent.
        assert_eq!(&levels[0][0..4], &[255, 0, 0, 255]);
        assert_eq!(&levels[0][(0 * 4 + 3) * 4..][..4], &[0, 0, 0, 0]);
        assert_eq!(levels[0][(1 * 4 + 2) * 4 + 3], 0);
        assert!(import_rgba(&tex, vec![0; 4 * 4 * 3], 4, 3).is_err());
    }
}
