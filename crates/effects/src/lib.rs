//! What a simulated grenade leaves behind, made from the map's collider so it respects walls: the voxel grid a smoke
//! fills (handed to `nades_scene::smoke::fill_cloud`, the port of the web view's smoke) and the flames a fire spreads
//! to (handed to `nades_scene::fire::build_fire`). docs/simulator.md is the contract.
//!
//! SKELETON: the signatures are the contract; the bodies are placeholders (no solid voxels, a ring of flames) for the
//! effects unit to replace and verify against recorded smokes and fires.

use glam::DVec3;
use nades_model::smoke::Emitter;
use nades_scene::collider::Collider;
use nades_scene::fire::Flame;
use nades_scene::smoke::{CELLS, GRID, Grid};

/// The smoke's grid around `detonation`, as a server-sent voxel log would describe it: which voxels are solid map
/// geometry, and the emitter the cloud grows from. PLACEHOLDER: nothing solid.
pub fn smoke_grid(_detonation: DVec3, _collider: &Collider) -> Grid {
    let mid = (GRID / 2) as u8;
    Grid {
        solid: vec![0; CELLS],
        sent: vec![1; CELLS],
        sent_frame: vec![0; CELLS],
        emitters: vec![Emitter { cell: [mid, mid, mid], level: 1, hold: 0.0 }],
        frames: 160,
        fading_from: 160,
    }
}

/// The flames a molotov or incendiary spreads to from `detonation`, in seconds after it. PLACEHOLDER: a ring.
pub fn fire_flames(detonation: DVec3, normal: Option<DVec3>, _collider: &Collider) -> Vec<Flame> {
    let n = normal.unwrap_or(DVec3::Z);
    (0..12)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / 12.0;
            let pos = detonation + DVec3::new(a.cos(), a.sin(), 0.0) * 60.0;
            Flame { pos: pos.to_array(), normal: n.to_array(), start: 0.1 * i as f64, end: 7.0 }
        })
        .collect()
}
