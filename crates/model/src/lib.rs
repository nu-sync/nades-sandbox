//! The sandbox's vocabulary and rules, with no I/O (docs/standalone-api.md §4–§5).
//!
//! Three layers:
//!
//! 1. **Facts** ([`facts`]): what a CS2 demo records, named after the game's own properties and events. Any parser can
//!    produce them; a data source hands them over through the [`ThrowSource`] port and computes nothing.
//! 2. **Derivations**: everything computed from facts, one implementation for every source. [`technique`] (CS2's
//!    release model), [`derive`] (times, landing, the eye, the practice command, round time), [`lineups`] (grouping),
//!    [`places`] (landing callouts), [`viewpoints`] (the other side's views), [`smoke`] (the voxel log's decoder), and
//!    [`Library`], which applies them to one map's facts and answers the app's questions.
//! 3. **Tags**: a producer's own labels (`namespace.key = value`) on a demo or a throw, shown and filtered on, never
//!    interpreted.
//!
//! Conventions are the game's: Hammer units with z up, angles in degrees as `m_angEyeAngles` stores them (pitch
//! positive looking down), server ticks at 64 a second.

pub mod derive;
pub mod facts;
pub mod features;
pub mod fixture;
pub mod library;
pub mod lineups;
pub mod places;
pub mod replay;
pub mod smoke;
pub mod source;
pub mod technique;
pub mod viewpoints;

pub use facts::*;
pub use library::{Derived, Facets, Library, Lineup, LineupQuery, Practice, Viewpoint};
pub use source::{Revision, SourceError, ThrowSource};
pub use technique::{Click, Movement, Technique};
