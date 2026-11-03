//! `mm2-tools`: inspect and unpack Motocross Madness 2 data files.

use mm2_formats::{dtm, faoe, rs2, slb, tga, trn, Container};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

mod terrain;
mod textures;

const USAGE: &str = "\
usage:
  mm2-tools info <file>...            show the container format of each file
  mm2-tools list <file>               list the entries in an archive
  mm2-tools extract <file> <out-dir>  write every entry of an archive to out-dir
  mm2-tools decode <file> <out-file>  write the RS2 archive inside a FAOE file
  mm2-tools terrain <track.env> <out-dir>  export a track's heightmap and texture map
  mm2-tools model <file.slb>          show a model's frames, materials and surfaces
  mm2-tools dialog <UI/X.DAT> [out.png]  show a front-end dialog's controls, and draw it
  mm2-tools strings <X.DLL>           list a DLL's string table (UILANG.DLL, LANG.DLL)
  mm2-tools overlay <file.tga> <out.png>  convert an overlay picture from RES/GLOBAL.RES
  mm2-tools textures <GAME-dir> <out-dir>  export every texture as PNG for upscaling";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["info", ref files @ ..] if !files.is_empty() => files.iter().try_for_each(|f| info(f)),
        ["list", file] => list(file),
        ["extract", file, out] => extract(file, Path::new(out)),
        ["decode", file, out] => decode(file, Path::new(out)),
        ["terrain", file, out] => export_terrain(file, Path::new(out)),
        ["model", file] => model(file),
        ["textures", game, out] => textures::export(Path::new(game), Path::new(out)),
        ["dialog", file] => dialog(file, None),
        ["strings", file] => strings(file),
        ["overlay", file, out] => overlay(file, out),
        ["dialog", file, out] => dialog(file, Some(out)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type BoxResult = Result<(), Box<dyn std::error::Error>>;

fn read(path: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}").into())
}

fn info(path: &str) -> BoxResult {
    let bytes = read(path)?;
    let kind = Container::detect(&bytes);
    if kind == Container::Unknown {
        println!("{path}: unknown, {} bytes", bytes.len());
        return Ok(());
    }
    let inner = archive_bytes(path)?;
    let ar = rs2::Archive::parse(&inner)?;
    println!(
        "{path}: {kind}, {} entries, {} bytes",
        ar.entries().len(),
        bytes.len()
    );
    Ok(())
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Returns the archive bytes, decoding the outer FAOE layer if present.
fn archive_bytes(path: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let bytes = read(path)?;
    match Container::detect(&bytes) {
        Container::Rs2 => Ok(bytes),
        Container::Faoe => Ok(faoe::decode(&bytes, &file_name(path))?),
        Container::Unknown => Err(format!("{path}: not an RS2 or FAOE file").into()),
    }
}

fn list(path: &str) -> BoxResult {
    let bytes = archive_bytes(path)?;
    let ar = rs2::Archive::parse(&bytes)?;
    for e in ar.entries() {
        println!("{:>10}  {:#010x}  {}", e.size, e.offset, e.name);
    }
    Ok(())
}

fn extract(path: &str, out: &Path) -> BoxResult {
    let bytes = archive_bytes(path)?;
    let ar = rs2::Archive::parse(&bytes)?;
    for e in ar.entries() {
        let dest = out.join(safe_relative(&e.name)?);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, ar.data(e))?;
    }
    println!(
        "extracted {} entries to {}",
        ar.entries().len(),
        out.display()
    );
    Ok(())
}

fn decode(path: &str, out: &Path) -> BoxResult {
    let bytes = read(path)?;
    let plain = faoe::decode(&bytes, &file_name(path))?;
    std::fs::write(out, plain)?;
    Ok(())
}

fn export_terrain(path: &str, out: &Path) -> BoxResult {
    let bytes = archive_bytes(path)?;
    let ar = rs2::Archive::parse(&bytes)?;
    let entry = ar
        .entries()
        .iter()
        .find(|e| e.name.to_ascii_lowercase().ends_with(".trn"))
        .ok_or("no .trn entry in this archive")?;
    let t = trn::parse(ar.data(entry))?;
    let max = t.heights.iter().cloned().fold(f32::MIN, f32::max);
    println!(
        "{}: {}x{} samples, scale {}, max height {max:.2}, {} textures, {} tiles",
        entry.name,
        t.size,
        t.size,
        t.scale,
        t.textures.len(),
        t.tiles.len()
    );
    terrain::export(&t, out)?;
    println!("wrote heightmap.pgm and map.tga to {}", out.display());
    Ok(())
}

fn model(path: &str) -> BoxResult {
    let m = slb::parse(&read(path)?)?;
    let world = m.world_matrices();
    println!("{} frames", m.frames.len());
    for (i, f) in m.frames.iter().enumerate() {
        let parent = f.parent.map_or("-".to_string(), |p| p.to_string());
        let w = &world[i];
        println!(
            "  {i:>3} {:<24} parent {parent:>3}  at {:8.3} {:8.3} {:8.3}",
            f.name, w[12], w[13], w[14]
        );
    }
    println!("{} materials", m.materials.len());
    for (i, mat) in m.materials.iter().enumerate() {
        println!("  {i:>3} {}", mat.texture);
    }
    for (l, lod) in m.lods.iter().enumerate() {
        println!("LOD {l}: {} surfaces", lod.surfaces.len());
        for (i, s) in lod.surfaces.iter().enumerate() {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &s.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            println!(
                "  {i:>3} {} verts, {} faces, materials {:?}, groups {:?}, bounds {lo:.2?}..{hi:.2?}",
                s.positions.len(),
                s.faces.len(),
                s.materials,
                s.groups
            );
        }
    }
    Ok(())
}

/// Turns an entry name into a relative path that cannot escape the output
/// directory.
fn safe_relative(name: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(name.replace('\\', "/"));
    if name.is_empty() || !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(format!("refusing unsafe entry name {name:?}"));
    }
    Ok(path)
}

/// Prints a front-end dialog's controls and draws it as it first appears.
fn dialog(path: &str, out: Option<&str>) -> BoxResult {
    let bytes = archive_bytes(path)?;
    let ar = rs2::Archive::parse(&bytes)?;
    let entry = ar
        .entries()
        .iter()
        .find(|e| e.name.to_ascii_lowercase().ends_with(".dtm"))
        .ok_or("no .dtm in this archive")?;
    let d = dtm::parse(ar.data(entry));
    // Shared art lives in GLOBAL.DAT beside the dialogs.
    let global_bytes = Path::new(path)
        .parent()
        .map(|dir| dir.join("GLOBAL.DAT"))
        .and_then(|p| std::fs::read(p).ok())
        .unwrap_or_default();
    let global = rs2::Archive::parse(&global_bytes).ok();
    let global_dtm = global
        .as_ref()
        .and_then(|g| g.get("global.dtm"))
        .map(dtm::parse);
    println!(
        "{}: background {:?}, {} controls, {} images",
        entry.name,
        d.background(),
        d.controls.len(),
        d.anims.len()
    );
    let image = |name: &str| -> Option<tga::Image> {
        let file = d
            .anim(name)
            .or_else(|| global_dtm.as_ref()?.anim(name))
            .and_then(|a| a.frames.first().cloned())
            .unwrap_or_else(|| format!("{name}.tga"));
        let found = ar
            .get(&file)
            .or_else(|| global.as_ref()?.get(&file))
            .or_else(|| {
                let base = file.rsplit('\\').next()?;
                ar.entries()
                    .iter()
                    .find(|e| {
                        e.name
                            .rsplit('\\')
                            .next()
                            .is_some_and(|n| n.eq_ignore_ascii_case(base))
                    })
                    .map(|e| ar.data(e))
            });
        match found.map(tga::parse) {
            Some(Ok(img)) => Some(img),
            Some(Err(e)) => {
                println!("  {file}: {e}");
                None
            }
            None => {
                println!("  missing image {name} ({file})");
                None
            }
        }
    };
    for c in &d.controls {
        println!(
            "  {:<24} {:?} at {:?}{}{}",
            c.name,
            c.kind,
            d.place(c),
            c.anim("AnimNorm")
                .map_or(String::new(), |a| format!(" image {a}")),
            c.get("Text")
                .map_or(String::new(), |t| format!(" text {t:?}"))
        );
    }
    let Some(out) = out else { return Ok(()) };
    let (w, h) = (640u32, 480u32);
    let mut canvas = vec![0u8; (w * h * 4) as usize];
    let mut blit = |img: &tga::Image, x: i32, y: i32| {
        for sy in 0..img.height {
            for sx in 0..img.width {
                let (dx, dy) = (x + sx as i32, y + sy as i32);
                if dx < 0 || dy < 0 || dx >= w as i32 || dy >= h as i32 {
                    continue;
                }
                let p = img.pixel(sx, sy);
                if p[3] == 0 {
                    continue;
                }
                let i = ((dy as u32 * w + dx as u32) * 4) as usize;
                canvas[i..i + 4].copy_from_slice(&p);
            }
        }
    };
    if let Some(img) = d.background().and_then(&image) {
        blit(&img, 0, 0);
    }
    for c in &d.controls {
        if c.anchor.is_some() || c.props.flag("Show") == Some(false) {
            continue;
        }
        if let Some(img) = c.anim("AnimNorm").and_then(&image) {
            let r = d.place(c);
            blit(&img, r[0], r[1]);
        }
    }
    let file = std::fs::File::create(out)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&canvas)?;
    println!("wrote {out}");
    Ok(())
}

fn overlay(path: &str, out: &str) -> BoxResult {
    let img = mm2_formats::texture::parse_overlay(&read(path)?)?;
    let file = std::fs::File::create(out)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&img.rgba)?;
    println!("{}x{} wrote {out}", img.width, img.height);
    Ok(())
}

fn strings(path: &str) -> BoxResult {
    let table = mm2_formats::pe::strings(&read(path)?)?;
    let mut ids: Vec<_> = table.keys().copied().collect();
    ids.sort();
    for id in ids {
        println!("{id:>6}  {:?}", table[&id]);
    }
    Ok(())
}
