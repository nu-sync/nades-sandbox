//! What the sandbox keeps of CS2's own encodings and rules, with no I/O: [`smoke`], the decoder of a smoke's networked
//! voxel log (the scene's smoke types), and [`technique`], CS2's grenade release model and the names of its techniques.
//!
//! Conventions are the game's: Hammer units with z up, angles in degrees as `m_angEyeAngles` stores them (pitch
//! positive looking down), server ticks at 64 a second.

pub mod smoke;
pub mod technique;

pub use technique::{Click, Movement, Technique};
