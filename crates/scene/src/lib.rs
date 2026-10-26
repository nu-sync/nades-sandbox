//! A throw becomes a stage: what the sandbox draws of it and where it can look from. Ported from show-me-the-nades'
//! web view (`web/app/src/scene/`), number for number, so both open a throw from the same camera and draw the same
//! cloud and fire:
//!
//! - [`camera`]: CS2's view (73.74° vertical, the aim zoom), angles, and the choice of a result shot's camera;
//! - [`collider`]: awpy's collision mesh and a BVH over it, the line of sight both views judge by;
//! - [`smoke`]: the cloud rebuilt from the smoke's voxel log, its 32³ volume, and the light the map lets reach it;
//! - [`noise`]: the noise CS2's smoke shader breaks the cloud up with;
//! - [`fire`]: a molotov's or incendiary's fire as CS2's particle systems draw it;
//! - [`stage`]: one throw's cameras (Lineup, Aim, Result, Orbit), its effects and its timeline.
//!
//! World units are Hammer units with z up, as the database stores them; the renderer turns them into its own frame.

pub mod camera;
pub mod collider;
pub mod fire;
pub mod noise;
pub mod smoke;
pub mod stage;
