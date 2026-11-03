//! The bikes and rider outfits on offer, from `UI/UIRES.RES` `bikes.pb`
//! and `riders.pb` as the game reads them (0x49b7d0), and the player's
//! choice of them.

use mm2_formats::{rs2, scn};
use std::path::Path;

/// One bike of the Bike & Rider page.
#[derive(Debug, Clone, PartialEq)]
pub struct BikeChoice {
    /// "Honda CR250": the maker and the bike (0x450630 shows `%s %s`).
    pub name: String,
    /// The race model (`honda.mcf`) and the one the menus turn
    /// (`uihonda.mcf`), in `RES/BIKE.RES`.
    pub model: String,
    pub ui_model: String,
    /// The paint, replacing the model's first texture.
    pub texture: String,
    pub cc: u32,
    pub four_stroke: bool,
}

/// One rider outfit.
#[derive(Debug, Clone, PartialEq)]
pub struct Outfit {
    pub name: String,
    pub file: String,
}

/// What the player rides.
#[derive(Debug, Clone, PartialEq)]
pub struct Ride {
    pub bike: BikeChoice,
    pub outfit: String,
    pub plate: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Roster {
    pub bikes: Vec<BikeChoice>,
    pub outfits: Vec<Outfit>,
}

/// Shown in the lists: `Availability` 2 and 3. 1 is kept from the player
/// (the amateurs' kit), and 10 is the award bike, shown once the career is
/// won (0x450630).
fn offered(availability: i32) -> bool {
    matches!(availability, 2 | 3)
}

impl Roster {
    /// Reads the lists from the game's `UI/UIRES.RES`.
    pub fn load(game: &Path) -> Roster {
        let Ok(bytes) = crate::read_archive(&game.join("UI").join("UIRES.RES")) else {
            return Roster::default();
        };
        let Ok(ar) = rs2::Archive::parse(&bytes) else {
            return Roster::default();
        };
        let text = |name: &str| {
            ar.get(name)
                .map(|b| String::from_utf8_lossy(b).into_owned())
        };
        Roster {
            bikes: text("bikes.pb").map(|t| bikes(&t)).unwrap_or_default(),
            outfits: text("riders.pb").map(|t| outfits(&t)).unwrap_or_default(),
        }
    }

    /// The bike and outfit the game starts a new profile on: the first of
    /// each, with our pick of plate (the game's run from 101 to 999).
    pub fn default_ride(&self) -> Option<Ride> {
        Some(Ride {
            bike: self.bikes.first()?.clone(),
            outfit: self.outfits.first()?.file.clone(),
            plate: 107,
        })
    }
}

fn int(s: &scn::Section, key: &str) -> Option<i32> {
    s.get(key)?.trim().parse().ok()
}

fn bikes(text: &str) -> Vec<BikeChoice> {
    let ini = scn::Ini::parse(text);
    let makers = ini
        .section("Information")
        .and_then(|s| int(s, "NumberOfManufacturers"))
        .unwrap_or(0);
    let mut out = Vec::new();
    for m in 1..=makers {
        let Some(s) = ini.section(&format!("Manufacturer_{m:02}")) else {
            continue;
        };
        if !offered(int(s, "Availability").unwrap_or(1)) {
            continue;
        }
        let maker = s.get("Name").unwrap_or("");
        for b in 1..=int(s, "NumberOfBikes").unwrap_or(0) {
            let key = |k: &str| s.get(&format!("{k}_{b:02}")).map(str::to_string);
            out.push(BikeChoice {
                name: format!("{maker} {}", key("BikeName").unwrap_or_default()),
                model: s.get("ModelFile").unwrap_or("").to_string(),
                ui_model: s.get("UIModelFile").unwrap_or("").to_string(),
                texture: key("BikeTexture").unwrap_or_default(),
                cc: key("BikeEngineSize")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(250),
                four_stroke: key("BikeFourStroke").is_some_and(|v| v.trim() == "1"),
            });
        }
    }
    out
}

fn outfits(text: &str) -> Vec<Outfit> {
    let ini = scn::Ini::parse(text);
    let count = ini
        .section("Information")
        .and_then(|s| int(s, "NumberOfRiders"))
        .unwrap_or(0);
    (1..=count)
        .filter_map(|r| ini.section(&format!("Rider_{r:02}")))
        .filter(|s| offered(int(s, "Availability").unwrap_or(1)))
        .map(|s| Outfit {
            name: s.get("Name").unwrap_or("").to_string(),
            file: s.get("File").unwrap_or("").to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_lists() {
        let bikes = bikes(
            "[Information]\nNumberOfManufacturers = 2\n[Manufacturer_01]\nName = \"Honda\"\n\
             NumberOfBikes = 1\nUIModelFile = uihonda.mcf\nModelFile = honda.mcf\nAvailability = 3\n\
             BikeName_01 = \"CR250\"\nBikeTexture_01 = HondaCR250_08.tga\nBikeEngineSize_01 = 250\n\
             BikeFourStroke_01 = 0\n[Manufacturer_02]\nName = \"Generic\"\nNumberOfBikes = 1\n\
             Availability = 1\nBikeName_01 = \"125\"\n",
        );
        assert_eq!(bikes.len(), 1);
        assert_eq!(bikes[0].name, "Honda CR250");
        assert_eq!((bikes[0].cc, bikes[0].four_stroke), (250, false));
        assert_eq!(bikes[0].texture, "HondaCR250_08.tga");
        let outfits = outfits(
            "[Information]\nNumberOfRiders=2\n[Rider_01]\nFile=A.tga\nName=\"A\"\nAvailability = 3\n\
             [Rider_02]\nFile=B.tga\nName=\"B\"\nAvailability = 1\n",
        );
        assert_eq!(
            outfits,
            [Outfit {
                name: "A".into(),
                file: "A.tga".into()
            }]
        );
    }

    /// With the game's files beside the engine, the lists fill.
    #[test]
    fn game_lists() {
        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../GAME");
        if !crate::platform::fs::is_file(game.join("UI/UIRES.RES")) {
            return;
        }
        let r = Roster::load(&game);
        assert!(r.bikes.len() > 10, "{}", r.bikes.len());
        assert!(r.outfits.len() > 40, "{}", r.outfits.len());
        assert_eq!(r.bikes[0].name, "Honda CR125");
    }
}
