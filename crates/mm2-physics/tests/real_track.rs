//! Rides the bike across every real track's terrain. Skipped unless
//! `MM2_DATA` points at the game's `GAME` folder.

use glam::Vec3;
use mm2_formats::{faoe, rs2, trn, Container};
use mm2_physics::{Bike, Controls, Ground};
use std::path::{Path, PathBuf};

struct Track(trn::Terrain);

impl Track {
    fn height(&self, x: f32, z: f32) -> f32 {
        let t = &self.0;
        let max = (t.size - 1) as f32;
        let (tx, tz) = ((x / t.scale).clamp(0.0, max), (z / t.scale).clamp(0.0, max));
        let (x0, z0) = (tx.floor() as usize, tz.floor() as usize);
        let (x1, z1) = ((x0 + 1).min(t.size - 1), (z0 + 1).min(t.size - 1));
        let (fx, fz) = (tx.fract(), tz.fract());
        let row = |z| t.height(x0, z) * (1.0 - fx) + t.height(x1, z) * fx;
        (row(z0) * (1.0 - fz) + row(z1) * fz) * t.scale
    }
}

impl Ground for Track {
    fn terrain(&self, x: f32, z: f32) -> (f32, Vec3) {
        let e = 0.5;
        let dx = self.height(x + e, z) - self.height(x - e, z);
        let dz = self.height(x, z + e) - self.height(x, z - e);
        (self.height(x, z), Vec3::new(-dx, 2.0 * e, -dz).normalize())
    }
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
fn rides_every_track() {
    let Some(dir) = std::env::var_os("MM2_DATA").map(PathBuf::from) else {
        eprintln!("MM2_DATA not set, skipping");
        return;
    };
    let mut files = Vec::new();
    walk(&dir.join("TERAFORM"), &mut files);
    let mut tracks = 0;
    let (mut crashes, mut air, mut frames, mut fastest) = (Vec::new(), 0, 0, 0f32);
    for path in files {
        let raw = std::fs::read(&path).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        let bytes = match Container::detect(&raw) {
            Container::Rs2 => raw,
            Container::Faoe => faoe::decode(&raw, name).unwrap(),
            Container::Unknown => continue,
        };
        let ar = rs2::Archive::parse(&bytes).unwrap();
        for e in ar.entries() {
            if !e.name.to_ascii_lowercase().ends_with(".trn") {
                continue;
            }
            let track = Track(trn::parse(ar.data(e)).unwrap());
            let mid = (track.0.size / 2) as f32 * track.0.scale;
            let start = Vec3::new(mid, track.height(mid, mid), mid);
            let mut bike = Bike::new(&track, start, Vec3::Z);
            for i in 0..60 * 30 {
                let controls = Controls {
                    throttle: 1.0,
                    steer: ((i / 120) % 3) as f32 - 1.0,
                    ..Default::default()
                };
                let was = bike.crash.is_some();
                bike.step(&track, controls, 1.0 / 60.0);
                if !was {
                    if let Some((why, _)) = bike.crash {
                        crashes.push(why);
                    }
                }
                air += bike.airborne as usize;
                frames += 1;
                fastest = fastest.max(bike.velocity.length());
                if bike.velocity.length() > 400.0 && std::env::var_os("MM2_DEBUG").is_some() {
                    panic!("{name} {} frame {i}: {bike:?}", e.name);
                }
                assert!(
                    bike.position.is_finite() && bike.velocity.is_finite(),
                    "{name}: {} after {i} frames: {bike:?}",
                    e.name
                );
                // Never deep under the ground.
                let floor = track.height(bike.position.x, bike.position.z);
                assert!(bike.position.y > floor - 2.0, "{name}: {}: sank at frame {i}, y {} floor {floor} pos {:?} v {:?} air {} crash {:?}", e.name, bike.position.y, bike.position, bike.velocity, bike.airborne, bike.crash);
            }
            tracks += 1;
        }
    }
    assert!(tracks > 0);
    eprintln!(
        "rode {tracks} tracks: {} crashes {crashes:?}, airborne {:.0}%, fastest {fastest:.0} ft/s",
        crashes.len(),
        air as f32 * 100.0 / frames as f32
    );
}
