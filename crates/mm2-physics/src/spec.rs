//! A bike's weight and power curve, from the game's `presets.pb`
//! (UI/UIRES.RES), as the profile code reads it (0x505760 for the weight,
//! 0x4d7090 for the curve).
//!
//! The file has one `[Category_N]` per engine class (125, 250, 400
//! thumper, 500, 600 thumper) with `Weight`, `RpmLowerLimit`,
//! `RpmUpperLimit` and eleven `rpmNNNNN` horsepower points from the lower
//! limit to the upper, and three `[Category_NPreset_M]` curves for the
//! shipped bikes of that class. The player's curve is the preset's, or the
//! category's own when the rider tuned a custom one.

/// Engine sizes of the five categories (0x572bec).
pub const ENGINE_SIZES: [u32; 5] = [125, 250, 400, 500, 600];

#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    /// Bike and rider, lb.
    pub weight: f32,
    pub rpm_low: f32,
    pub rpm_high: f32,
    /// Horsepower at eleven even steps from `rpm_low` to `rpm_high`.
    pub power: [f32; 11],
}

impl Default for Spec {
    /// The 250 class with the first preset (the CR250), what a new
    /// profile rides.
    fn default() -> Self {
        Spec {
            weight: 230.0,
            rpm_low: 5000.0,
            rpm_high: 10000.0,
            power: [
                24.0, 28.0, 33.0, 38.0, 39.0, 41.0, 43.0, 45.0, 41.0, 35.0, 22.0,
            ],
        }
    }
}

/// The category (0 to 4) of an engine of `cc`, as the game picks it.
pub fn category(cc: u32) -> usize {
    let s = ENGINE_SIZES;
    if cc <= s[0] {
        return 0;
    }
    if cc >= s[4] {
        return 4;
    }
    (1..4)
        .find(|&i| s[i - 1] < cc && cc < s[i + 1])
        .unwrap_or(1)
}

impl Spec {
    /// The spec of `category` (0 to 4) from the text of `presets.pb`, with
    /// the curve of `preset` (0 to 2), or the category's own with `None`.
    pub fn from_presets(text: &str, category: usize, preset: Option<usize>) -> Option<Spec> {
        let base = format!("Category_{}", category + 1);
        let section = |name: &str| -> Vec<(String, f32)> {
            let mut inside = false;
            let mut out = Vec::new();
            for line in text.lines() {
                let line = line.split(';').next().unwrap_or("").trim();
                if let Some(head) = line.strip_prefix('[') {
                    inside = head.trim_end_matches(']').eq_ignore_ascii_case(name);
                } else if inside {
                    if let Some((k, v)) = line.split_once('=') {
                        if let Ok(v) = v.trim().parse() {
                            out.push((k.trim().to_ascii_lowercase(), v));
                        }
                    }
                }
            }
            out
        };
        let main = section(&base);
        let get = |k: &str| main.iter().find(|(n, _)| n == k).map(|&(_, v)| v);
        let mut spec = Spec {
            weight: get("weight")?,
            rpm_low: get("rpmlowerlimit")?,
            rpm_high: get("rpmupperlimit")?,
            power: [0.0; 11],
        };
        let curve = match preset {
            Some(p) => section(&format!("{base}Preset_{}", p + 1)),
            None => main.clone(),
        };
        let points: Vec<f32> = curve
            .iter()
            .filter(|(n, _)| n.starts_with("rpm") && n[3..].bytes().all(|b| b.is_ascii_digit()))
            .map(|&(_, v)| v)
            .collect();
        if points.len() != 11 {
            return None;
        }
        spec.power.copy_from_slice(&points);
        Some(spec)
    }

    /// The physics mass, slugs (0x505760: weight × 0.0310559).
    pub fn mass(&self) -> f32 {
        self.weight * 0.031_055_9
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_from_engine_size() {
        assert_eq!(category(80), 0);
        assert_eq!(category(125), 0);
        assert_eq!(category(250), 1);
        assert_eq!(category(400), 2);
        assert_eq!(category(450), 2);
        assert_eq!(category(500), 3);
        assert_eq!(category(650), 4);
    }

    #[test]
    fn reads_presets() {
        let text =
            "[Category_2]\r\nWeight=230\r\nRpmLowerLimit = 5000\r\nRpmUpperLimit = 10000\r\n\
            rpm05000 = 25\r\nrpm05500 = 30\r\nrpm06000 = 35\r\nrpm06500 = 40\r\nrpm07000 = 42\r\n\
            rpm07500 = 45\r\nrpm08000 = 45\r\nrpm08500 = 45\r\nrpm09000 = 42\r\nrpm09500 = 35\r\n\
            rpm10000 = 22\r\nMinRange = 20\r\n[Category_2Preset_1]\r\n;cr250\r\nrpm05000=24\r\n\
            rpm05500=28\r\nrpm06000=33\r\nrpm06500=38\r\nrpm07000=39\r\nrpm07500=41\r\n\
            rpm08000=43\r\nrpm08500=45\r\nrpm09000=41\r\nrpm09500=35\r\nrpm10000=22\r\n";
        assert_eq!(Spec::from_presets(text, 1, Some(0)), Some(Spec::default()));
        let own = Spec::from_presets(text, 1, None).unwrap();
        assert_eq!(own.power[0], 25.0);
        assert!(Spec::from_presets(text, 0, None).is_none());
    }
}
