//! Motocross Madness 2's bike physics, rebuilt from the game.
//!
//! Units are feet, seconds and pounds; coordinates are the game's
//! (x right, y up, z forward, left-handed).

pub mod ai;
pub mod bike;
pub mod bump;
pub mod camera;
pub mod career;
pub mod collision;
pub mod computer;
pub mod drivetrain;
pub mod procedural;
pub mod race;
pub mod spec;
pub mod stunt;
mod thrust;
pub mod wreck;

pub use bike::{Bike, Controls, Crash};
pub use spec::Spec;

use glam::Vec3;

/// What the bike stands on: the track's terrain and its solid objects.
pub trait Ground {
    /// The terrain's height under `x`, `z` and its surface normal.
    fn terrain(&self, x: f32, z: f32) -> (f32, Vec3);

    /// The world's width and depth, ft, when riding off its edge throws
    /// the rider back (not in Supercross, 0x48edd0).
    fn edge(&self) -> Option<(f32, f32)> {
        None
    }

    /// The deepest touch of a sphere with the track's solid objects.
    fn solid(&self, _centre: Vec3, _radius: f32) -> Option<collision::Hit> {
        None
    }

    /// The first solid face a point moving from `from` to `to` passes
    /// through from its front (the bike's collision points, 0x428ae0).
    fn crossing(&self, _from: Vec3, _to: Vec3) -> Option<collision::Crossing> {
        None
    }

    /// The tyres' braking, side and cornering grip on the surface at a
    /// point (`[TerrainZoneN] SurfaceFriction`, tyre+0x144).
    fn friction(&self, _at: Vec3) -> f32 {
        1.0
    }

    /// The rear's grip under power there (`SurfaceTraction`, tyre+0x284,
    /// 0x519670). Only the Ski Lodge's snow sets either below 1.
    fn traction(&self, _at: Vec3) -> f32 {
        1.0
    }
}

/// Flat ground at y = 0, for tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flat;

impl Ground for Flat {
    fn terrain(&self, _x: f32, _z: f32) -> (f32, Vec3) {
        (0.0, Vec3::Y)
    }
}
