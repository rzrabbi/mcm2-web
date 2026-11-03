//! An outdoor track's trees and bushes: its `.esb` eco-system placed as
//! `ECO.RES` models, with the trunks solid (Tree Collision).

use glam::Vec3;
use mm2_formats::esb::{EcoSystem, Shape};
use mm2_formats::rs2;
use mm2_formats::scn::{Lighting, Placement};
use mm2_formats::trn::Terrain;
use mm2_physics::collision::Trunk;

/// Each object as a model placement, and the trunks.
pub fn place(eco: &EcoSystem, models: &rs2::Archive, terrain: &Terrain) -> (Vec<Placement>, Vec<Trunk>) {
    // Positions count in the world's larger side over 65536 (0x45a290):
    // the collision grid's extent (0x4e39f5), which is the terrain's span,
    // one sample short of its size.
    let unit = (terrain.size - 1) as f32 * terrain.scale / 65536.0;
    // Each species' model, for its height and spread.
    let sizes: Vec<Option<(f32, f32)>> = eco
        .species
        .iter()
        .map(|s| {
            let data = models.get(&format!("{}.slt", s.name))?;
            let model = mm2_formats::slt::parse(data).ok()?;
            let (mut top, mut spread) = (0.0f32, 0.0f32);
            for p in model.lods.first()?.surfaces.iter().flat_map(|s| &s.positions) {
                top = top.max(p[1]);
                spread = spread.max(p[0].abs()).max(p[2].abs());
            }
            (top > 0.0 && spread > 0.0).then_some((top, spread))
        })
        .collect();
    let mut placements = Vec::with_capacity(eco.objects.len());
    let mut trunks = Vec::new();
    for o in &eco.objects {
        let Some(i) = eco.species.iter().position(|s| s.slot == o.species) else {
            continue;
        };
        let species = &eco.species[i];
        let position = Vec3::from(o.position.map(|p| p as f32 * unit));
        let (height, width) = (species.height_of(o.height), species.width_of(o.width));
        if let Some((top, spread)) = sizes[i] {
            // Stretched to its height, spread to its width.
            let (up, out) = (height / top, width / spread);
            placements.push(Placement {
                model: format!("{}.slt", species.name),
                position: position.into(),
                look: [0.0, 0.0, 1.0],
                up: [0.0, 1.0, 0.0],
                scale: [out, up, out],
                lighting: Lighting::Plant,
            });
        }
        // A trunk is a mesh, only moved to the object (0x457e60): the
        // object's size doesn't scale it.
        for shape in &species.shapes {
            if let Shape::Trunk { radius, height } = *shape {
                trunks.push(Trunk {
                    base: position,
                    radius,
                    height,
                });
            }
        }
    }
    (placements, trunks)
}
