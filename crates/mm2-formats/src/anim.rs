//! Animated models: `.mcf` descriptions and `.mot` motions, packed in
//! `RES/OBJECTS.RES`.
//!
//! An `.mcf` is INI text naming the model (`SLTFile`, loaded as `.slb`)
//! and its motions:
//!
//! ```text
//! [General info]  NumberOfMotions, SLTFile, VUTfilename
//! [MOTION n]      NAME, VUE (the .MOT file), FPS, FrameCount, Looping
//! ```
//!
//! A `.mot` holds a transform for each animated model frame on each
//! animation frame (loader at 0x4a7290):
//!
//! ```text
//! u32 frame_count, u32 key_count
//! keys[frame_count][key_count]
//!     u8 model_frame (1-based), f32 look[3], f32 up[3], f32 position[3]
//! 256 bytes of leftover memory
//! ```
//!
//! Each key replaces the frame's transform relative to its parent. A root
//! frame's position is in track coordinates, before the scene's offset.

use crate::reader::Reader;
use crate::scn::Ini;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct Mcf {
    /// The model, as named in the file (an `.slt`).
    pub model: String,
    pub motions: Vec<MotionInfo>,
}

#[derive(Debug, Clone)]
pub struct MotionInfo {
    pub name: String,
    /// The `.mot` file.
    pub file: String,
    pub fps: f32,
    pub frame_count: u32,
    pub looping: bool,
}

impl Mcf {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let ini = Ini::parse(&String::from_utf8_lossy(bytes));
        let general = ini
            .section("General info")
            .ok_or(Error::Corrupt("mcf has no [General info]"))?;
        let model = general
            .get("SLTFile")
            .ok_or(Error::Corrupt("mcf names no model"))?
            .to_string();
        let count: usize = general
            .get("NumberOfMotions")
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0);
        let motions = (1..=count)
            .filter_map(|i| ini.section(&format!("MOTION {i}")))
            .filter_map(|s| {
                Some(MotionInfo {
                    name: s.get("NAME")?.to_string(),
                    file: s.get("VUE")?.to_string(),
                    fps: s
                        .get("FPS")
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(15.0),
                    frame_count: s
                        .get("FrameCount")
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0),
                    looping: s.get("Looping").is_some_and(|v| {
                        matches!(v.trim().to_ascii_lowercase().as_str(), "t" | "true" | "yes")
                    }),
                })
            })
            .collect();
        Ok(Mcf { model, motions })
    }
}

/// One model frame's transform on one animation frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// Index into the model's frames.
    pub frame: usize,
    pub look: [f32; 3],
    pub up: [f32; 3],
    pub position: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct Motion {
    /// `frames[i]` holds the keys for animation frame `i`.
    pub frames: Vec<Vec<Key>>,
}

const MAX_KEYS: u32 = 1 << 24;

impl Motion {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes, 0);
        let frame_count = r.u32("motion frame count")?;
        let key_count = r.u32("motion key count")?;
        if frame_count.saturating_mul(key_count) > MAX_KEYS {
            return Err(Error::Corrupt("motion is too large"));
        }
        let mut frames = Vec::with_capacity(frame_count as usize);
        for _ in 0..frame_count {
            let mut keys = Vec::with_capacity(key_count as usize);
            for _ in 0..key_count {
                let frame = r.u8("key frame")? as usize;
                let mut v = [0.0; 9];
                for f in &mut v {
                    *f = r.f32("key")?;
                }
                keys.push(Key {
                    frame: frame
                        .checked_sub(1)
                        .ok_or(Error::Corrupt("motion key for frame 0"))?,
                    look: [v[0], v[1], v[2]],
                    up: [v[3], v[4], v[5]],
                    position: [v[6], v[7], v[8]],
                });
            }
            frames.push(keys);
        }
        Ok(Motion { frames })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mcf() {
        let text = "[General info]\r\nNumberOfMotions=1\r\nSLTFile=StartGate.slt\r\n\
                    [MOTION 1]\r\nNAME=DropGate\r\nVUE=Gate.MOT\r\nFPS=30\r\n\
                    FrameCount=7\r\nLooping=FALSE\r\n";
        let m = Mcf::parse(text.as_bytes()).unwrap();
        assert_eq!(m.model, "StartGate.slt");
        let motion = &m.motions[0];
        assert_eq!(
            (motion.name.as_str(), motion.file.as_str()),
            ("DropGate", "Gate.MOT")
        );
        assert_eq!(
            (motion.fps, motion.frame_count, motion.looping),
            (30.0, 7, false)
        );
    }

    #[test]
    fn parses_motion() {
        let mut b = Vec::new();
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        for i in 0..2 {
            b.push(1);
            for f in [0.0f32, 0.0, 1.0, 0.0, 1.0, 0.0, i as f32, 2.0, 3.0] {
                b.extend_from_slice(&f.to_le_bytes());
            }
        }
        b.extend_from_slice(&[0; 256]);
        let m = Motion::parse(&b).unwrap();
        assert_eq!(m.frames.len(), 2);
        assert_eq!(m.frames[1][0].frame, 0);
        assert_eq!(m.frames[1][0].position, [1.0, 2.0, 3.0]);
    }
}
