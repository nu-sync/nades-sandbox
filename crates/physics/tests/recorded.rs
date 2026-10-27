//! The simulation against recorded throws: `fixture.json` holds 145 of them from pro demos (derived numbers only,
//! written by `examples/flight_check.rs --fixture`). Free flight, fuses and the release need nothing else; the
//! landings need the maps' colliders, so they run where the packs are and are skipped elsewhere (CI).

use glam::DVec3;
use nades_physics::{Grenade, Release, TICK_RATE, Throw, fly, release};
use nades_scene::collider::Collider;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    throws: Vec<Recorded>,
}

#[derive(Deserialize)]
struct Recorded {
    map: String,
    grenade: Grenade,
    start: [f64; 3],
    vel: [f64; 3],
    /// The step after the release the detonate event fired on, and the position it reported.
    det_step: i64,
    det: [f64; 3],
    /// It went off before touching anything.
    free: bool,
    /// The simulation's landing error on the map's collider when the fixture was written.
    landing_error: f64,
    /// The networked flight until it first turned, every 8th tick: [step, x, y, z].
    flight: Vec<[f64; 4]>,
    release: Option<Setup>,
}

#[derive(Deserialize)]
struct Setup {
    throw: Throw,
    p0_error: f64,
    v0_error: f64,
}

fn fixture() -> Vec<Recorded> {
    let fixture: Fixture = serde_json::from_str(include_str!("fixture.json")).expect("the fixture parses");
    assert!(fixture.throws.len() >= 100);
    fixture.throws
}

fn recorded_release(r: &Recorded) -> Release {
    Release { t: 0.0, p0: DVec3::from(r.start), v0: DVec3::from(r.vel) }
}

#[test]
fn the_free_flight_follows_the_recorded_one() {
    let mut points = 0;
    for r in fixture() {
        let path = fly(&recorded_release(&r), r.grenade, None);
        for [step, x, y, z] in &r.flight {
            let Some((t, p)) = path.points.get(*step as usize) else {
                continue;
            };
            assert!((t * TICK_RATE - step).abs() < 1e-9);
            // positions are networked to 1/32 of a unit
            let off = (*p - DVec3::new(*x, *y, *z)).length();
            assert!(off < 0.1, "{} {:?}: {off:.3} units off at step {step}", r.map, r.grenade);
            points += 1;
        }
    }
    assert!(points > 1500, "{points}");
}

#[test]
fn fuses_and_airbursts_go_off_where_and_when_the_game_did() {
    let mut checked = 0;
    for r in fixture().iter().filter(|r| r.free && matches!(r.grenade, Grenade::Flash | Grenade::He | Grenade::Molotov | Grenade::Incendiary)) {
        let path = fly(&recorded_release(r), r.grenade, None);
        assert_eq!((path.detonation.t * TICK_RATE).round() as i64, r.det_step, "{} {:?}", r.map, r.grenade);
        let off = (path.detonation.pos - DVec3::from(r.det)).length();
        assert!(off < 0.1, "{} {:?}: {off:.3} units off", r.map, r.grenade);
        checked += 1;
    }
    assert!(checked >= 20, "{checked}");
}

#[test]
fn the_release_from_the_setup_matches_the_recorded_one() {
    let mut checked = 0;
    for r in fixture() {
        let Some(setup) = &r.release else { continue };
        let rel = release(&setup.throw);
        let (p0, v0) = ((rel.p0 - DVec3::from(r.start)).length(), (rel.v0 - DVec3::from(r.vel)).length());
        assert!(p0 <= setup.p0_error && v0 <= setup.v0_error, "{} {:?} {:?}: p0 {p0:.3} v0 {v0:.3}", r.map, r.grenade, setup.throw.movement);
        checked += 1;
    }
    assert!(checked >= 40, "{checked}");
}

/// Needs the packs (`~/.cache/nades/packs` or the configured folder); skipped without them.
#[test]
fn landings_on_the_maps_colliders_stay_within_the_measured_errors() {
    let Ok(packs) = nades_config::packs(None) else {
        return;
    };
    let throws = fixture();
    let mut maps: Vec<&str> = throws.iter().map(|r| r.map.as_str()).collect();
    maps.dedup();
    let (mut checked, mut within) = (0, 0);
    for map in maps {
        let path = packs.path.join(format!("{map}.pack"));
        let Ok(pack) = nades_pack::Pack::open(&path) else {
            eprintln!("skipped {map}: no pack at {}", path.display());
            continue;
        };
        let collider = Collider::from_awmh(pack.collider().expect("a Collider section")).expect("a triangle mesh");
        for r in throws.iter().filter(|r| r.map == map) {
            let path = fly(&recorded_release(r), r.grenade, Some(&collider));
            let error = (path.detonation.pos - DVec3::from(r.det)).length();
            assert!(error <= r.landing_error + 0.5, "{map} {:?}: lands {error:.1} off, measured {:.1}", r.grenade, r.landing_error);
            checked += 1;
            within += usize::from(error <= 32.0);
        }
    }
    eprintln!("{checked} landings checked, {within} within 32 units");
}
