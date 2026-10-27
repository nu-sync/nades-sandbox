//! The demo adapter's writer, read back through the file adapter: facts in, the same facts out. A real demo is read by
//! `reads_a_real_demo` (ignored without `NADES_SANDBOX_DEMO`), and by the parity check in docs/decisions.md.

use std::path::{Path, PathBuf};

use nades_model::{DemoId, Effect, MapName, ThrowId, ThrowSource, Tick};
use nades_source_demo::extract::{BlindFact, Facts, RoundFact, SampleFact, ThrowFact};
use nades_source_demo::tracks::{Flame, PathPoint, SmokeVoxels};
use nades_source_file::{FileSource, check};

const SHA: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nades-source-demo-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A smoke thrown in a jump with a voxel log, an incendiary with two flames, a flash that blinded one player, and a
/// throw without a side (left out), in one round with samples.
fn facts() -> Facts {
    let smoke = ThrowFact {
        entity: 211,
        tick: 2000,
        last_tick: 3600,
        round: Some(1),
        nade: "smoke",
        side: Some("T"),
        player: Some("ZywOo".into()),
        steamid: Some(76561198113666193),
        team: Some("Team Vitality".into()),
        place: Some("TSpawn".into()),
        start: [-2098.0, 401.0, 1724.5],
        velocity: [620.5, 130.25, 512.0],
        origin: Some([-2100.0, 400.5, 1660.25]),
        takeoff: Some([-2101.5, 398.0, 1613.0]),
        eye: Some([-35.5, 12.25, 0.0]),
        pawn_velocity: Some([0.0, 0.0, 218.75]),
        on_ground: Some(false),
        duck: Some(0.0),
        walking: Some(false),
        detonate_tick: Some(2100),
        land: [-1340.5, 512.25, 1614.0],
        effect_ticks: Some(1412),
        flight: vec![PathPoint { tick: 2000, pos: [-2098.0, 401.0, 1724.5] }, PathPoint { tick: 2100, pos: [-1340.5, 512.25, 1614.0] }],
        voxels: Some(SmokeVoxels { first_tick: 2101, detonation: [-1340.5, 512.25, 1615.0], log: vec![0, 0, 3, 0, 0, 0, 0] }),
        ..Default::default()
    };
    let fire = ThrowFact {
        entity: 305,
        tick: 2400,
        last_tick: 2500,
        round: Some(1),
        nade: "molotov",
        incendiary: true,
        side: Some("CT"),
        player: Some("torzsi".into()),
        steamid: Some(76561198000000002),
        start: [600.0, -1200.0, 1746.0],
        velocity: [-150.0, 300.0, 100.0],
        detonate_tick: Some(2500),
        land: [400.0, -800.0, 1700.0],
        effect_ticks: Some(450),
        flight: vec![PathPoint { tick: 2400, pos: [600.0, -1200.0, 1746.0] }, PathPoint { tick: 2500, pos: [400.0, -800.0, 1700.0] }],
        flames: vec![
            Flame { slot: 0, pos: [400.0, -800.0, 1700.0], normal: Some([0.0, 0.0, 1.0]), first_tick: 2500, last_tick: 2950 },
            Flame { slot: 1, pos: [430.0, -790.0, 1700.0], normal: None, first_tick: 2540, last_tick: 2900 },
        ],
        ..Default::default()
    };
    let flash = ThrowFact { entity: 410, tick: 2700, last_tick: 2790, round: Some(1), nade: "flash", side: Some("T"), player: Some("ZywOo".into()), steamid: Some(76561198113666193), start: [-1000.0, 0.0, 1704.0], velocity: [500.0, 0.0, 200.0], detonate_tick: Some(2790), land: [-500.0, 0.0, 1700.0], ..Default::default() };
    let sideless = ThrowFact { entity: 9, tick: 100, last_tick: 120, nade: "decoy", start: [0.0; 3], velocity: [0.0; 3], ..Default::default() };
    let sample = |tick, steamid, player: &str, side, alive| SampleFact { tick, steamid, player: Some(player.into()), side, pos: [-1800.5, 250.25, 1613.0], eye: Some([4.5, 90.0, 0.0]), duck: Some(0.25), alive, place: Some("Mid".into()) };
    Facts {
        map: "de_cache".into(),
        rounds: vec![RoundFact { number: 1, freeze_end: Some(1000), plant: Some(5000), end: 9000, winner: Some("T") }],
        throws: vec![smoke, fire, flash, sideless],
        blinds: vec![BlindFact { throw: 2, player: Some("Brollan".into()), steamid: Some(76561198000000003), side: Some("CT"), pos: Some([-600.0, 10.0, 1613.0]), seconds: 2.5 }],
        samples: vec![sample(2784, 76561198000000003, "Brollan", "CT", true), sample(2784, 76561198113666193, "ZywOo", "T", true), sample(2816, 76561198000000003, "Brollan", "CT", false)],
    }
}

fn written(dir: &Path) -> PathBuf {
    let demo = dir.join("match.dem");
    std::fs::write(&demo, b"not read: the facts are given").unwrap();
    let file = dir.join("throws.sqlite");
    nades_source_demo::write(&facts(), SHA, &demo, &file).unwrap();
    file
}

#[test]
fn the_written_facts_read_back_as_they_were() {
    let dir = scratch("facts");
    let file = written(&dir);
    let report = check(&file).unwrap();
    assert!(report.errors.is_empty() && report.warnings.is_empty(), "{:?} {:?}", report.errors, report.warnings);
    assert!(report.meta.contains(&("producer".into(), nades_source_demo::PRODUCER.into())));
    let c = report.census;
    assert_eq!((c.demos, c.throws, c.blinds, c.samples, c.smokes_with_voxels, c.fires_with_flames, c.airborne_with_takeoff), (1, 3, 1, 3, 1, 1, 1), "the throw without a side is left out");

    let source = FileSource::open(&[file]).unwrap();
    let facts = source.map(&MapName("de_cache".into())).unwrap();
    assert_eq!(facts.demos[0].file_name.as_deref(), Some("match.dem"));
    assert!(facts.demos[0].played_at.is_some(), "the file's time");
    let smoke = facts.throws.iter().find(|t| t.id.as_str() == format!("{SHA}:2000:211")).unwrap();
    assert_eq!((smoke.player.name.as_deref(), smoke.player.steamid, smoke.place.as_deref(), smoke.team.as_deref()), (Some("ZywOo"), Some(76561198113666193), Some("TSpawn"), Some("Team Vitality")));
    let pawn = smoke.thrower.unwrap();
    assert_eq!((pawn.takeoff, pawn.on_ground, pawn.pitch), (Some([-2101.5, 398.0, 1613.0]), Some(false), Some(-35.5)));
    assert_eq!((smoke.detonation, smoke.end), (Some((Tick(2100), [-1340.5, 512.25, 1614.0])), Some(Tick(3512))));
    let fire = facts.throws.iter().find(|t| t.id.as_str().ends_with(":2400:305")).unwrap();
    assert_eq!((fire.grenade.name(), fire.side.name()), ("incendiary", "CT"));

    let tracks = source.tracks(&ThrowId(format!("{SHA}:2000:211"))).unwrap().unwrap();
    assert_eq!(tracks.flight, [(Tick(2000), [-2098.0, 401.0, 1724.5]), (Tick(2100), [-1340.5, 512.25, 1614.0])]);
    let Effect::Smoke(Some(voxels)) = tracks.effect else { panic!() };
    assert_eq!((voxels.first, voxels.origin, voxels.bytes), (Tick(2101), Some([-1340.5, 512.25, 1615.0]), vec![0, 0, 3, 0, 0, 0, 0]));
    let Effect::Fire(flames) = source.tracks(&fire.id).unwrap().unwrap().effect else { panic!() };
    assert_eq!(flames.iter().map(|f| (f.slot, f.first.0, f.normal)).collect::<Vec<_>>(), [(0, 2500, Some([0.0, 0.0, 1.0])), (1, 2540, None)]);
    let Effect::Flash(blinds) = source.tracks(&ThrowId(format!("{SHA}:2700:410"))).unwrap().unwrap().effect else { panic!() };
    assert_eq!((blinds[0].player.name.as_deref(), blinds[0].pos, blinds[0].seconds), (Some("Brollan"), Some([-600.0, 10.0, 1613.0]), 2.5));

    let samples = source.samples(&DemoId(SHA.into()), Tick(0)..=Tick(10_000)).unwrap();
    assert_eq!(samples.iter().map(|s| (s.tick.0, s.player.name.as_deref().unwrap(), s.alive, s.duck)).collect::<Vec<_>>(), [(2784, "Brollan", true, Some(0.25)), (2784, "ZywOo", true, Some(0.25)), (2816, "Brollan", false, Some(0.25))]);
    assert_eq!(source.places(&MapName("de_cache".into())).unwrap().len(), 2, "the living samples' places");
}

#[test]
fn a_demo_in_the_cache_is_not_parsed_again() {
    let dir = scratch("cache");
    let demo = dir.join("match.dem");
    std::fs::write(&demo, b"PBDEMS2\0 not a whole demo").unwrap();
    let sha = nades_source_demo::sha256(&demo).unwrap();
    let cache = dir.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    // what an earlier import left for this demo; the bytes are no demo awpy can read, so a parse would fail
    nades_source_demo::write(&facts(), &sha, &demo, &nades_source_demo::cached(&cache, &sha)).unwrap();
    let done = nades_source_demo::import(&demo, &cache, &mut |_| {}).unwrap();
    assert_eq!((done.parsed, done.throws, done.sha256.as_str()), (false, 3, sha.as_str()));
    // another reader revision's file is parsed again, and this demo does not parse
    rusqlite::Connection::open(&done.file).unwrap().execute("update meta set value = 'someone else' where key = 'producer'", []).unwrap();
    assert!(nades_source_demo::import(&demo, &cache, &mut |_| {}).is_err());
    assert_eq!(nades_source_demo::demos_in(&dir).unwrap(), [demo]);
}

/// `NADES_SANDBOX_DEMO=<a .dem file> cargo test --release -p nades-source-demo -- --ignored`
#[test]
#[ignore = "needs NADES_SANDBOX_DEMO, a real CS2 demo"]
fn reads_a_real_demo() {
    let demo = PathBuf::from(std::env::var_os("NADES_SANDBOX_DEMO").expect("NADES_SANDBOX_DEMO"));
    let dir = scratch("real");
    let mut shares = Vec::new();
    let done = nades_source_demo::import(&demo, &dir, &mut |s| shares.push(s)).unwrap();
    assert!(done.parsed && done.throws > 0);
    assert!(shares.windows(2).all(|w| w[0] <= w[1]) && shares.last() == Some(&1.0), "progress only goes up, to 1");
    let report = check(&done.file).unwrap();
    assert!(report.errors.is_empty() && report.warnings.is_empty(), "{:?} {:?}", report.errors, report.warnings);
    assert!(report.census.samples > 0 && report.census.demos_with_rounds == 1);
    let again = nades_source_demo::import(&demo, &dir, &mut |_| {}).unwrap();
    assert_eq!((again.parsed, again.file), (false, done.file));
}
