//! A round replayed (SPEC.md M5, docs/decisions.md S30): where every player stood and looked at any moment, from the
//! round's tracks (cs2-throws 2.1's `player_tracks`, 16 a second), else from its samples (two a second), and which of
//! its throws are in the air or still burning or smoking.
//!
//! Between two points a player moves in a straight line and turns the short way round. Points further apart than
//! [`MAX_GAP`] are not joined: the player holds the earlier one. A player who died stays where they last stood, dead.

use std::collections::BTreeMap;

use crate::facts::{Player, PlayerTrack, Round, Sample, Side, Throw, Tick, TrackPoint, Vec3};

/// Ticks a second.
pub const TICKS: f64 = 64.0;
/// Points further apart than this many ticks are not interpolated between.
pub const MAX_GAP: i32 = 96;

/// One player at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub player: Player,
    pub side: Side,
    /// The pawn's origin (its feet).
    pub origin: Vec3,
    pub pitch: f32,
    pub yaw: f32,
    pub alive: bool,
}

/// `a` to `b` by `f`, the short way round, in (−180, 180].
pub fn lerp_yaw(a: f32, b: f32, f: f32) -> f32 {
    let d = (b - a + 540.0).rem_euclid(360.0) - 180.0;
    let y = (a + d * f + 180.0).rem_euclid(360.0) - 180.0;
    if y == -180.0 { 180.0 } else { y }
}

/// Where `track`'s player is at `tick` (fractional): None before they have a point.
pub fn player_at(track: &PlayerTrack, tick: f64) -> Option<PlayerState> {
    let first = track.points.first()?;
    let state = |p: &TrackPoint, alive| PlayerState { player: track.player.clone(), side: track.side, origin: p.origin, pitch: p.pitch, yaw: p.yaw, alive };
    if f64::from(first.tick.0) > tick {
        return None;
    }
    let last = track.points.last().expect("a first point");
    if track.died.is_some_and(|d| f64::from(d.0) <= tick) {
        return Some(state(last, false));
    }
    // the last point at or before `tick`
    let i = track.points.partition_point(|p| f64::from(p.tick.0) <= tick) - 1;
    let a = &track.points[i];
    let Some(b) = track.points.get(i + 1).filter(|b| b.tick.0 - a.tick.0 <= MAX_GAP) else { return Some(state(a, true)) };
    let f = ((tick - f64::from(a.tick.0)) / f64::from(b.tick.0 - a.tick.0)) as f32;
    let origin = std::array::from_fn(|k| a.origin[k] + (b.origin[k] - a.origin[k]) * f);
    Some(PlayerState { player: track.player.clone(), side: track.side, origin, pitch: a.pitch + (b.pitch - a.pitch) * f, yaw: lerp_yaw(a.yaw, b.yaw, f), alive: true })
}

/// Every player of a round at `tick`.
pub fn players_at(tracks: &[PlayerTrack], tick: f64) -> Vec<PlayerState> {
    tracks.iter().filter_map(|t| player_at(t, tick)).collect()
}

/// A round's tracks from its samples, for a source without `player_tracks`: each player's living samples between the
/// round's freeze end and its end, and the first tick sampled dead.
pub fn tracks_from_samples(samples: &[Sample], round: &Round) -> Vec<PlayerTrack> {
    let (Some(start), Some(end)) = (round.freeze_end, round.end) else { return Vec::new() };
    let mut by: BTreeMap<(Option<u64>, Option<String>), PlayerTrack> = BTreeMap::new();
    for s in samples.iter().filter(|s| s.tick >= start && s.tick <= end) {
        let t = by.entry((s.player.steamid, s.player.name.clone())).or_insert_with(|| PlayerTrack { player: s.player.clone(), side: s.side, died: None, points: Vec::new() });
        if t.died.is_some() {
            continue;
        }
        if !s.alive {
            t.died = Some(s.tick);
            continue;
        }
        t.points.push(TrackPoint { tick: s.tick, origin: s.origin, pitch: s.pitch.unwrap_or(0.0), yaw: s.yaw.unwrap_or(0.0) });
    }
    by.into_values().filter(|t| !t.points.is_empty()).collect()
}

/// The ticks a round's replay runs over: its freeze end to its end, and on while a throw of it still shows.
pub fn span(round: &Round, last_effect: Option<Tick>) -> Option<(Tick, Tick)> {
    let start = round.freeze_end?;
    let end = round.end?.max(last_effect.unwrap_or(Tick(0)));
    (end > start).then_some((start, end))
}

/// A round's throws in tick order.
pub fn round_throws<'a>(throws: impl IntoIterator<Item = &'a Throw>, round: &Round) -> Vec<&'a Throw> {
    let mut out: Vec<&Throw> = throws.into_iter().filter(|t| t.demo == round.demo && t.round == Some(round.number)).collect();
    out.sort_by(|a, b| (a.tick, &a.id).cmp(&(b.tick, &b.id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::DemoId;

    fn point(tick: i32, x: f32, yaw: f32) -> TrackPoint {
        TrackPoint { tick: Tick(tick), origin: [x, 0.0, 0.0], pitch: 0.0, yaw }
    }

    fn track(points: Vec<TrackPoint>, died: Option<i32>) -> PlayerTrack {
        PlayerTrack { player: Player { name: Some("a".into()), steamid: Some(1) }, side: Side::Ct, died: died.map(Tick), points }
    }

    #[test]
    fn yaw_turns_the_short_way_round() {
        assert_eq!(lerp_yaw(170.0, -170.0, 0.5), 180.0);
        assert_eq!(lerp_yaw(-170.0, 170.0, 0.25), -175.0);
        assert_eq!(lerp_yaw(10.0, 50.0, 0.5), 30.0);
        assert_eq!(lerp_yaw(350.0, 10.0, 0.5), 0.0);
    }

    #[test]
    fn a_player_moves_between_points_holds_across_gaps_and_stays_where_they_died() {
        let t = track(vec![point(100, 0.0, 170.0), point(104, 40.0, -170.0), point(300, 100.0, 0.0), point(304, 140.0, 0.0)], Some(310));
        assert_eq!(player_at(&t, 99.0), None, "before their first point");
        let s = player_at(&t, 102.0).unwrap();
        assert_eq!((s.origin[0], s.yaw, s.alive), (20.0, 180.0, true));
        assert_eq!(player_at(&t, 200.0).unwrap().origin[0], 40.0, "a gap of 196 ticks is not joined");
        assert_eq!(player_at(&t, 303.0).unwrap().origin[0], 130.0);
        let dead = player_at(&t, 400.0).unwrap();
        assert_eq!((dead.origin[0], dead.alive), (140.0, false));
        assert_eq!(player_at(&t, 305.0).unwrap().origin[0], 140.0, "after the last point, alive, it holds");
    }

    #[test]
    fn samples_make_tracks_for_a_source_without_them() {
        let round = Round { demo: DemoId("d".into()), number: 3, freeze_end: Some(Tick(1000)), plant: None, end: Some(Tick(2000)), winner: None };
        let sample = |tick: i32, steamid: u64, alive: bool| Sample { tick: Tick(tick), player: Player { name: None, steamid: Some(steamid) }, side: Side::T, origin: [tick as f32, 0.0, 0.0], pitch: Some(1.0), yaw: Some(2.0), duck: None, alive, place: None };
        let samples = [sample(968, 1, true), sample(1000, 1, true), sample(1032, 1, true), sample(1064, 1, false), sample(1096, 1, false), sample(1000, 2, true), sample(2032, 2, true)];
        let tracks = tracks_from_samples(&samples, &round);
        assert_eq!(tracks.len(), 2);
        assert_eq!((tracks[0].points.iter().map(|p| p.tick.0).collect::<Vec<_>>(), tracks[0].died), (vec![1000, 1032], Some(Tick(1064))));
        assert_eq!((tracks[1].points.len(), tracks[1].died), (1, None));
        assert_eq!(span(&round, Some(Tick(2300))), Some((Tick(1000), Tick(2300))));
        assert_eq!(span(&round, None), Some((Tick(1000), Tick(2000))));
    }
}
