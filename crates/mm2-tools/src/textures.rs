//! `mm2-tools textures`: export every texture in the game as PNG, ready
//! for an AI upscaler. See `mm2_formats::hires` for the naming and the
//! colour key.
//!
//! ```text
//! out/models/   model textures from every .RES, .TEX and .ENV
//! out/terrain/  terrain textures, per track
//! out/skies/    sky box faces, whole (the four sides, then the top)
//! out/images/   menu art, fonts, HUD and previews (TGA and format 11)
//! ```
//!
//! Each folder is flat, so an upscaler's batch mode can take it whole.
//! `MODELTGA.RES` holds 8-bit palette copies of the model textures (format
//! 9) for old graphics cards; they are not exported.

use mm2_formats::{cub, faoe, hires, rs2, texture, tga, trn, Container};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Export {
    /// The bytes behind each model texture key already written, to spot
    /// different textures that share a name.
    models: HashMap<String, Vec<u8>>,
    terrain: usize,
    skies: usize,
    images: usize,
    clashes: usize,
    skipped: usize,
}

pub fn export(game: &Path, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for dir in ["models", "terrain", "skies", "images"] {
        std::fs::create_dir_all(out.join(dir))?;
    }
    let mut files = Vec::new();
    walk(game, &mut files)?;
    files.sort();
    let mut ex = Export::default();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(path)?;
        let archive = match Container::detect(&bytes) {
            Container::Rs2 => bytes,
            Container::Faoe => faoe::decode(&bytes, &name)?,
            Container::Unknown => {
                if name.to_ascii_lowercase().ends_with(".tga") {
                    ex.image(&name, &bytes, out)?;
                }
                continue;
            }
        };
        let ar = rs2::Archive::parse(&archive)?;
        for e in ar.entries() {
            let lower = e.name.to_ascii_lowercase();
            let data = ar.data(e);
            if lower.ends_with(".cub") {
                let cube = cub::Cube::parse(data).map_err(|err| format!("{}: {err}", e.name))?;
                let side = cube.tile_size() * cub::TILES as u32;
                for face in 0..6 {
                    if cube.faces[face].iter().all(Option::is_none) {
                        continue;
                    }
                    let mut rgba = cube.face_rgba(face, side);
                    hires::bleed(&mut rgba, side as usize, side as usize);
                    let file = format!("{}.png", hires::sky_key(&e.name, face));
                    write_png(&out.join("skies").join(file), side, side, &rgba)?;
                    ex.skies += 1;
                }
            } else if lower.ends_with(".trn") {
                let t = trn::parse(data).map_err(|err| format!("{name}/{}: {err}", e.name))?;
                for (i, tex) in t.textures.iter().enumerate() {
                    let file = format!("{}.png", hires::terrain_key(&name, i));
                    write_texture(&out.join("terrain").join(file), tex)?;
                    ex.terrain += 1;
                }
            } else if lower.ends_with(".tga") {
                let image_name = format!("{name}/{}", e.name);
                if let Ok(tex) = texture::parse(data) {
                    ex.model(&name, &e.name, data, &tex, out)?;
                } else if let Ok(img) = texture::parse_overlay(data) {
                    ex.write_image(&image_name, img, out)?;
                } else {
                    ex.image(&image_name, data, out)?;
                }
            }
        }
    }
    println!(
        "wrote {} model textures, {} terrain textures, {} sky faces and {} images to {}",
        ex.models.len(),
        ex.terrain,
        ex.skies,
        ex.images,
        out.display()
    );
    if ex.skipped > 0 {
        println!(
            "skipped {} .tga entries in formats this tool does not read",
            ex.skipped
        );
    }
    if ex.clashes > 0 {
        println!(
            "{} textures share a name with a different texture; the first one found was kept",
            ex.clashes
        );
    }
    Ok(())
}

impl Export {
    fn model(
        &mut self,
        archive: &str,
        name: &str,
        data: &[u8],
        tex: &trn::Texture,
        out: &Path,
    ) -> std::io::Result<()> {
        let key = hires::model_key(archive, name);
        if let Some(seen) = self.models.get(&key) {
            if seen != data {
                eprintln!(
                    "{archive}: {name} differs from an earlier texture of that name; skipped"
                );
                self.clashes += 1;
            }
            return Ok(());
        }
        write_texture(&out.join("models").join(format!("{key}.png")), tex)?;
        self.models.insert(key, data.to_vec());
        Ok(())
    }

    /// Writes a plain TGA as `images/<name>.png`, with the folders in
    /// `name` flattened to `__`.
    /// Writes a plain TGA as `images/<name>.png`, with the folders in
    /// `name` flattened to `__`.
    fn image(&mut self, name: &str, data: &[u8], out: &Path) -> std::io::Result<()> {
        match tga::parse(data) {
            Ok(img) => self.write_image(name, img, out),
            Err(_) => {
                self.skipped += 1;
                Ok(())
            }
        }
    }

    fn write_image(&mut self, name: &str, mut img: tga::Image, out: &Path) -> std::io::Result<()> {
        let (w, h) = (img.width as usize, img.height as usize);
        hires::key_rgba(&mut img.rgba, w, h);
        let flat = name.to_ascii_lowercase().replace(['\\', '/'], "__");
        let file = out.join("images").join(format!("{flat}.png"));
        write_png(&file, img.width, img.height, &img.rgba)?;
        self.images += 1;
        Ok(())
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn write_texture(path: &Path, tex: &trn::Texture) -> std::io::Result<()> {
    write_png(path, tex.size, tex.size, &hires::export_rgba(tex))
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(rgba))
        .map_err(std::io::Error::other)
}
