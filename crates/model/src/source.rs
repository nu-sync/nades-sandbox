//! The port: facts only. An adapter (the `cs2-throws` file reader, later the demo reader, the in-memory fixture)
//! implements [`ThrowSource`] and nothing else; it never computes what crate::Library derives.

use std::ops::RangeInclusive;
use std::path::PathBuf;

use crate::facts::{DemoId, MapFacts, MapName, PlayerTrack, Sample, ThrowId, Tick, Tracks, Vec3};

/// Changes whenever the source's data does, so the app reloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Revision(pub u64);

pub trait ThrowSource: Send + Sync {
    fn revision(&self) -> Result<Revision, SourceError>;
    /// The maps with demos, by name.
    fn maps(&self) -> Result<Vec<MapName>, SourceError>;
    /// A map's demos, rounds and throws, without flights or effects: what grouping and the filters read.
    fn map(&self, map: &MapName) -> Result<MapFacts, SourceError>;
    /// The map a throw is on, or None when no throw has that id.
    fn map_of(&self, throw: &ThrowId) -> Result<Option<MapName>, SourceError>;
    /// A throw's flight and effect, or None when no throw has that id.
    fn tracks(&self, throw: &ThrowId) -> Result<Option<Tracks>, SourceError>;
    /// One demo's samples between two ticks, in tick order: for viewpoints, and later for the replay.
    fn samples(&self, demo: &DemoId, ticks: RangeInclusive<Tick>) -> Result<Vec<Sample>, SourceError>;
    /// A map's place-labelled sample points of living players: for landing callouts.
    fn places(&self, map: &MapName) -> Result<Vec<(Vec3, String)>, SourceError>;
    /// One round's players at the finer rate replays want (cs2-throws 2.1's `player_tracks`); empty when the source has
    /// none, and a replay then follows the samples.
    fn player_tracks(&self, demo: &DemoId, round: u32) -> Result<Vec<PlayerTrack>, SourceError> {
        let _ = (demo, round);
        Ok(Vec::new())
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SourceError {
    /// A producer is writing; try again.
    #[error("a producer is writing the data: it is read again when it is done")]
    Busy,
    #[error("no data at {}", .0.display())]
    Missing(PathBuf),
    /// Another major version of the format.
    #[error("{found}, and this sandbox reads {supported}")]
    Incompatible { found: String, supported: &'static str },
    #[error("{0}")]
    Corrupt(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
