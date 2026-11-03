//! `.scn` track scene descriptions.
//!
//! Scenes are INI text: `[Section]` headers, `Key=Value` lines and `;`
//! comments. They name the terrain, the lights, the fog colour, the start
//! position, and every static model and animation placed on the track.

use crate::{Error, Result};

/// A parsed INI file, keeping sections in file order.
#[derive(Debug, Clone, Default)]
pub struct Ini {
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, Default)]
pub struct Section {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

impl Section {
    /// Looks up a key, ignoring case. The last occurrence wins.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// `UseLighting`: on unless it says `F` (0x4f2ee0, 0x4f1c60).
    fn lighting(&self) -> Lighting {
        match self.get("UseLighting").and_then(|v| v.trim().chars().next()) {
            Some('F' | 'f' | '0') => Lighting::Off,
            _ => Lighting::Lit,
        }
    }

    fn vec3(&self, key: &str) -> Option<[f32; 3]> {
        parse_floats(self.get(key)?)
    }
}

impl Ini {
    pub fn parse(text: &str) -> Self {
        let mut ini = Ini::default();
        for line in text.lines() {
            let line = line.split(';').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                ini.sections.push(Section {
                    name: name.trim().to_string(),
                    entries: Vec::new(),
                });
            } else if let Some((key, value)) = line.split_once('=') {
                if let Some(section) = ini.sections.last_mut() {
                    section
                        .entries
                        .push((key.trim().to_string(), unquote(value.trim()).to_string()));
                }
            }
        }
        ini
    }

    /// Looks up a section, ignoring case. The first occurrence wins.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

fn parse_floats<const N: usize>(value: &str) -> Option<[f32; N]> {
    let mut out = [0.0; N];
    let mut parts = value.split(',');
    for slot in &mut out {
        *slot = parts.next()?.trim().parse().ok()?;
    }
    Some(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightKind {
    Ambient,
    Directional,
    /// A point light that lights each object from one direction, the way
    /// to it from the object (the game's type 2; the sun of the quarries
    /// and Moto-Tag scenes, at infinity).
    ObjectPoint,
    Other,
}

#[derive(Debug, Clone)]
pub struct Light {
    pub kind: LightKind,
    /// Colour as 0..=255 RGB.
    pub color: [f32; 3],
    /// Direction the light travels, for directional lights.
    pub direction: Option<[f32; 3]>,
    pub position: Option<[f32; 3]>,
}

/// How the game lights a model's vertices (0x49fa50, 0x4577d0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lighting {
    /// By the scene's lights (`UseLighting`, on unless `F`).
    #[default]
    Lit,
    /// By the colours stored in the model (`UseLighting=F`).
    Off,
    /// An eco-system plant: by the scene's lights, with the sun level.
    Plant,
}

/// A model placed on the track, from a `[ModelN]` section.
#[derive(Debug, Clone)]
pub struct Placement {
    /// The `.slt` model name, as written in the scene.
    pub model: String,
    pub position: [f32; 3],
    pub look: [f32; 3],
    pub up: [f32; 3],
    pub scale: [f32; 3],
    pub lighting: Lighting,
}

/// An animated model, from an `[AnimationN]` section.
#[derive(Debug, Clone)]
pub struct Animation {
    /// The `.mcf` model name.
    pub model: String,
    /// Where the model's root goes, from `Offset` or `Position`.
    pub offset: [f32; 3],
    pub look: [f32; 3],
    pub up: [f32; 3],
    pub scale: [f32; 3],
    /// The motion to play first, by name.
    pub motion: Option<String>,
    pub lighting: Lighting,
}

/// A scripted vehicle, from an `[AnimationN]` section with
/// `AnimationType=Procedural` (CarProcedural loader 0x430920).
#[derive(Debug, Clone)]
pub struct Procedural {
    /// The `.slt` model name.
    pub model: String,
    pub offset: [f32; 3],
    /// The path file in `RES/PROCVUE.RES`.
    pub vue: String,
    pub fps: f32,
    /// How far behind the path's point it runs, ft (a trailer's 21).
    pub lag_distance: f32,
    pub front_wheels_turn: bool,
}

/// A stadium's place in the scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StadiumFrame {
    pub top: f32,
    pub bottom: f32,
    pub scale: f32,
    pub position: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct Scene {
    pub name: String,
    pub terrain_file: String,
    pub start_position: [f32; 3],
    pub start_direction: [f32; 3],
    pub lights: Vec<Light>,
    pub fog_color: Option<[f32; 3]>,
    /// `[Fog] Visibility` and `Haziness`, 0 to 1 (0x4f0b40; 1.0 and 0.5
    /// when missing or out of range, which every shipped scene leaves them).
    pub fog_visibility: f32,
    pub fog_haziness: f32,
    /// The sky box in `RES/SKIES.RES` (`[Environment] CubeFile`).
    pub cube_file: Option<String>,
    /// `[Environment] DetailTextureMapName`, laid over the terrain (the
    /// game's `GrayNoise.tga` when missing, 0x50b140).
    pub detail_texture: Option<String>,
    /// `[Environment] ParticleTextureMapName`, the dirt and dust sheet.
    pub particle_texture: Option<String>,
    pub stadium: Option<String>,
    /// Where the stadium's wall outline stands: its top and bottom, scale
    /// and position (`[Stadium]` Top, Bottom, Scale, Position).
    pub stadium_frame: Option<StadiumFrame>,
    /// Where the podium stands after a race and the way it faces
    /// (`[SceneInfo] PodiumPosition`, `PodiumDirection`; 0x45e490 takes
    /// them only when the direction is not zero).
    pub podium: Option<([f32; 3], [f32; 3])>,
    /// Extra texture archives in `RES`, such as a stadium's `.tex`.
    pub resource_files: Vec<String>,
    pub models: Vec<Placement>,
    pub animations: Vec<Animation>,
    pub procedural: Vec<Procedural>,
    /// Each terrain surface type's ground (`[TerrainZone2..8]`, 0x4f0d50;
    /// zone 1 is type 0 and stays 1.0).
    pub zones: [Zone; 8],
    /// A crowd in the stands (`[Sounds] CrowdPresent`, 0x4f43c0 → scene
    /// +0xbc): the race loads the crowd's loop and its reactions.
    pub crowd: bool,
    /// The sounds the scene's objects make, and its `[StaticSoundN]`s.
    pub sounds: Vec<SceneSound>,
}

/// A sound a scene object makes (`SoundResourceFile` and the keys
/// 0x4f48c0 reads beside it).
#[derive(Debug, Clone, PartialEq)]
pub struct SoundSpec {
    /// The `.wav` in `RES/AUDIO.RES`.
    pub file: String,
    /// DirectSound 3D's minimum and maximum distances, ft.
    pub min_distance: f32,
    pub max_distance: f32,
    pub looping: bool,
    /// Plays once, the first time the camera comes within
    /// `one_shot_distance` (20 ft unless given).
    pub one_shot: bool,
    pub one_shot_distance: f32,
    /// The chance, in percent, that it plays each time it comes in range.
    pub trigger_percent: f32,
    /// `SoundForce2D`: the game puts it in a software buffer.
    pub force_2d: bool,
}

/// What a scene sound is attached to.
#[derive(Debug, Clone, PartialEq)]
pub enum SoundSite {
    /// A static model, by index into `models`.
    Model(usize),
    /// An animated model, by index into `animations`, and the frame named
    /// by `SoundAttachToObject` (its root when none).
    Animation(usize, Option<String>),
    /// A scripted vehicle, by index into `procedural`.
    Procedural(usize),
    /// A `[StaticSoundN]` emitter at a fixed place.
    Fixed([f32; 3]),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneSound {
    pub spec: SoundSpec,
    pub site: SoundSite,
}

impl Section {
    /// The sound named in this section, if any (0x4f48c0): `.wav` is added
    /// to the file when missing, and "None" names none.
    fn sound(&self) -> Option<SoundSpec> {
        let file = self.get("SoundResourceFile")?.trim();
        if file.is_empty() || file.eq_ignore_ascii_case("none") {
            return None;
        }
        let file = if file.to_ascii_lowercase().contains(".wav") {
            file.to_string()
        } else {
            format!("{file}.wav")
        };
        let flag = |k: &str| {
            self.get(k).is_some_and(|v| {
                let v = v.trim();
                v.eq_ignore_ascii_case("T") || v.eq_ignore_ascii_case("true") || v == "1"
            })
        };
        let num = |k: &str, d: f32| self.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(d);
        Some(SoundSpec {
            file,
            min_distance: num("SoundMinDistance", 0.0),
            max_distance: num("SoundMaxDistance", 0.0),
            looping: flag("SoundLooping"),
            one_shot: flag("SoundOneShot"),
            one_shot_distance: num("SoundOneShotDistance", 20.0),
            trigger_percent: num("SoundRandomTriggerPercent", 100.0),
            force_2d: flag("SoundForce2D"),
        })
    }
}

/// How a surface type grips (`[TerrainZoneN]`, each 1.0 when not given).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    /// SurfaceFriction: the tyres' braking and side friction (tyre+0x144).
    pub friction: f32,
    /// SurfaceDrag.
    pub drag: f32,
    /// SurfaceTraction: the rear's grip under power (tyre+0x284).
    pub traction: f32,
    /// GenerateDust and GenerateDirt: whether the tyres raise dust and
    /// throw dirt here (scene +0x400 and +0x408 by surface type).
    pub dust: bool,
    pub dirt: bool,
}

impl Default for Zone {
    fn default() -> Self {
        Zone {
            friction: 1.0,
            drag: 1.0,
            traction: 1.0,
            dust: true,
            dirt: true,
        }
    }
}

impl Scene {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let text = String::from_utf8_lossy(bytes);
        let ini = Ini::parse(&text);
        let info = ini
            .section("SceneInfo")
            .ok_or(Error::Corrupt("scene has no [SceneInfo]"))?;
        let env = ini
            .section("Environment")
            .ok_or(Error::Corrupt("scene has no [Environment]"))?;

        let count = |section: &str, key: &str| -> usize {
            ini.section(section)
                .and_then(|s| s.get(key))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0)
        };

        let lights = (1..=count("Lights", "NumberOfLights"))
            .filter_map(|i| ini.section(&format!("Light{i}")))
            .map(|s| {
                let kind = match s.get("Type").map(|t| t.to_ascii_lowercase()) {
                    Some(t) if t == "ambient" => LightKind::Ambient,
                    Some(t) if t == "directional" => LightKind::Directional,
                    Some(t) if t == "objectpoint" => LightKind::ObjectPoint,
                    _ => LightKind::Other,
                };
                let direction = s.vec3("LookVector").or_else(|| {
                    let from = s.vec3("Position")?;
                    let to = s.vec3("TargetPosition")?;
                    Some([to[0] - from[0], to[1] - from[1], to[2] - from[2]])
                });
                Light {
                    kind,
                    color: s.vec3("ColorRGB").unwrap_or([255.0; 3]),
                    direction,
                    position: s.vec3("Position"),
                }
            })
            .collect();

        let mut sounds = Vec::new();
        let mut models = Vec::new();
        for s in (1..=count("StaticModels", "NumberOfStaticModels"))
            .filter_map(|i| ini.section(&format!("Model{i}")))
        {
            let placement = (|| {
                Some(Placement {
                    model: s.get("SLT")?.to_string(),
                    position: s.vec3("Position")?,
                    look: s.vec3("LookVector").unwrap_or([0.0, 0.0, 1.0]),
                    up: s.vec3("UpVector").unwrap_or([0.0, 1.0, 0.0]),
                    scale: s.vec3("Scale").unwrap_or([1.0; 3]),
                    lighting: s.lighting(),
                })
            })();
            if let Some(p) = placement {
                if let Some(spec) = s.sound() {
                    sounds.push(SceneSound {
                        spec,
                        site: SoundSite::Model(models.len()),
                    });
                }
                models.push(p);
            }
        }

        let mut animations = Vec::new();
        for s in (1..=count("Animations", "NumberOfAnimations"))
            .filter_map(|i| ini.section(&format!("Animation{i}")))
        {
            let animation = (|| {
                Some(Animation {
                    model: s.get("MCF")?.to_string(),
                    offset: s
                        .vec3("Offset")
                        .or_else(|| s.vec3("Position"))
                        .unwrap_or([0.0; 3]),
                    look: s.vec3("LookVector").unwrap_or([0.0, 0.0, 1.0]),
                    up: s.vec3("UpVector").unwrap_or([0.0, 1.0, 0.0]),
                    scale: s.vec3("Scale").unwrap_or([1.0; 3]),
                    motion: s.get("MotionToPlay1").map(str::to_string),
                    lighting: s.lighting(),
                })
            })();
            if let Some(a) = animation {
                if let Some(spec) = s.sound() {
                    let frame = s
                        .get("SoundAttachToObject")
                        .map(|f| f.trim().to_string())
                        .filter(|f| !f.is_empty());
                    sounds.push(SceneSound {
                        spec,
                        site: SoundSite::Animation(animations.len(), frame),
                    });
                }
                animations.push(a);
            }
        }

        let procedural_sections: Vec<&Section> = (1..=count("Animations", "NumberOfAnimations"))
            .filter_map(|i| ini.section(&format!("Animation{i}")))
            .filter(|s| {
                s.get("AnimationType")
                    .is_some_and(|t| t.trim().eq_ignore_ascii_case("Procedural"))
            })
            .collect();
        let mut procedural = Vec::new();
        for s in procedural_sections {
            let vehicle = (|| {
                let num =
                    |k: &str, d: f32| s.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(d);
                Some(Procedural {
                    model: s.get("SLT")?.trim().to_string(),
                    offset: s.vec3("Offset").unwrap_or([0.0; 3]),
                    vue: s.get("VUE")?.trim().to_string(),
                    fps: num("FPS", 15.0),
                    lag_distance: num("LagDistance", 0.0),
                    front_wheels_turn: s
                        .get("FrontWheelsTurn")
                        .is_some_and(|v| v.trim().eq_ignore_ascii_case("T")),
                })
            })();
            if let Some(v) = vehicle {
                if let Some(spec) = s.sound() {
                    sounds.push(SceneSound {
                        spec,
                        site: SoundSite::Procedural(procedural.len()),
                    });
                }
                procedural.push(v);
            }
        }

        let sounds_section = ini.section("Sounds");
        let crowd = sounds_section
            .and_then(|s| s.get("CrowdPresent"))
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("T") || v.trim() == "1");
        for s in (1..=count("Sounds", "NumStaticSounds"))
            .filter_map(|i| ini.section(&format!("StaticSound{i}")))
        {
            // Only emitters are made (0x4f43c0); they need a place.
            let emitter = s
                .get("SoundType")
                .is_none_or(|t| t.trim().eq_ignore_ascii_case("Emitter"));
            if let (true, Some(spec), Some(at)) = (emitter, s.sound(), s.vec3("Position")) {
                sounds.push(SceneSound {
                    spec,
                    site: SoundSite::Fixed(at),
                });
            }
        }

        // A missing zone raises dust and dirt, but for the eighth, the
        // solid objects (0x4f0d50); zone 1's grip is fixed at 1.
        let mut zones = [Zone::default(); 8];
        zones[7].dust = false;
        zones[7].dirt = false;
        for (i, zone) in zones.iter_mut().enumerate() {
            if let Some(s) = ini.section(&format!("TerrainZone{}", i + 1)) {
                let num = |k: &str| s.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(1.0);
                let flag = |k: &str| {
                    s.get(k)
                        .and_then(|v| v.trim().chars().next())
                        .is_none_or(|c| matches!(c, 'T' | 't' | '1' | 'Y' | 'y'))
                };
                *zone = Zone {
                    friction: if i == 0 { 1.0 } else { num("SurfaceFriction") },
                    drag: if i == 0 { 1.0 } else { num("SurfaceDrag") },
                    traction: if i == 0 { 1.0 } else { num("SurfaceTraction") },
                    dust: flag("GenerateDust"),
                    dirt: flag("GenerateDirt"),
                };
            }
        }

        // The game checks Haziness against Visibility's range test, so an
        // out-of-range Visibility resets Haziness too (0x4f0b40).
        let fog_value = |k: &str| {
            ini.section("Fog")
                .and_then(|s| s.get(k))
                .and_then(|v| v.trim().parse::<f32>().ok())
        };
        let visible_ok = fog_value("Visibility").is_none_or(|v| (0.0..=1.0).contains(&v));
        let fog_number = |k: &str, d: f32| match fog_value(k) {
            Some(v) if visible_ok && (k != "Visibility" || (0.0..=1.0).contains(&v)) => v,
            _ => d,
        };
        Ok(Scene {
            crowd,
            sounds,
            name: info.get("SceneName").unwrap_or("").to_string(),
            terrain_file: env.get("TerrainFile").unwrap_or("").to_string(),
            start_position: info.vec3("DefaultStartPosition").unwrap_or([0.0; 3]),
            start_direction: info
                .vec3("DefaultStartDirection")
                .unwrap_or([0.0, 0.0, 1.0]),
            lights,
            fog_color: ini.section("Fog").and_then(|s| s.vec3("ColorRGB")),
            fog_visibility: fog_number("Visibility", 1.0),
            fog_haziness: fog_number("Haziness", 0.5),
            cube_file: env
                .get("CubeFile")
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty()),
            detail_texture: env
                .get("DetailTextureMapName")
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty()),
            particle_texture: env
                .get("ParticleTextureMapName")
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty()),
            podium: info
                .vec3("PodiumDirection")
                .filter(|d| d.iter().any(|&v| v != 0.0))
                .map(|d| (info.vec3("PodiumPosition").unwrap_or([0.0; 3]), d)),
            stadium_frame: ini.section("Stadium").map(|s| {
                let num =
                    |k: &str, d: f32| s.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(d);
                StadiumFrame {
                    top: num("Top", 500.0),
                    bottom: num("Bottom", 0.0),
                    scale: num("Scale", 1.0),
                    position: s.vec3("Position").unwrap_or([0.0; 3]),
                }
            }),
            stadium: ini
                .section("Stadium")
                .and_then(|s| s.get("FileName"))
                .map(str::to_string),
            resource_files: ini
                .section("ResourceFiles")
                .map(|s| s.entries.iter().map(|(_, v)| v.clone()).collect())
                .unwrap_or_default(),
            models,
            animations,
            procedural,
            zones,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[SceneInfo]
SceneName="Week 01"
DefaultStartPosition=1152.0,50.0,1152.0 ; middle
DefaultStartDirection=0.0,0.0,1.0
[Environment]
TerrainFile=SX01.trn
[Lights]
NumberOfLights=2
[Light1]
Type=Ambient
ColorRGB=90,90,110
[Light2]
Type=Directional
Position=0,10,0
TargetPosition=1,0,0
[Fog]
ColorRGB=125,125,183
[ResourceFiles]
ResourceFile1=B1B_02.tex
[StaticModels]
NumberOfStaticModels=1
[Model1]
SLT=HayBale.slt
Position=1,2,3
"#;

    #[test]
    fn parses_scene() {
        let scene = Scene::parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(scene.name, "Week 01");
        assert_eq!(scene.terrain_file, "SX01.trn");
        assert_eq!(scene.start_position, [1152.0, 50.0, 1152.0]);
        assert_eq!(scene.lights.len(), 2);
        assert_eq!(scene.lights[0].kind, LightKind::Ambient);
        assert_eq!(scene.lights[1].direction, Some([1.0, -10.0, 0.0]));
        assert_eq!(scene.fog_color, Some([125.0, 125.0, 183.0]));
        assert_eq!(scene.resource_files, vec!["B1B_02.tex"]);
        assert_eq!(scene.models.len(), 1);
        assert_eq!(scene.models[0].model, "HayBale.slt");
        assert_eq!(scene.models[0].up, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn missing_scene_info() {
        assert!(Scene::parse(b"[Environment]\nTerrainFile=a.trn\n").is_err());
    }
}
