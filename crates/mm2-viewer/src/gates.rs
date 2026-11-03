//! The Baja and Enduro checkpoint gates (`BajaRace.cpp`): each course's
//! `<course>.wpt` lists the gates in order (0x405330 reads it), and the
//! race puts up an arch at each, the last one the finish arch (0x405020).
//! Riders must pass the gates in turn; past the last a lap is done.

use glam::Vec3;
use mm2_formats::scn;
use mm2_physics::race::Line;

/// The game takes at most 34 gates (it gives up on 35 or more).
const MOST: usize = 34;
/// The gates stand this far above the ground (0x5557e0).
const RAISE: f32 = 2.0;
/// Half the arch's width and its height, from `Waypnt.slb` (both arches
/// are the same size), cut down as the game cuts its gate box (0.98 of the
/// width at 0x5557e4, 0.67 of the height at 0x5557e8).
const HALF_WIDTH: f32 = 18.5 * 0.98;
const HEIGHT: f32 = 19.4 * 0.67;
/// The arches' models, in `RES/MODELS.RES`.
const GATE: &str = "waypnt.slt";
const FINISH: &str = "Finish.slt";

/// One gate: where it stands and the way through it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gate {
    pub position: Vec3,
    pub look: Vec3,
}

/// A course's gates and the arch model it asks for (`DefaultSLT`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Waypoints {
    pub model: String,
    /// Level positions and directions; the heights come from the ground.
    pub points: Vec<([f32; 2], [f32; 2])>,
}

impl Waypoints {
    /// Reads a `.wpt`: `[Waypoints]` with `NumWaypoints` and, from 1,
    /// `Point<n>X`, `Point<n>Z`, `Point<n>LookX` and `Point<n>LookZ`.
    pub fn parse(text: &str) -> Waypoints {
        let ini = scn::Ini::parse(text);
        let Some(s) = ini.section("Waypoints") else {
            return Waypoints::default();
        };
        let num = |key: &str| s.get(key).and_then(|v| v.trim().parse::<f32>().ok());
        let count = num("NumWaypoints").unwrap_or(0.0) as usize;
        if count > MOST {
            return Waypoints::default();
        }
        let points = (1..=count)
            .map(|n| {
                let v = |k: &str| num(&format!("Point{n}{k}")).unwrap_or(0.0);
                ([v("X"), v("Z")], [v("LookX"), v("LookZ")])
            })
            .collect();
        Waypoints {
            model: s.get("DefaultSLT").unwrap_or(GATE).trim().to_string(),
            points,
        }
    }

    /// The gates on the ground `height` gives.
    pub fn gates(&self, height: impl Fn(f32, f32) -> f32) -> Vec<Gate> {
        self.points
            .iter()
            .map(|&([x, z], [lx, lz])| Gate {
                position: Vec3::new(x, height(x, z) + RAISE, z),
                look: Vec3::new(lx, 0.0, lz).normalize_or(Vec3::Z),
            })
            .collect()
    }

    /// The arches to put up, the last one the finish.
    pub fn placements(&self, gates: &[Gate]) -> Vec<scn::Placement> {
        gates
            .iter()
            .enumerate()
            .map(|(i, g)| scn::Placement {
                model: if i + 1 == gates.len() {
                    FINISH.to_string()
                } else {
                    self.model.clone()
                },
                position: g.position.into(),
                look: g.look.into(),
                up: [0.0, 1.0, 0.0],
                scale: [1.0; 3],
                lighting: Default::default(),
            })
            .collect()
    }
}

impl Gate {
    /// The line a rider must cross, going the gate's way.
    pub fn line(&self) -> Line {
        let right = Vec3::new(self.look.z, 0.0, -self.look.x);
        Line {
            centre: self.position - Vec3::Y * RAISE,
            forward: self.look,
            half_width: right * HALF_WIDTH,
            height: HEIGHT + RAISE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_gates() {
        let w = Waypoints::parse(
            "; a comment\n[Waypoints]\nNumWaypoints=2\n\nPoint1X=10\nPoint1Z=20\n\
             Point1LookX=0.0\nPoint1LookZ=1.0\nPoint2X=30\nPoint2Z=40\nPoint2LookX=1\nPoint2LookZ=0\n",
        );
        assert_eq!(w.model, "waypnt.slt");
        assert_eq!(
            w.points,
            [([10.0, 20.0], [0.0, 1.0]), ([30.0, 40.0], [1.0, 0.0])]
        );
        let gates = w.gates(|_, _| 5.0);
        assert_eq!(gates[0].position, Vec3::new(10.0, 7.0, 20.0));
        let p = w.placements(&gates);
        assert_eq!(
            (p[0].model.as_str(), p[1].model.as_str()),
            ("waypnt.slt", "Finish.slt")
        );
        // Through the first gate going +z, not back through it.
        let l = gates[0].line();
        let (a, b) = (Vec3::new(12.0, 5.0, 19.0), Vec3::new(12.0, 5.0, 21.0));
        assert!(l.crossed(a, b, true));
        assert!(!l.crossed(b, a, true));
        assert!(!l.crossed(a + Vec3::X * 20.0, b + Vec3::X * 20.0, true));
    }

    /// With the game's files beside the engine, every Baja course has
    /// its gates.
    #[test]
    fn baja_courses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../GAME/TERAFORM/BAJA/BAJA01.ENV");
        let Ok(bytes) = crate::read_archive(&path) else {
            return;
        };
        let ar = mm2_formats::rs2::Archive::parse(&bytes).unwrap();
        let wpts: Vec<_> = ar
            .entries()
            .iter()
            .filter(|e| e.name.to_ascii_lowercase().ends_with(".wpt"))
            .collect();
        assert_eq!(wpts.len(), 3);
        for e in wpts {
            let w = Waypoints::parse(&String::from_utf8_lossy(ar.data(e)));
            assert!(w.points.len() > 3, "{}", e.name);
        }
    }
}
