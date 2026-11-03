//! The port's own graphics upgrades, none of them the game's: anti-aliasing,
//! per-pixel lighting, shadows from the sun for every object, a longer view
//! with soft haze, HDR with bloom, ambient occlusion and a sun in the sky.
//!
//! Off unless asked for (`--enhanced`), so the game draws as it always did.
//! They change only the drawing: the riding, the camera and the clock are
//! the same either way. With them on, F3 switches between the enhanced look
//! and the original one, to compare.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

/// The colour the enhanced scene is drawn in before it is tone mapped.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Which upgrades are on.
#[derive(Debug, Clone, Copy)]
pub struct Enhanced {
    /// Samples a pixel for anti-aliasing (1 is none).
    pub msaa: u32,
    /// Per-pixel lighting, bumps from the textures, highlights.
    pub lighting: bool,
    /// The sun's shadows from every object.
    pub shadows: bool,
    /// The whole track drawn, fading into haze rather than ending at the fog.
    pub distance: bool,
    /// Bright light glows.
    pub bloom: bool,
    /// Ambient occlusion: corners and contacts darkened.
    pub ao: bool,
    /// A glow round the sun and haze along the horizon.
    pub sky: bool,
    /// The bolder look: colour, contrast and a vignette.
    pub grade: bool,
    /// The ground re-made from photo materials and lit by the sun.
    pub ground: bool,
    /// Grass growing on the grassy ground near the camera.
    pub grass: bool,
    /// A real sky with clouds in place of the painted one.
    pub clouds: bool,
    /// Tyre ruts that stay, and clouds of dust (spray on wet ground)
    /// behind the bikes.
    pub ruts: bool,
    pub dust: bool,
    /// Shiny paint and metal on the bikes, and smoother riders.
    pub bikes: bool,
    /// Shafts of sunlight past hills, trees and riders.
    pub shafts: bool,
    /// Replays filmed as through a lens: depth of field and motion blur.
    pub cinematic: bool,
    /// Smooth hills round the eye, drawn only.
    pub smooth: bool,
    /// Props along an outdoor course and a paddock by the start, drawn
    /// only.
    pub scenery: bool,
    /// Mud and dust caking the bikes and riders as they ride.
    pub mud: bool,
    /// Photo materials' grain (wood, metal, concrete, straw) on the
    /// objects of the trial track (materials::surfaced).
    pub surfaces: bool,
    /// How dirty the bikes start, 0 to 1.
    pub mud_start: f32,
    /// How strongly the new ground's detail shows, 0 to 1.
    pub detail: f32,
    pub weather: Weather,
    /// The hour the sun is placed at, 0 to 24; the scene's own sun if none.
    pub time: Option<f32>,
    /// The most pixels the 3D scene is drawn with: on a bigger screen
    /// (Retina, 4K) it is drawn smaller and stretched over it, the menus
    /// and race display still sharp. None for the screen's own.
    pub pixels: Option<u32>,
}

/// The weather, for looks only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weather {
    Clear,
    Overcast,
    Rain,
}

impl Default for Enhanced {
    fn default() -> Self {
        Enhanced {
            msaa: 4,
            lighting: true,
            shadows: true,
            distance: true,
            bloom: true,
            ao: true,
            sky: true,
            grade: false,
            ground: true,
            grass: true,
            clouds: false,
            ruts: false,
            dust: true,
            bikes: true,
            shafts: true,
            cinematic: false,
            smooth: true,
            scenery: false,
            mud: true,
            surfaces: true,
            mud_start: 0.0,
            detail: 0.5,
            weather: Weather::Clear,
            time: None,
            pixels: Some(QHD),
        }
    }
}

/// 2560 × 1440.
const QHD: u32 = 2560 * 1440;

/// The quality presets (`quality=`): what each turns off, and the most
/// pixels it draws the scene with.
fn preset(e: &mut Enhanced, name: &str) -> Result<(), String> {
    match name {
        "low" => {
            e.msaa = 1;
            e.ao = false;
            e.shafts = false;
            e.smooth = false;
            e.cinematic = false;
            e.grass = false;
            e.pixels = Some(1280 * 720);
        }
        "medium" => {
            e.msaa = 2;
            e.shafts = false;
            e.smooth = false;
            e.pixels = Some(1920 * 1080);
        }
        "high" => {}
        "ultra" => {
            e.msaa = 8;
            e.pixels = None;
        }
        _ => return Err(format!("quality must be low, medium, high or ultra, not {name}")),
    }
    Ok(())
}

impl Enhanced {
    /// The size to draw the 3D scene at on a `screen` of that size: the
    /// screen's own, or smaller with the same shape when it has more than
    /// [`Enhanced::pixels`].
    pub fn scene_size(&self, screen: (u32, u32)) -> (u32, u32) {
        let have = screen.0 as f64 * screen.1 as f64;
        match self.pixels {
            Some(most) if have > most as f64 => {
                let k = (most as f64 / have).sqrt();
                (((screen.0 as f64 * k).round() as u32).max(1), ((screen.1 as f64 * k).round() as u32).max(1))
            }
            _ => screen,
        }
    }
}

impl Enhanced {
    /// From `--enhanced`'s list: each upgrade's name turns it off with a
    /// `no-` before it (`no-bloom,no-ao`), and `msaa=N` sets the samples
    /// (1, 2, 4 or 8).
    pub fn parse(list: &str) -> Result<Enhanced, String> {
        let mut e = Enhanced::default();
        // An hour or weather of its own needs the drawn sky, unless asked
        // not to have it: the scene's sky box shows its own.
        let wants_sky = list.contains("time=") || list.contains("weather=");
        e.clouds = wants_sky && !list.contains("no-clouds");
        for item in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (off, name) = match item.strip_prefix("no-") {
                Some(name) => (true, name),
                None => (false, item),
            };
            if let Some(w) = name.strip_prefix("weather=") {
                e.weather = match w {
                    "clear" => Weather::Clear,
                    "overcast" | "cloudy" => Weather::Overcast,
                    "rain" | "wet" => Weather::Rain,
                    _ => return Err(format!("weather must be clear, overcast or rain, not {w}")),
                };
                continue;
            }
            if let Some(t) = name.strip_prefix("time=") {
                e.time = Some(match t {
                    "dawn" => 7.0,
                    "morning" => 9.0,
                    "noon" => 12.5,
                    "afternoon" => 15.5,
                    "dusk" | "sunset" => 18.0,
                    _ => match t.parse::<f32>() {
                        Ok(h) if (0.0..=24.0).contains(&h) => h,
                        _ => return Err(format!("time must be dawn, morning, noon, afternoon, dusk or an hour 0-24, not {t}")),
                    },
                });
                continue;
            }
            if let Some(d) = name.strip_prefix("detail=") {
                e.detail = match d.parse::<f32>() {
                    Ok(d) if (0.0..=1.0).contains(&d) => d,
                    _ => return Err(format!("detail must be 0 to 1, not {d}")),
                };
                continue;
            }
            if let Some(m) = name.strip_prefix("mud=") {
                e.mud_start = match m.parse::<f32>() {
                    Ok(m) if (0.0..=1.0).contains(&m) => m,
                    _ => return Err(format!("mud must be 0 to 1, not {m}")),
                };
                continue;
            }
            if let Some(q) = name.strip_prefix("quality=") {
                preset(&mut e, q)?;
                continue;
            }
            // The most pixels to draw the scene with: a size (1920x1080),
            // a height (1080p), or "native" for the screen's own.
            if let Some(r) = name.strip_prefix("resolution=").or(name.strip_prefix("res=")) {
                e.pixels = match r {
                    "native" | "full" => None,
                    _ => {
                        let px = match r.trim_end_matches('p').split_once('x') {
                            Some((w, h)) => w.parse::<u32>().ok().zip(h.parse::<u32>().ok()).map(|(w, h)| w * h),
                            None => r.trim_end_matches('p').parse::<u32>().ok().map(|h| h * h * 16 / 9),
                        };
                        match px {
                            Some(px) if px >= 320 * 200 => Some(px),
                            _ => return Err(format!("resolution must be like 1920x1080, 1080p or native, not {r}")),
                        }
                    }
                };
                continue;
            }
            if let Some(n) = name.strip_prefix("msaa=") {
                e.msaa = match n.parse() {
                    Ok(n @ (1 | 2 | 4 | 8)) => n,
                    _ => return Err(format!("msaa must be 1, 2, 4 or 8, not {n}")),
                };
                continue;
            }
            let on = !off;
            match name {
                "msaa" | "aa" => e.msaa = if on { 4 } else { 1 },
                "lighting" => e.lighting = on,
                "shadows" => e.shadows = on,
                "distance" => e.distance = on,
                "bloom" => e.bloom = on,
                "ao" => e.ao = on,
                "sky" => e.sky = on,
                "grade" => e.grade = on,
                "ground" => e.ground = on,
                "grass" => e.grass = on,
                "clouds" => e.clouds = on,
                "ruts" => e.ruts = on,
                "dust" => e.dust = on,
                "bikes" => e.bikes = on,
                "shafts" => e.shafts = on,
                "cinematic" | "blur" => e.cinematic = on,
                "mud" | "splash" => e.mud = on,
                "smooth" => e.smooth = on,
                "scenery" | "props" => e.scenery = on,
                "surfaces" | "materials" => e.surfaces = on,
                _ => {
                    return Err(format!(
                        "unknown upgrade {item:?}: try no-msaa, no-lighting, no-shadows, no-distance, no-bloom, no-ao, no-sky, no-grade, no-ground, no-grass, no-clouds, no-ruts, no-dust, no-bikes, no-shafts, no-cinematic, no-mud, no-smooth, no-scenery, no-surfaces, mud=0.5, detail=0.5, msaa=8, quality=low/medium/high/ultra, resolution=1080p, weather=rain or time=dusk"
                    ))
                }
            }
        }
        Ok(e)
    }
}

static ENHANCED: RwLock<Option<Enhanced>> = RwLock::new(None);
/// The anti-aliasing samples the GPU can draw the scene with, beyond the
/// 1 and 4 every GPU has.
static SAMPLE_COUNTS: OnceLock<Vec<u32>> = OnceLock::new();

/// Says which samples the GPU can draw the scene with.
pub fn set_sample_counts(counts: Vec<u32>) {
    let _ = SAMPLE_COUNTS.set(counts);
}

/// The most samples, up to `wanted`, the GPU can draw the scene with.
pub fn samples(wanted: u32) -> u32 {
    let known = SAMPLE_COUNTS.get().map_or(&[][..], |c| &c[..]);
    [1, 2, 4, 8]
        .into_iter()
        .filter(|&c| c <= wanted && (c == 1 || c == 4 || known.contains(&c)))
        .max()
        .unwrap_or(1)
}
/// F3's switch: false shows the original look on the enhanced drawing.
static SHOWING: AtomicBool = AtomicBool::new(true);

/// Turns the upgrades on, before anything is drawn.
pub fn enable(e: Enhanced) {
    *ENHANCED.write().unwrap_or_else(|e| e.into_inner()) = Some(e);
}

/// The upgrades, when on.
pub fn enhanced() -> Option<Enhanced> {
    *ENHANCED.read().unwrap_or_else(|e| e.into_inner())
}

/// The upgrades as they show now: none while F3 has switched to the
/// original look.
pub fn showing() -> Option<Enhanced> {
    enhanced().filter(|_| SHOWING.load(Ordering::Relaxed))
}

/// F3: between the enhanced look and the original one. Returns whether
/// the enhanced look now shows.
pub fn toggle() -> bool {
    !SHOWING.fetch_xor(true, Ordering::Relaxed)
}

/// How sharply textures seen at a slant are filtered: the most the GPU
/// allows with the enhanced graphics, else as the port always has.
pub fn anisotropy() -> u16 {
    if enhanced().is_some() {
        16
    } else {
        8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_size() {
        let e = Enhanced::default();
        assert_eq!(e.scene_size((1920, 1080)), (1920, 1080));
        assert_eq!(e.scene_size((3840, 2160)), (2560, 1440));
        assert_eq!(Enhanced::parse("resolution=native").unwrap().scene_size((3840, 2160)), (3840, 2160));
        assert_eq!(Enhanced::parse("res=1080p").unwrap().scene_size((3840, 2160)), (1920, 1080));
        let low = Enhanced::parse("quality=low").unwrap();
        assert_eq!((low.msaa, low.ao, low.scene_size((2560, 1440))), (1, false, (1280, 720)));
        assert!(Enhanced::parse("quality=potato").is_err());
    }

    #[test]
    fn upgrades_parse() {
        let all = Enhanced::parse("").unwrap();
        assert_eq!(all.msaa, 4);
        assert!(all.lighting && all.shadows && all.distance && all.bloom && all.ao && all.sky);
        let some = Enhanced::parse("no-bloom, no-ao,msaa=8").unwrap();
        assert!(!some.bloom && !some.ao && some.shadows);
        assert_eq!(some.msaa, 8);
        assert_eq!(Enhanced::parse("no-msaa").unwrap().msaa, 1);
        assert!(Enhanced::parse("msaa=3").is_err());
        assert!(Enhanced::parse("raytracing").is_err());
        let w = Enhanced::parse("weather=rain,time=dusk,no-grass").unwrap();
        assert_eq!(w.weather, Weather::Rain);
        assert_eq!(w.time, Some(18.0));
        assert!(!w.grass && w.ground);
        assert!(!Enhanced::parse("no-shafts").unwrap().shafts);
        assert!(Enhanced::parse("time=25").is_err());
    }
}
