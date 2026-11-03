//! Textures ready to upload, and the optional high-resolution pack that
//! replaces them (`--hires <dir>`). Without a pack every texture is the
//! game's own. See `mm2_formats::hires` for how pack images are named.

use mm2_formats::cub::{Cube, TILES};
use mm2_formats::hires::{self, COLOR_KEY};
pub use mm2_formats::hires::{model_key, terrain_key};
use mm2_formats::trn::Texture;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

static PACK: OnceLock<HiRes> = OnceLock::new();

/// Makes `pack` the high-res pack for every track loaded from now on.
pub fn set_pack(pack: HiRes) {
    let _ = PACK.set(pack);
}

/// The high-res pack, if `--hires` gave one.
pub fn pack() -> Option<&'static HiRes> {
    PACK.get()
}

/// A square RGBA8 texture with its full mip chain, largest first, rows in
/// the game's bottom-up order.
#[derive(Clone)]
pub struct Image {
    pub size: u32,
    pub levels: Vec<Vec<u8>>,
}

impl Image {
    /// The game's own texture.
    pub fn from_texture(tex: &Texture) -> Image {
        let levels = if hires::is_keyed(tex) {
            // The stored mips blend the key colour into the edges, so
            // rebuild them from the top level with the key as alpha.
            let mut top = tex.rgba8(0);
            for (px, &p) in top.chunks_exact_mut(4).zip(&tex.levels[0]) {
                if p & 0x7fff == COLOR_KEY {
                    px.copy_from_slice(&[0; 4]);
                }
            }
            let mut levels = vec![top];
            for level in 1..tex.levels.len() {
                let side = (tex.size >> level) as usize;
                levels.push(hires::downsample(&levels[level - 1], side));
            }
            levels
        } else {
            (0..tex.levels.len()).map(|l| tex.rgba8(l)).collect()
        };
        Image {
            size: tex.size,
            levels,
        }
    }

    /// Doubles the image with nearest-neighbour sampling until it is
    /// `size` pixels square, keeping its existing mips as they are.
    pub fn grow_to(&mut self, size: u32) {
        while self.size < size {
            let side = self.size as usize;
            let src = &self.levels[0];
            let mut big = vec![0u8; side * side * 16];
            for y in 0..side * 2 {
                for x in 0..side * 2 {
                    let s = ((y / 2) * side + x / 2) * 4;
                    big[(y * side * 2 + x) * 4..][..4].copy_from_slice(&src[s..s + 4]);
                }
            }
            self.levels.insert(0, big);
            self.size *= 2;
        }
    }
}

/// A folder of upscaled PNGs, found by texture key.
pub struct HiRes {
    files: HashMap<String, PathBuf>,
    used: AtomicUsize,
}

impl HiRes {
    /// Indexes every PNG under `dir`, in any subfolder.
    pub fn open(dir: &Path) -> std::io::Result<HiRes> {
        let mut files = HashMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in crate::platform::fs::read_dir(&dir)? {
                let path = entry?.path();
                if crate::platform::fs::is_dir(&path) {
                    stack.push(path);
                } else if let Some(key) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(hires::key_from_file_name)
                {
                    files.insert(key, path);
                }
            }
        }
        Ok(HiRes {
            files,
            used: Default::default(),
        })
    }

    /// How many images have replaced textures so far.
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// The pack's replacement for `tex`, if it has a usable one.
    /// The pack's image of a sky face, as (side, RGBA8), with the
    /// original face's `side` and pixels; where the original has no tile
    /// the image stays clear.
    fn sky_face(&self, key: &str, side: u32, original: &[u8]) -> Option<(u32, Vec<u8>)> {
        let path = self.files.get(key)?;
        let (w, h, mut rgba) = match read_png(path) {
            Ok(image) => image,
            Err(err) => {
                eprintln!("{}: {err}; using the original", path.display());
                return None;
            }
        };
        if w != h || !w.is_power_of_two() || w < side {
            eprintln!(
                "{}: image is {w}x{h}, not a square power of two at least {side}; using the original",
                path.display()
            );
            return None;
        }
        let k = (w / side) as usize;
        for y in 0..w as usize {
            for x in 0..w as usize {
                let a = original[((y / k) * side as usize + x / k) * 4 + 3];
                rgba[(y * w as usize + x) * 4 + 3] = a;
            }
        }
        self.used.fetch_add(1, Ordering::Relaxed);
        Some((w, rgba))
    }

    pub fn image(&self, key: &str, tex: &Texture) -> Option<Image> {
        let path = self.files.get(key)?;
        let loaded = read_png(path)
            .map_err(|e| e.to_string())
            .and_then(|(w, h, rgba)| {
                hires::import_rgba(tex, rgba, w, h).map_err(|e| e.to_string())
            });
        match loaded {
            Ok(levels) => {
                self.used.fetch_add(1, Ordering::Relaxed);
                Some(Image {
                    size: (levels[0].len() / 4).isqrt() as u32,
                    levels,
                })
            }
            Err(err) => {
                eprintln!("{}: {err}; using the original", path.display());
                None
            }
        }
    }
}

/// A sky box's six faces as one RGBA8 image each, `side` pixels square,
/// in a row, ready to upload as layers.
pub struct SkyImages {
    pub side: u32,
    pub faces: Vec<u8>,
}

/// The faces of the sky box `name` (`Terra01.cub`): the pack's where it
/// has them, else the game's. Faces without an image are enlarged to match
/// the others.
pub fn sky_images(cube: &Cube, name: &str, hires: Option<&HiRes>) -> SkyImages {
    let side = cube.tile_size() * TILES as u32;
    let original: Vec<Vec<u8>> = (0..6).map(|f| cube.face_rgba(f, side)).collect();
    let mut faces: Vec<(u32, Vec<u8>)> = original.iter().map(|o| (side, o.clone())).collect();
    if let Some(pack) = hires {
        for (face, slot) in faces.iter_mut().enumerate() {
            if let Some(image) = pack.sky_face(&hires::sky_key(name, face), side, &original[face]) {
                *slot = image;
            }
        }
    }
    let target = faces.iter().map(|f| f.0).max().unwrap_or(side);
    let faces = faces
        .into_iter()
        .flat_map(|(s, rgba)| enlarge(&rgba, s as usize, (target / s) as usize))
        .collect();
    SkyImages {
        side: target,
        faces,
    }
}

/// Scales a square RGBA8 image up `k` times with nearest-neighbour sampling.
fn enlarge(src: &[u8], side: usize, k: usize) -> Vec<u8> {
    if k == 1 {
        return src.to_vec();
    }
    let big = side * k;
    let mut out = vec![0u8; big * big * 4];
    for y in 0..big {
        for x in 0..big {
            let s = ((y / k) * side + x / k) * 4;
            out[(y * big + x) * 4..][..4].copy_from_slice(&src[s..s + 4]);
        }
    }
    out
}

/// The pack's image for `key` when there is one, else the original.
pub fn image(hires: Option<&HiRes>, key: &str, tex: &Texture) -> Image {
    hires
        .and_then(|h| h.image(key, tex))
        .unwrap_or_else(|| Image::from_texture(tex))
}

/// Reads a PNG as top-down RGBA8.
fn read_png(path: &Path) -> Result<(u32, u32, Vec<u8>), Box<dyn std::error::Error>> {
    let file = std::io::BufReader::new(crate::platform::fs::open(path)?);
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf)?;
    let pixels = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("indexed PNG was not expanded".into()),
    };
    Ok((info.width, info.height, rgba))
}
