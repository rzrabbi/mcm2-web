//! Checks against a real copy of the game. Skipped unless `MM2_DATA` points
//! at the game's `GAME` folder, so CI never needs the game files.

use mm2_formats::{anim, col, faoe, rs2, scn, slb, tdf, texture, trn, Container};
use std::path::{Path, PathBuf};

fn game_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("MM2_DATA")?);
    // Relative paths resolve against the crate, not where cargo was run.
    assert!(
        dir.is_dir(),
        "MM2_DATA={} is not a directory",
        dir.display()
    );
    Some(dir)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn every_archive_parses_and_fills_the_file() {
    let Some(dir) = game_dir() else {
        eprintln!("MM2_DATA not set, skipping");
        return;
    };
    let mut files = Vec::new();
    walk(&dir, &mut files);
    let (mut rs2_count, mut faoe_count, mut scn_count, mut trn_count) = (0, 0, 0, 0);
    let mut tdf_count = 0;
    for path in files {
        let raw = std::fs::read(&path).unwrap();
        let bytes = match Container::detect(&raw) {
            Container::Rs2 => {
                rs2_count += 1;
                raw
            }
            Container::Faoe => {
                faoe_count += 1;
                let name = path.file_name().unwrap().to_str().unwrap();
                faoe::decode(&raw, name).unwrap()
            }
            Container::Unknown => continue,
        };
        let ar = rs2::Archive::parse(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let end = ar
            .entries()
            .iter()
            .map(|e| e.offset + e.size)
            .max()
            .unwrap_or(0);
        assert_eq!(end as usize, bytes.len(), "{}", path.display());
        // Every track's scene file is INI text, which catches a wrong key.
        for e in ar.entries() {
            if e.name.to_ascii_lowercase().ends_with(".trn") {
                let t = trn::parse(ar.data(e))
                    .unwrap_or_else(|err| panic!("{}: {}: {err}", path.display(), e.name));
                assert!(t.size > 17 && !t.tiles.is_empty(), "{}", e.name);
                assert!(
                    t.heights.iter().all(|h| h.is_finite() && *h >= 0.0),
                    "{}",
                    e.name
                );
                trn_count += 1;
            }
            if e.name.to_ascii_lowercase().ends_with(".scn") {
                assert!(
                    ar.data(e).starts_with(b"[SceneInfo]"),
                    "{}: {}",
                    path.display(),
                    e.name
                );
                let scene = scn::Scene::parse(ar.data(e))
                    .unwrap_or_else(|err| panic!("{}: {}: {err}", path.display(), e.name));
                assert!(
                    ar.get(&scene.terrain_file).is_some(),
                    "{}: {} names missing terrain {}",
                    path.display(),
                    e.name,
                    scene.terrain_file
                );
                scn_count += 1;
            }
            if e.name.to_ascii_lowercase().ends_with(".tdf") {
                let def = tdf::parse(ar.data(e))
                    .unwrap_or_else(|err| panic!("{}: {}: {err}", path.display(), e.name));
                assert!(
                    def.start_line().is_some(),
                    "{}: {} has no start",
                    path.display(),
                    e.name
                );
                tdf_count += 1;
            }
        }
    }
    assert!(tdf_count > 0, "no .tdf entries found");
    assert!(scn_count > 0, "no .scn entries found");
    assert_eq!(trn_count, scn_count, "every track should have terrain");
    assert!(rs2_count > 0, "no RS2 files found under {}", dir.display());
    assert!(
        faoe_count > 0,
        "no FAOE files found under {}",
        dir.display()
    );
}

fn archive(dir: &Path, name: &str) -> Vec<u8> {
    let path = dir.join("RES").join(name);
    let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    match Container::detect(&raw) {
        Container::Faoe => faoe::decode(&raw, name).unwrap(),
        _ => raw,
    }
}

#[test]
fn every_model_and_texture_parses() {
    let Some(dir) = game_dir() else {
        eprintln!("MM2_DATA not set, skipping");
        return;
    };
    let models = archive(&dir, "MODELS.RES");
    let models = rs2::Archive::parse(&models).unwrap();
    let mut count = 0;
    let mut textures = std::collections::BTreeSet::new();
    for e in models.entries() {
        if !e.name.to_ascii_lowercase().ends_with(".slb") {
            continue;
        }
        let m = slb::parse(models.data(e)).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert!(!m.lods.is_empty(), "{}", e.name);
        for s in m.lods.iter().flat_map(|l| &l.surfaces) {
            assert!(
                s.groups.iter().all(|g| (g.0 as usize) < m.frames.len()),
                "{}",
                e.name
            );
            assert!(
                s.materials
                    .iter()
                    .all(|&i| (i as usize) < m.materials.len()),
                "{}",
                e.name
            );
        }
        textures.extend(m.materials.iter().map(|t| t.texture.to_ascii_lowercase()));
        count += 1;
    }
    assert!(count > 300, "only {count} models");

    let objects = archive(&dir, "OBJECTS.RES");
    let objects = rs2::Archive::parse(&objects).unwrap();
    for e in objects.entries() {
        if e.name.to_ascii_lowercase().ends_with(".tga") {
            let t =
                texture::parse(objects.data(e)).unwrap_or_else(|err| panic!("{}: {err}", e.name));
            assert_eq!(t.levels[0].len() as u32, t.size * t.size);
        }
    }
    for e in objects.entries() {
        let name = e.name.to_ascii_lowercase();
        if name.ends_with(".mcf") {
            let mcf =
                anim::Mcf::parse(objects.data(e)).unwrap_or_else(|err| panic!("{}: {err}", e.name));
            for m in &mcf.motions {
                if let Some(data) = objects.get(&m.file) {
                    let motion =
                        anim::Motion::parse(data).unwrap_or_else(|err| panic!("{}: {err}", m.file));
                    assert!(!motion.frames.is_empty(), "{}", m.file);
                }
            }
        }
        if name.ends_with(".mot") {
            let data = objects.data(e);
            let motion =
                anim::Motion::parse(data).unwrap_or_else(|err| panic!("{}: {err}", e.name));
            let keys: usize = motion.frames.iter().map(Vec::len).sum();
            assert_eq!(8 + keys * 37 + 256, data.len(), "{}", e.name);
        }
    }
    let missing: Vec<_> = textures
        .iter()
        .filter(|t| !t.is_empty() && objects.get(t).is_none())
        .collect();
    eprintln!(
        "{count} models, {} textures, missing: {missing:?}",
        textures.len()
    );
}

#[test]
fn every_collision_model_parses() {
    let Some(dir) = game_dir() else {
        eprintln!("MM2_DATA not set, skipping");
        return;
    };
    let mut count = 0;
    for pack in ["COLSEG.RES", "BIKE.RES"] {
        let bytes = archive(&dir, pack);
        let ar = rs2::Archive::parse(&bytes).unwrap();
        for e in ar.entries() {
            if !e.name.to_ascii_lowercase().ends_with(".col") {
                continue;
            }
            let c = col::parse(ar.data(e)).unwrap_or_else(|err| panic!("{}: {err}", e.name));
            assert!(!c.parts.is_empty(), "{}", e.name);
            for p in &c.parts {
                for n in &p.body.triangles {
                    if let col::Node::Inner { right, .. } = n {
                        assert!((*right as usize) < p.body.triangles.len(), "{}", e.name);
                    }
                }
            }
            count += 1;
        }
    }
    assert_eq!(count, 248, "collision models");
}

#[test]
fn the_demo_replay_plays_through() {
    let Some(dir) = game_dir() else {
        eprintln!("MM2_DATA not set, skipping");
        return;
    };
    use mm2_formats::vcr;
    let bytes = std::fs::read(dir.join("UI/DEMO.RPL")).unwrap();
    let tape = vcr::Tape::parse(&bytes).unwrap();
    assert_eq!(tape.riders.len(), 7);
    assert_eq!(tape.riders[1].name, "VLADIMIR");
    assert_eq!(tape.track(), "Iffendic");
    assert!((tape.length - 14.132).abs() < 0.01);
    let line = tape.timeline();
    assert_eq!(line.frames.len(), 141);
    assert!((line.length() - tape.length).abs() < 0.2);
    // Every bike is on the tape throughout, and moves.
    let first = line.frames[0].bikes.clone();
    let last = line.frames[140].bikes.clone();
    for (a, b) in first.iter().zip(&last) {
        let (a, b) = (a.unwrap(), b.unwrap());
        assert!(
            (a.position[0] - b.position[0]).abs() + (a.position[2] - b.position[2]).abs() > 50.0
        );
    }
    // Written back, it reads the same.
    let again = vcr::Tape::parse(&tape.write()).unwrap();
    assert_eq!(again.records, tape.records);
    assert_eq!(again.riders, tape.riders);
}

#[test]
fn every_scene_sound_is_in_audio_res() {
    let Some(dir) = game_dir() else { return };
    let audio_bytes = archive(&dir, "AUDIO.RES");
    let audio = rs2::Archive::parse(&audio_bytes).unwrap();
    let mut files = Vec::new();
    walk(&dir.join("TERAFORM"), &mut files);
    let (mut sounds, mut crowds, mut missing) = (0, 0, Vec::new());
    for path in files {
        let raw = std::fs::read(&path).unwrap();
        let bytes = match Container::detect(&raw) {
            Container::Faoe => faoe::decode(&raw, &path.file_name().unwrap().to_string_lossy()).unwrap(),
            _ => raw,
        };
        let Ok(ar) = rs2::Archive::parse(&bytes) else { continue };
        for e in ar.entries().iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".scn")) {
            let scene = scn::Scene::parse(ar.data(e)).unwrap();
            crowds += scene.crowd as usize;
            for s in &scene.sounds {
                sounds += 1;
                if audio.get(&s.spec.file).is_none() {
                    missing.push(format!("{}: {}", e.name, s.spec.file));
                }
            }
        }
    }
    // The 17 stadiums and the 3 Moto-Tag arenas have a crowd.
    assert_eq!(crowds, 20);
    assert_eq!(sounds, 299);
    // Forestloop.wav is named in a scene but was never shipped.
    assert!(missing.iter().all(|m| m.to_ascii_lowercase().contains("forestloop")), "{missing:?}");
}
