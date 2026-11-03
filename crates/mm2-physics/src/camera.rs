//! The race camera's views (KrustyBikeCamera on FollowCamera, FollowCam.cpp;
//! mode switch 0x467d60).
//!
//! Each view is a distance, an elevation and a yaw offset from behind the
//! bike, which the follow camera eases toward; the Rider view sits at the
//! rider's `Head` node instead.

/// The views the camera button (action 10) cycles through, in order. The
/// game also has Bird's-Eye (5, a free debug camera), 6 (no name) and 7
/// (the flyby camera of FlybyCam.vue), which the cycle skips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Chase,
    Slot,
    Sky,
    Rider,
    User,
}

/// Where a view puts the camera (FollowCamera +0x220, +0x22c, +0x234,
/// +0x258).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preset {
    /// From the bike, ft.
    pub distance: f32,
    /// Above the horizontal, rad **[likely]**.
    pub elevation: f32,
    /// Turned from straight behind, rad **[likely]**.
    pub yaw: f32,
    /// Field of view, degrees, when the view sets one.
    pub fov: Option<f32>,
}

impl View {
    pub const ALL: [View; 5] = [View::Chase, View::Slot, View::Sky, View::Rider, View::User];

    /// The name the game flashes for 1.5 s on a switch (LANG.DLL 5049 +
    /// mode, 0x499270).
    pub fn name(self) -> &'static str {
        match self {
            View::Chase => "Chase Cam",
            View::Slot => "Slot Cam",
            View::Sky => "Sky Cam",
            View::Rider => "Rider Cam",
            View::User => "User Cam",
        }
    }

    /// The next view on a camera press.
    pub fn next(self) -> View {
        View::ALL[(self as usize + 1) % View::ALL.len()]
    }

    /// The view's placement (0x467c00, 0x467be0, 0x467bc0, 0x467ba0).
    /// User Cam reads the player's saved settings instead (global +0x2934
    /// distance, +0x2938 elevation, +0x293c yaw, +0x2940 FOV; 0x4991e0),
    /// so it returns `None`.
    pub fn preset(self) -> Option<Preset> {
        match self {
            View::Chase => Some(Preset {
                distance: 11.72,
                elevation: std::f32::consts::FRAC_PI_3,
                yaw: 0.0,
                fov: Some(85.0),
            }),
            View::Slot => Some(Preset {
                distance: 22.68,
                elevation: 0.683_296,
                yaw: -0.609_818,
                fov: None,
            }),
            View::Sky => Some(Preset {
                distance: 75.98,
                elevation: 0.964_818,
                yaw: -0.256_214,
                fov: None,
            }),
            View::Rider => Some(Preset {
                distance: 0.0,
                elevation: 0.0,
                yaw: 0.0,
                fov: None,
            }),
            View::User => None,
        }
    }
}

/// The follow camera's settings from KrustyBikeCamera's constructor
/// (0x4e2d70 → 0x464070): the two easing rates (0.55 on +0x16c, 0.3 on
/// the distance +0x220), 27 (+0x228), 7 (+0x2d0, a height) and a 1.048 rad
/// angle (+0x2e8, 60°) the yaw resets to.
pub const EASE: [f32; 2] = [0.55, 0.3];

/// The Home and End keys narrow and widen the field of view by 3° a press.
pub const FOV_STEP: f32 = 3.0;

/// The Rider view eases from the rider's `Head` node toward the follow
/// point by this much (0x417db0).
pub const RIDER_EASE: f32 = 0.38;

/// The flyby (TV) camera's field of view from its level distance to the
/// bike, degrees: `(200 − d) / 180 × 60 + 10`, kept to 10..70 (0x466b30).
pub fn flyby_fov(distance: f32) -> f32 {
    ((200.0 - distance) / 180.0 * 60.0 + 10.0).clamp(10.0, 70.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_the_views() {
        let mut v = View::Chase;
        let mut names = vec![];
        for _ in 0..5 {
            names.push(v.name());
            v = v.next();
        }
        assert_eq!(v, View::Chase);
        assert_eq!(names[3], "Rider Cam");
        assert_eq!(View::Chase.preset().unwrap().fov, Some(85.0));
        assert_eq!(flyby_fov(20.0), 70.0);
        assert_eq!(flyby_fov(200.0), 10.0);
    }
}
