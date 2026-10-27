//! Facts in memory: a [`ThrowSource`] for tests, and the smallest valid demo and throw to build them from.

use std::collections::HashMap;
use std::ops::RangeInclusive;

use crate::facts::{Demo, DemoId, Grenade, MapFacts, MapName, Player, Round, Sample, Side, Throw, ThrowId, Tick, Tracks, Vec3};
use crate::source::{Revision, SourceError, ThrowSource};

/// A demo with only its required facts: an id and the map.
pub fn demo(id: &str, map: &str) -> Demo {
    Demo { id: DemoId(id.into()), map: MapName(map.into()), played_at: None, file_name: None, tags: Vec::new() }
}

/// A throw with only its required facts: tick 0, its grenade, side and start (it still needs a detonation or a flight
/// to land it).
pub fn throw(id: &str, demo: &str, grenade: Grenade, side: Side, start: Vec3) -> Throw {
    Throw {
        id: ThrowId(id.into()),
        demo: DemoId(demo.into()),
        round: None,
        tick: Tick(0),
        grenade,
        side,
        player: Player::default(),
        team: None,
        place: None,
        start,
        velocity: None,
        thrower: None,
        detonation: None,
        end: None,
        flight_end: None,
        tags: Vec::new(),
    }
}

/// Facts held in memory.
#[derive(Debug, Clone, Default)]
pub struct Memory {
    pub demos: Vec<Demo>,
    pub rounds: Vec<Round>,
    pub throws: Vec<Throw>,
    pub tracks: HashMap<ThrowId, Tracks>,
    pub samples: Vec<(DemoId, Sample)>,
    /// Bumped by the test when it changes the facts.
    pub revision: u64,
}

impl ThrowSource for Memory {
    fn revision(&self) -> Result<Revision, SourceError> {
        Ok(Revision(self.revision))
    }

    fn maps(&self) -> Result<Vec<MapName>, SourceError> {
        let mut maps: Vec<MapName> = self.demos.iter().map(|d| d.map.clone()).collect();
        maps.sort();
        maps.dedup();
        Ok(maps)
    }

    fn map(&self, map: &MapName) -> Result<MapFacts, SourceError> {
        let demos: Vec<Demo> = self.demos.iter().filter(|d| &d.map == map).cloned().collect();
        let on = |id: &DemoId| demos.iter().any(|d| &d.id == id);
        Ok(MapFacts {
            rounds: self.rounds.iter().filter(|r| on(&r.demo)).cloned().collect(),
            throws: self.throws.iter().filter(|t| on(&t.demo)).cloned().collect(),
            demos,
        })
    }

    fn map_of(&self, throw: &ThrowId) -> Result<Option<MapName>, SourceError> {
        let Some(t) = self.throws.iter().find(|t| &t.id == throw) else { return Ok(None) };
        Ok(self.demos.iter().find(|d| d.id == t.demo).map(|d| d.map.clone()))
    }

    fn tracks(&self, throw: &ThrowId) -> Result<Option<Tracks>, SourceError> {
        Ok(self.tracks.get(throw).cloned())
    }

    fn samples(&self, demo: &DemoId, ticks: RangeInclusive<Tick>) -> Result<Vec<Sample>, SourceError> {
        let mut out: Vec<Sample> = self.samples.iter().filter(|(d, s)| d == demo && ticks.contains(&s.tick)).map(|(_, s)| s.clone()).collect();
        out.sort_by_key(|s| s.tick);
        Ok(out)
    }

    fn places(&self, map: &MapName) -> Result<Vec<(Vec3, String)>, SourceError> {
        let on: Vec<&DemoId> = self.demos.iter().filter(|d| &d.map == map).map(|d| &d.id).collect();
        Ok(self
            .samples
            .iter()
            .filter(|(d, s)| on.contains(&d) && s.alive)
            .filter_map(|(_, s)| Some((s.origin, s.place.clone().filter(|p| !p.is_empty())?)))
            .collect())
    }
}
