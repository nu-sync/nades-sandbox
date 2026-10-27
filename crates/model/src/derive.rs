//! One throw's derivations from its own facts (docs/standalone-api.md §4): times, landing, the eye, the practice
//! command, round time and after-plant.

use std::ops::RangeInclusive;

use crate::facts::{Blind, Grenade, Pawn, Player, Round, Sample, Throw, Tick, Vec3};
use crate::technique::{Movement, Technique};

/// CS2's eye above the feet, standing and crouched.
pub const EYE_STANDING: f64 = 64.0;
pub const EYE_CROUCHED: f64 = 46.0;

/// Where the eye is for someone standing at `feet`; a partial duck lowers it in proportion.
pub fn eye_at(feet: [f64; 3], duck: Option<f64>) -> [f64; 3] {
    [feet[0], feet[1], feet[2] + EYE_STANDING - (EYE_STANDING - EYE_CROUCHED) * duck.unwrap_or(0.0).clamp(0.0, 1.0)]
}

/// Where it landed: the detonation, or else the flight's last point.
pub fn landing(throw: &Throw) -> Option<Vec3> {
    throw.detonation.map(|(_, at)| at).or(throw.flight_end.map(|(_, at)| at))
}

/// Seconds from the throw's tick to its detonation.
pub fn airtime(throw: &Throw) -> Option<f32> {
    let (detonated, _) = throw.detonation?;
    Some((detonated.0 - throw.tick.0) as f32 / Tick::PER_SECOND as f32)
}

/// How long a grenade's effect lasts when its expiry is not recorded: CS2's smoke 18 s, its fires 7 s, a decoy 15 s; a
/// flash and an HE are over when they go off.
pub fn nominal_seconds(grenade: Grenade) -> f32 {
    match grenade {
        Grenade::Smoke => 18.0,
        Grenade::Molotov | Grenade::Incendiary => 7.0,
        Grenade::Decoy => 15.0,
        Grenade::Flash | Grenade::He => 0.0,
    }
}

/// The effect's length from the detonation to the expiry, and whether both were recorded; otherwise the grenade's
/// nominal length.
pub fn effect_seconds(throw: &Throw) -> (f32, bool) {
    match (throw.detonation, throw.end) {
        (Some((detonated, _)), Some(end)) => ((end.0 - detonated.0) as f32 / Tick::PER_SECOND as f32, true),
        _ => (nominal_seconds(throw.grenade), false),
    }
}

/// Where the thrower stood: the take-off point for a throw released in the air, the origin otherwise.
pub fn stand(pawn: &Pawn) -> Vec3 {
    if pawn.on_ground == Some(true) { pawn.origin } else { pawn.takeoff.unwrap_or(pawn.origin) }
}

/// The thrower's eye at release, in the doubles the cameras use: where they stood plus CS2's view height, crouched for
/// a crouched throw, else lowered by the duck amount.
pub fn eye(throw: &Throw, technique: Option<&Technique>) -> Option<[f64; 3]> {
    let pawn = throw.thrower.as_ref()?;
    let duck = if technique.is_some_and(|t| t.crouched) { 1.0 } else { pawn.duck.map_or(0.0, f64::from) };
    Some(eye_at(stand(pawn).map(f64::from), Some(duck)))
}

/// `setpos <stand>; setang <pitch> <yaw> 0`: where the thrower stood, looking as they did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Practice {
    pub command: String,
    /// Whether the command reproduces the throw: thrown standing still or as a plain jump-throw, with a known click.
    /// A moving throw also needs its run start, which the facts do not hold.
    pub ready: bool,
}

pub fn practice(throw: &Throw, technique: Option<&Technique>) -> Option<Practice> {
    let pawn = throw.thrower.as_ref()?;
    let (pitch, yaw) = (pawn.pitch?, pawn.yaw?);
    let [x, y, z] = stand(pawn);
    let ready = technique.is_some_and(|t| matches!(t.movement, Movement::Stand | Movement::Jumpthrow) && t.click.is_some());
    Some(Practice { command: format!("setpos {x:.2} {y:.2} {z:.2}; setang {pitch:.2} {yaw:.2} 0"), ready })
}

/// Seconds since the round's freeze time ended.
pub fn round_seconds(throw: &Throw, round: Option<&Round>) -> Option<f32> {
    let freeze_end = round?.freeze_end?;
    Some(throw.tick.seconds_since(freeze_end) as f32)
}

/// Whether the bomb was down when it was thrown (`plant ≤ tick`); None without the round.
pub fn after_plant(throw: &Throw, round: Option<&Round>) -> Option<bool> {
    Some(round?.plant.is_some_and(|plant| plant <= throw.tick))
}

/// Half a second: how far from a flash's detonation a sample may be to place a player it blinded.
pub const BLIND_REACH: i32 = 32;

/// The ticks whose samples place the players a flash blinded: half a second either side of its detonation.
pub fn blind_window(throw: &Throw) -> Option<RangeInclusive<Tick>> {
    let (at, _) = throw.detonation.filter(|_| throw.grenade == Grenade::Flash)?;
    Some(at.plus(-BLIND_REACH)..=at.plus(BLIND_REACH))
}

/// The same player in two records: by Steam id when both have one, else by name.
fn same_player(a: &Player, b: &Player) -> bool {
    match (a.steamid, b.steamid) {
        (Some(x), Some(y)) => x == y,
        _ => a.name.is_some() && a.name == b.name,
    }
}

/// Places each blinded player whose position the facts lack at their sample nearest the flash's detonation, within
/// half a second ([`blind_window`]); one with no such sample stays unplaced.
pub fn place_blinds(throw: &Throw, blinds: &mut [Blind], samples: &[Sample]) {
    let Some(window) = blind_window(throw) else { return };
    let at = window.start().plus(BLIND_REACH);
    for blind in blinds.iter_mut().filter(|b| b.pos.is_none()) {
        blind.pos = samples
            .iter()
            .filter(|s| window.contains(&s.tick) && same_player(&s.player, &blind.player))
            .min_by_key(|s| ((s.tick.0 - at.0).abs(), s.tick))
            .map(|s| s.origin);
    }
}

/// Whether a throw is part of play, and so of a lineup: in a demo that records rounds, it was thrown in a round (not in
/// the warm-up) and not after that round ended. A demo without rounds (a practice server's) is all play.
pub fn in_play(throw: &Throw, round: Option<&Round>, demo_has_rounds: bool) -> bool {
    if !demo_has_rounds {
        return true;
    }
    throw.round.is_some() && round.and_then(|r| r.end).is_none_or(|end| throw.tick <= end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::throw;
    use crate::technique::Click;
    use crate::{DemoId, Side};

    fn pawn() -> Pawn {
        Pawn { origin: [10.0, 20.0, 30.0], pitch: Some(-12.5), yaw: Some(45.0), on_ground: Some(true), duck: Some(0.0), ..Default::default() }
    }

    #[test]
    fn the_eye_is_cs2s_view_height_above_where_they_stood() {
        assert_eq!(eye_at([1.0, 2.0, 3.0], None)[2], 67.0);
        assert_eq!(eye_at([1.0, 2.0, 3.0], Some(1.0))[2], 49.0);
        assert_eq!(eye_at([1.0, 2.0, 3.0], Some(0.5))[2], 58.0);
        let mut t = throw("t", "d", Grenade::Smoke, Side::T, [12.0, 22.0, 94.0]);
        assert_eq!(eye(&t, None), None);
        t.thrower = Some(pawn());
        assert_eq!(eye(&t, None), Some([10.0, 20.0, 94.0]));
        // crouched by the technique, whatever the duck amount says
        let crouched = Technique { movement: Movement::Stand, click: Some(Click::Left), crouched: true };
        assert_eq!(eye(&t, Some(&crouched)).unwrap()[2], 76.0);
        // released in the air: from the take-off point
        t.thrower = Some(Pawn { on_ground: Some(false), takeoff: Some([0.0, 0.0, 0.0]), ..pawn() });
        assert_eq!(eye(&t, None), Some([0.0, 0.0, 64.0]));
    }

    #[test]
    fn landing_times_and_the_effect() {
        let mut t = throw("t", "d", Grenade::Smoke, Side::T, [0.0, 0.0, 0.0]);
        assert_eq!((landing(&t), airtime(&t)), (None, None));
        t.flight_end = Some((Tick(1200), [5.0, 6.0, 7.0]));
        assert_eq!(landing(&t), Some([5.0, 6.0, 7.0]));
        assert_eq!(effect_seconds(&t), (18.0, false));
        t.tick = Tick(1000);
        t.detonation = Some((Tick(1100), [800.0, 900.0, 40.0]));
        t.end = Some(Tick(1100 + 64 * 18 + 1));
        assert_eq!(landing(&t), Some([800.0, 900.0, 40.0]));
        assert_eq!(airtime(&t), Some(1.5625));
        assert_eq!(effect_seconds(&t), (18.015625, true));
        t.grenade = Grenade::Flash;
        t.end = None;
        assert_eq!(effect_seconds(&t), (0.0, false));
    }

    #[test]
    fn the_practice_command_stands_where_they_stood() {
        let mut t = throw("t", "d", Grenade::Smoke, Side::T, [12.0, 22.0, 94.0]);
        t.thrower = Some(pawn());
        let stand_left = Technique { movement: Movement::Stand, click: Some(Click::Left), crouched: false };
        let p = practice(&t, Some(&stand_left)).unwrap();
        assert_eq!(p.command, "setpos 10.00 20.00 30.00; setang -12.50 45.00 0");
        assert!(p.ready);
        // a jump-throw from its take-off, ready; a running throw is not
        t.thrower = Some(Pawn { on_ground: Some(false), takeoff: Some([1.0, 2.0, 3.0]), ..pawn() });
        let jump = Technique { movement: Movement::Jumpthrow, ..stand_left };
        assert_eq!(practice(&t, Some(&jump)).unwrap(), Practice { command: "setpos 1.00 2.00 3.00; setang -12.50 45.00 0".into(), ready: true });
        let run = Technique { movement: Movement::Run, ..stand_left };
        assert!(!practice(&t, Some(&run)).unwrap().ready);
        assert!(!practice(&t, None).unwrap().ready, "no technique, no click");
        t.thrower.as_mut().unwrap().yaw = None;
        assert_eq!(practice(&t, Some(&stand_left)), None);
    }

    #[test]
    fn a_blinded_player_without_a_position_stands_where_they_were_sampled_nearest_the_flash() {
        let mut t = throw("f", "d", Grenade::Flash, Side::T, [0.0; 3]);
        t.detonation = Some((Tick(2560), [0.0; 3]));
        assert_eq!(blind_window(&t), Some(Tick(2528)..=Tick(2592)));
        let player = |name: &str, steamid: Option<u64>| Player { name: Some(name.into()), steamid };
        let sample = |tick: i32, who: Player, x: f32| Sample { tick: Tick(tick), player: who, side: Side::Ct, origin: [x, 0.0, 0.0], pitch: None, yaw: None, duck: None, alive: true, place: None };
        let samples = [sample(2530, player("ZywOo", Some(1)), 1.0), sample(2562, player("ZywOo", Some(1)), 2.0), sample(2562, player("apEX", None), 3.0), sample(2600, player("flameZ", Some(3)), 4.0)];
        let blind = |who: Player, pos: Option<Vec3>| Blind { player: who, side: Some(Side::Ct), pos, seconds: 1.0 };
        let mut blinds = [blind(player("renamed", Some(1)), None), blind(player("apEX", None), None), blind(player("flameZ", Some(3)), None), blind(player("x", None), Some([9.0, 9.0, 9.0]))];
        place_blinds(&t, &mut blinds, &samples);
        let placed: Vec<Option<f32>> = blinds.iter().map(|b| b.pos.map(|p| p[0])).collect();
        // by Steam id across a name change, the nearest tick; by name without one; a sample too late places nobody;
        // a recorded position stays
        assert_eq!(placed, [Some(2.0), Some(3.0), None, Some(9.0)]);
        t.grenade = Grenade::He;
        assert_eq!(blind_window(&t), None);
    }

    #[test]
    fn round_time_after_plant_and_play() {
        let mut t = throw("t", "d", Grenade::Smoke, Side::T, [0.0; 3]);
        t.tick = Tick(2280);
        let round = Round { demo: DemoId("d".into()), number: 3, freeze_end: Some(Tick(1000)), plant: Some(Tick(2000)), end: Some(Tick(3000)), winner: None };
        assert_eq!(round_seconds(&t, Some(&round)), Some(20.0));
        assert_eq!(after_plant(&t, Some(&round)), Some(true));
        assert_eq!(after_plant(&t, Some(&Round { plant: None, ..round.clone() })), Some(false));
        assert_eq!((round_seconds(&t, None), after_plant(&t, None)), (None, None));
        // a warm-up throw is not play, nor one after the round ended; a demo without rounds is all play
        assert!(!in_play(&t, None, true));
        t.round = Some(3);
        assert!(in_play(&t, Some(&round), true));
        t.tick = Tick(3001);
        assert!(!in_play(&t, Some(&round), true));
        t.round = None;
        assert!(in_play(&t, None, false));
    }
}
