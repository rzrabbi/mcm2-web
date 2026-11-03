//! Scripted traffic: the cars, trucks and trains that drive set paths
//! through a scene (CarProcedural.cpp: init 0x430520, loader 0x430920,
//! update 0x430ca0, path 0x431ad0).
//!
//! A scene section with `AnimationType=Procedural` names a `.vue` file in
//! PROCVUE.RES. The file is 3ds Max keyframe text; the game keeps only each
//! key's frame number and position and draws a spline through them.

use glam::Vec3;

/// The front wheels turn toward the path by at most this much, rad (30°).
pub const MAX_WHEEL_TURN: f32 = std::f32::consts::FRAC_PI_6;

/// A vehicle's path: keys of (frame, position) in the file's own axes.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub keys: Vec<(f32, Vec3)>,
    /// The scene's `FPS` key (15 in every shipped scene).
    pub fps: f32,
}

impl Path {
    /// Reads a `.vue` file: `FRAME n` lines, each followed by
    /// `transform "<node>" m00 .. m22 x y z` (or `camera "<node>" ..` in
    /// FlybyCam.vue). The last three numbers after the 3×3 matrix are the
    /// position.
    pub fn parse(text: &str, fps: f32) -> Path {
        let mut keys = vec![];
        let mut frame = None;
        for line in text.lines() {
            let mut words = line.split_whitespace();
            match words.next() {
                Some(w) if w.eq_ignore_ascii_case("FRAME") => {
                    frame = words.next().and_then(|n| n.parse::<f32>().ok());
                }
                Some("transform") | Some("camera") => {
                    let nums: Vec<f32> = words.skip(1).filter_map(|n| n.parse().ok()).collect();
                    if let (Some(f), true) = (frame.take(), nums.len() >= 12) {
                        keys.push((f, Vec3::new(nums[9], nums[10], nums[11])));
                    }
                }
                _ => {}
            }
        }
        Path { keys, fps }
    }

    /// One loop of the path, s: the last key's frame over the frame rate.
    pub fn duration(&self) -> f32 {
        self.keys.last().map_or(0.0, |k| k.0) / self.fps
    }

    /// Where the vehicle is `time` seconds in (0x431ad0). The path loops;
    /// between keys it is a cubic Hermite whose tangents are the summed
    /// neighbouring chords scaled for uneven key spacing (a non-uniform
    /// Catmull-Rom), and a plain chord at either end.
    pub fn at(&self, time: f32) -> Vec3 {
        let k = &self.keys;
        match k.len() {
            0 => return Vec3::ZERO,
            1 => return k[0].1,
            _ => {}
        }
        let d = self.duration();
        let t = if d > 0.0 { time.rem_euclid(d) } else { 0.0 } * self.fps;
        let i = k.windows(2).position(|w| t < w[1].0).unwrap_or(k.len() - 2);
        let (f0, p0) = k[i];
        let (f1, p1) = k[i + 1];
        let chord = p1 - p0;
        let m0 = if i > 0 {
            (p0 - k[i - 1].1 + chord) * ((f1 - f0) / (f1 - k[i - 1].0))
        } else {
            chord
        };
        let m1 = if i + 2 < k.len() {
            (k[i + 2].1 - p1 + chord) * ((f1 - f0) / (k[i + 2].0 - f0))
        } else {
            chord
        };
        let s = ((t - f0) / (f1 - f0)).clamp(0.0, 1.0);
        let (s2, s3) = (s * s, s * s * s);
        let h01 = 3.0 * s2 - 2.0 * s3;
        p0 * (1.0 - h01) + p1 * h01 + m0 * (s3 - 2.0 * s2 + s) + m1 * (s3 - s2)
    }
}

impl Path {
    /// How many seconds back along the path the point `distance` feet
    /// behind `time` lies, measured along the curve (0x431800). A negative
    /// distance looks ahead. The search steps back 2 s at first and halves
    /// the step each time it turns around, summing the chords it walks,
    /// until the sum is within 0.1 ft of `distance` or 200 steps pass.
    pub fn lag_time(&self, time: f32, distance: f32) -> f32 {
        let (mut walked, mut back, mut step) = (0.0f32, 0.0f32, 2.0f32);
        let mut going_back = distance >= 0.0;
        let mut prev = self.at(time);
        for _ in 0..200 {
            if (walked - distance).abs() <= 0.1 {
                break;
            }
            let p;
            if walked < distance {
                if !going_back {
                    step *= 0.5;
                    going_back = true;
                }
                back += step;
                p = self.at(time - back);
                walked += chord(p - prev);
            } else {
                if going_back {
                    step *= 0.5;
                    going_back = false;
                }
                back -= step;
                p = self.at(time - back);
                walked -= chord(p - prev);
            }
            prev = p;
        }
        back
    }
}

/// A chord's length; the game skips the root when the square is exactly 1.
fn chord(d: Vec3) -> f32 {
    let sq = d.length_squared();
    if sq == 1.0 {
        1.0
    } else {
        sq.sqrt()
    }
}

/// How far ahead the speed is read, s (+0x1a4), and its scale (+0x1a8).
const LOOK_AHEAD: f32 = 0.1;
const SPEED_SCALE: f32 = 10.0;
/// How much of the old heading (+0x1ac) and ground normal (+0x1b0) each
/// update keeps.
const KEEP_HEADING: f32 = 0.8;
const KEEP_NORMAL: f32 = 0.5;

/// A scripted vehicle (CarProcedural, update 0x430ca0).
#[derive(Debug, Clone)]
pub struct Vehicle {
    /// Its path, in the game's axes.
    pub path: Path,
    /// `LagDistance`, ft: a trailer runs this far behind its cab.
    pub lag: f32,
    /// `Offset`, added to the path's level position.
    pub offset: Vec3,
    /// The path clock, s (+0x194). It starts with the session.
    pub time: f32,
    pub position: Vec3,
    /// Level velocity, ft/s (+0x40).
    pub velocity: Vec3,
    /// Smoothed heading (+0x1f4) and ground normal (+0x200).
    pub heading: Vec3,
    pub up: Vec3,
    fresh: bool,
}

impl Vehicle {
    pub fn new(path: Path, lag: f32, offset: Vec3) -> Self {
        Vehicle {
            path,
            lag,
            offset,
            time: 0.0,
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            heading: Vec3::Z,
            up: Vec3::Y,
            fresh: true,
        }
    }

    /// Advances the clock by `dt` and places the vehicle: on the path
    /// `lag` feet back, facing a point 1 ft further on, standing on the
    /// ground that `ground(x, z)` gives as (height, normal).
    pub fn update(&mut self, dt: f32, ground: impl Fn(f32, f32) -> (f32, Vec3)) {
        let d = self.path.duration();
        self.time = if d > 0.0 { (self.time + dt) % d } else { 0.0 };
        let t = if self.lag > 0.0 {
            self.time - self.path.lag_time(self.time, self.lag)
        } else {
            self.time
        };
        let at = self.path.at(t);
        let ahead = self.path.at(t + LOOK_AHEAD) - at;
        self.velocity = Vec3::new(ahead.x, 0.0, ahead.z) * SPEED_SCALE;
        let look = self.path.at(t - self.path.lag_time(t, -1.0)) - at;
        let look = if look.length_squared() == 0.0 {
            Vec3::Z
        } else {
            look.normalize()
        };
        // The blend is not renormalised.
        self.heading = if self.fresh {
            look
        } else {
            look * (1.0 - KEEP_HEADING) + self.heading * KEEP_HEADING
        };
        let (x, z) = (at.x + self.offset.x, at.z + self.offset.z);
        let (y, n) = ground(x, z);
        self.up = if self.fresh {
            n
        } else {
            n * (1.0 - KEEP_NORMAL) + self.up * KEEP_NORMAL
        };
        self.position = Vec3::new(x, y, z);
        self.fresh = false;
    }

    /// The vehicle's turn: the heading laid on the ground (columns right,
    /// up, forward; 0x4367f0).
    pub fn rotation(&self) -> glam::Mat3 {
        let up = self.up.normalize_or(Vec3::Y);
        let look = (self.heading - up * self.heading.dot(up)).normalize_or(Vec3::Z);
        let right = up.cross(look).normalize_or(Vec3::X);
        glam::Mat3::from_cols(right, look.cross(right), look)
    }
}

/// From the file's axes (Max, z up) to the game's (y up): **[likely]**
/// x stays, the height becomes y and Max's y becomes z.
pub fn to_game(p: Vec3) -> Vec3 {
    Vec3::new(p.x, p.z, p.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_keys() {
        let text = "FRAME 0\ntransform \"Body\" 1 0 0 0 1 0 0 0 1 0 0 0\n\
                    FRAME 15\ntransform \"Body\" 1 0 0 0 1 0 0 0 1 30 0 0\n\
                    FRAME 30\ntransform \"Body\" 1 0 0 0 1 0 0 0 1 60 0 5\n";
        let p = Path::parse(text, 15.0);
        assert_eq!(p.keys.len(), 3);
        assert_eq!(p.duration(), 2.0);
        assert!((p.at(1.0) - Vec3::new(30.0, 0.0, 0.0)).length() < 1e-4);
        assert!((p.at(0.5).x - 15.0).abs() < 1.0);
        // It loops back to the start.
        assert!((p.at(2.0) - Vec3::ZERO).length() < 1e-4);
        assert_eq!(to_game(Vec3::new(1.0, 2.0, 3.0)), Vec3::new(1.0, 3.0, 2.0));
    }

    #[test]
    fn lags_by_arc_length() {
        // A straight line at 15 ft/s.
        let text = "FRAME 0\ntransform \"Body\" 1 0 0 0 1 0 0 0 1 0 0 0\n\
                    FRAME 150\ntransform \"Body\" 1 0 0 0 1 0 0 0 1 150 0 0\n";
        let p = Path::parse(text, 15.0);
        let back = p.lag_time(5.0, 21.0);
        assert!((back - 21.0 / 15.0).abs() < 0.01, "{back}");
        let ahead = p.lag_time(5.0, -1.0);
        assert!((ahead + 1.0 / 15.0).abs() < 0.01, "{ahead}");
        assert_eq!(p.lag_time(5.0, 0.0), 0.0);
    }
}
