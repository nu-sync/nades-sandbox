//! Where the other side stood and looked while a throw's effect was up: the views a result shot can take. The other
//! side's living players in the throw's demo and round, from 1.5 s after the detonation (or the release, for a throw
//! that never went off) to the effect's recorded end, or for 5 s when the facts hold no end (a flash, an HE); nearest
//! the landing first. The result view then picks one with a line of sight through the map's collider.
//!
//! show-me-the-nades' `scene_viewers` macro, as a rule of the sandbox's own.

use std::ops::RangeInclusive;

use crate::derive::eye_at;
use crate::facts::{Player, Round, Sample, Side, Throw, Tick, Vec3};

/// 1.5 s: a smoke has bloomed.
pub const AFTER: i32 = 96;
/// 5 s, when no end is recorded.
pub const WITHOUT_END: i32 = 320;

/// One view a result shot can take.
#[derive(Debug, Clone, PartialEq)]
pub struct Viewpoint {
    pub tick: Tick,
    pub player: Player,
    pub side: Side,
    /// The pawn's feet.
    pub origin: Vec3,
    /// Its eye, lowered by its duck amount.
    pub eye: Vec3,
    pub pitch: Option<f32>,
    pub yaw: Option<f32>,
    /// From the landing.
    pub distance: f32,
}

/// The ticks a throw's views are taken from, within its round when the round's ticks are known; None when that leaves
/// nothing.
pub fn window(throw: &Throw, round: Option<&Round>) -> Option<RangeInclusive<Tick>> {
    let base = throw.detonation.map_or(throw.tick, |(at, _)| at);
    let mut from = base.plus(AFTER);
    let mut to = throw.end.unwrap_or(base.plus(WITHOUT_END));
    if let Some(r) = round {
        if let Some(freeze_end) = r.freeze_end {
            from = from.max(freeze_end);
        }
        if let Some(end) = r.end {
            to = to.min(end);
        }
    }
    (from <= to).then_some(from..=to)
}

/// The views `samples` (the throw's demo's, at least over its [`window`]) offer of a throw that landed at `landing`,
/// nearest first, then earliest.
pub fn viewpoints(throw: &Throw, landing: Vec3, round: Option<&Round>, samples: &[Sample]) -> Vec<Viewpoint> {
    let Some(window) = window(throw, round) else { return Vec::new() };
    let mut out: Vec<Viewpoint> = samples
        .iter()
        .filter(|s| s.alive && s.side != throw.side && window.contains(&s.tick))
        .map(|s| {
            // per axis in f32, the sum and its root in doubles, as the database computed it
            let d = [s.origin[0] - landing[0], s.origin[1] - landing[1], s.origin[2] - landing[2]].map(f64::from);
            let eye = eye_at(s.origin.map(f64::from), s.duck.map(f64::from));
            Viewpoint {
                tick: s.tick,
                player: s.player.clone(),
                side: s.side,
                origin: s.origin,
                eye: eye.map(|v| v as f32),
                pitch: s.pitch,
                yaw: s.yaw,
                distance: (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32,
            }
        })
        .collect();
    out.sort_by(|a, b| a.distance.total_cmp(&b.distance).then(a.tick.cmp(&b.tick)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::throw;
    use crate::{DemoId, Grenade};

    fn sample(tick: i32, side: Side, x: f32, alive: bool) -> Sample {
        Sample { tick: Tick(tick), player: Player { name: Some(format!("p{tick}")), steamid: None }, side, origin: [x, 900.0, 0.0], pitch: Some(2.0), yaw: Some(180.0), duck: None, alive, place: None }
    }

    #[test]
    fn the_other_sides_living_players_from_a_second_and_a_half_after_it_went_off_nearest_first() {
        let mut t = throw("t1", "d1", Grenade::Smoke, Side::T, [12.0, 22.0, 94.0]);
        t.tick = Tick(1500);
        t.detonation = Some((Tick(1600), [800.0, 900.0, 40.0]));
        t.end = Some(Tick(1600 + 18 * 64));
        assert_eq!(window(&t, None), Some(Tick(1696)..=Tick(2752)));
        let samples = [
            sample(1680, Side::Ct, 900.0, true),  // before it bloomed
            sample(1760, Side::Ct, 1300.0, true),
            sample(1792, Side::Ct, 1200.0, true),
            sample(1792, Side::T, 800.0, true),   // the thrower's side
            sample(1824, Side::Ct, 1000.0, false), // dead
            sample(2784, Side::Ct, 800.0, true),  // after it was gone
        ];
        let views = viewpoints(&t, [800.0, 900.0, 40.0], None, &samples);
        assert_eq!(views.iter().map(|v| (v.tick.0, v.origin[0])).collect::<Vec<_>>(), [(1792, 1200.0), (1760, 1300.0)]);
        assert_eq!(views[0].eye, [1200.0, 900.0, 64.0]);
        assert!((views[0].distance - (400.0f32 * 400.0 + 1600.0).sqrt()).abs() < 1e-3);
    }

    #[test]
    fn without_an_end_five_seconds_and_never_past_the_round() {
        let mut t = throw("t", "d", Grenade::He, Side::Ct, [0.0; 3]);
        t.tick = Tick(1000);
        assert_eq!(window(&t, None), Some(Tick(1096)..=Tick(1320)), "never went off: from the release");
        t.detonation = Some((Tick(1100), [0.0; 3]));
        assert_eq!(window(&t, None), Some(Tick(1196)..=Tick(1420)));
        let round = Round { demo: DemoId("d".into()), number: 1, freeze_end: Some(Tick(0)), plant: None, end: Some(Tick(1300)), winner: None };
        assert_eq!(window(&t, Some(&round)), Some(Tick(1196)..=Tick(1300)));
        let over = Round { end: Some(Tick(1150)), ..round };
        assert_eq!(window(&t, Some(&over)), None);
    }
}
