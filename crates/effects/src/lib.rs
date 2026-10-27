//! What a simulated grenade leaves behind, made from the map's collider so it respects walls: the voxel grid a smoke
//! fills (handed to `nades_scene::smoke::fill_cloud`, the port of the web view's smoke) and the flames a fire spreads
//! to (handed to `nades_scene::fire::build_fire`). docs/simulator.md is the contract.
//!
//! Both are measured against recorded smokes and fires: `examples/effects_check.rs` reports how alike they are per
//! map, and docs/decisions.md S33 says what was learned and what still differs.

use glam::DVec3;
use nades_scene::collider::Collider;
use nades_scene::fire::{FireKind, Flame};
use nades_scene::smoke::{Grid, HALF_SPAN};

pub mod fire;
pub mod measure;
pub mod smoke;
pub mod voxels;

/// The smoke's grid around `detonation`, as a server-sent voxel log would describe it: which voxels are solid map
/// geometry, which bricks are sent when, and the emitters the cloud grows from (S33).
pub fn smoke_grid(detonation: DVec3, collider: &Collider) -> Grid {
    smoke::grid(&voxels::surface(detonation - DVec3::splat(HALF_SPAN), collider, voxels::SAMPLES, 0.0), &smoke::Model::default())
}

/// The flames a molotov spreads to from `detonation`, in seconds after it (S33). The fire starts on the floor under the
/// detonation whatever surface the grenade broke on, so `normal` is not needed; [`fire_flames_of`] spreads an
/// incendiary's smaller, shorter fire.
pub fn fire_flames(detonation: DVec3, _normal: Option<DVec3>, collider: &Collider) -> Vec<Flame> {
    fire::flames(FireKind::Molotov, detonation, collider)
}

/// The flames a fire of `kind` spreads to from `detonation`, in seconds after it.
pub fn fire_flames_of(kind: FireKind, detonation: DVec3, collider: &Collider) -> Vec<Flame> {
    fire::flames(kind, detonation, collider)
}
