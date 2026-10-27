//! A third producer: the fixture `tools/cs2_throws.py` writes with Python's standard library, read through the file
//! adapter and the sandbox's rules.

use std::path::{Path, PathBuf};
use std::process::Command;

use nades_model::technique::{Click, Movement};
use nades_model::{DemoId, Effect, Grenade, Library, LineupQuery, MapName, Side, SourceError, Technique, ThrowId, ThrowSource, Tick};
use nades_source_file::{FileSource, check};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nades-source-file-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the reference writer: `python3` (or `python`) is needed, as in CI.
fn write(kind: &str, out: &Path) {
    let tool = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/cs2_throws.py");
    let ran = ["python3", "python"].iter().find_map(|python| Command::new(python).arg("-I").arg(&tool).arg(kind).arg(out).output().ok()).expect("python3 on PATH (the fixture's writer)");
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
}

fn fixture(dir: &Path) -> PathBuf {
    let path = dir.join("fixture.sqlite");
    write("fixture", &path);
    path
}

fn id(s: &str) -> ThrowId {
    ThrowId(s.into())
}

#[test]
fn the_fixture_reads_as_facts_and_the_rules_apply() {
    let dir = scratch("facts");
    let source = FileSource::open(&[fixture(&dir)]).unwrap();
    assert_eq!(source.maps().unwrap(), [MapName("de_inferno".into()), MapName("de_mirage".into())]);
    let mirage = MapName("de_mirage".into());
    let facts = source.map(&mirage).unwrap();
    assert_eq!((facts.demos.len(), facts.rounds.len(), facts.throws.len()), (2, 3, 10));
    let demo = facts.demos.iter().find(|d| d.id.as_str() == "fixture-demo-1").unwrap();
    assert_eq!((demo.played_at, demo.file_name.as_deref(), demo.tags.as_slice()), (Some(1767225600000), Some("fixture-1.dem"), &[("fixture.event".to_string(), "Fixture Cup".to_string())][..]));
    let window = facts.throws.iter().find(|t| t.id.as_str() == "m1-window-a").unwrap();
    assert_eq!((window.player.steamid, window.tags.len(), window.round), (Some(76561198000000001), 1, Some(1)));
    assert_eq!(window.flight_end.map(|(t, _)| t), Some(Tick(1620)), "the flight's last point, without the flight");

    let library = Library::new(facts, source.places(&mirage).unwrap());
    let lineups = library.lineups(&LineupQuery::default());
    let summary: Vec<(u32, &str, u32, u32)> = lineups.iter().map(|l| (l.id.0, l.grenade.name(), l.throws, l.demos)).collect();
    assert_eq!(summary, [(1, "smoke", 2, 2), (3, "molotov", 2, 1), (2, "smoke", 1, 1), (4, "flash", 1, 1), (5, "he", 1, 1), (6, "decoy", 1, 1), (7, "flash", 1, 1)]);
    let smoke = &lineups[0];
    assert_eq!((smoke.example.as_str(), smoke.to.as_deref(), smoke.from.as_deref()), ("m1-window-a", Some("Window"), Some("TSpawn")));
    assert_eq!(smoke.technique, Some(Technique { movement: Movement::Stand, click: Some(Click::Left), crouched: false }));
    assert_eq!(library.derived(&id("m1-warmup")).unwrap().lineup, None, "a warm-up throw is no lineup's");
    let jump = library.derived(&id("m1-jungle-jump")).unwrap();
    assert_eq!(jump.technique.map(|t| t.name()).as_deref(), Some("jumpthrow/left"));
    assert_eq!(jump.practice.as_ref().map(|p| (p.command.as_str(), p.ready)), Some(("setpos 1100.00 100.00 -160.00; setang -45.00 150.00 0", true)));
    // the fire lineup holds the T molotov and the CT incendiary; filtered, each alone
    assert_eq!(library.lineups(&LineupQuery { grenade: Some(Grenade::Incendiary), ..Default::default() }).iter().map(|l| (l.id.0, l.side)).collect::<Vec<_>>(), [(3, Some(Side::Ct))]);
    assert_eq!(library.derived(&id("i3-minimum")), None, "another map's");

    // a throw's viewpoints: the samples of its window, nearest the landing first
    let (demo, ticks) = library.viewpoint_window(&id("m1-window-a")).unwrap();
    assert_eq!((demo.as_str(), ticks.clone()), ("fixture-demo-1", Tick(1716)..=Tick(2772)));
    let samples = source.samples(&demo, ticks).unwrap();
    let views = library.viewpoints(&id("m1-window-a"), &samples);
    assert_eq!(views.len(), 66);
    assert_eq!((views[0].player.name.as_deref(), views[0].tick), (Some("apEX"), Tick(1736)));
}

#[test]
fn tracks_come_with_the_effect_their_grenade_leaves() {
    let dir = scratch("tracks");
    let source = FileSource::open(&[fixture(&dir)]).unwrap();
    let smoke = source.tracks(&id("m1-window-a")).unwrap().unwrap();
    assert_eq!((smoke.flight.len(), smoke.flight[0].0, smoke.flight[60].0), (61, Tick(1500), Tick(1620)));
    let Effect::Smoke(Some(voxels)) = &smoke.effect else { panic!("{:?}", smoke.effect) };
    assert_eq!((voxels.first, voxels.origin), (Tick(1620), Some([-400.0, -650.0, -200.0])));
    let frames = nades_model::smoke::decode(&voxels.bytes).unwrap();
    assert_eq!((frames.len(), frames[0].emitters.len(), frames[2].fading), (3, 2, true));
    let Effect::Fire(flames) = source.tracks(&id("m1-incendiary")).unwrap().unwrap().effect else { panic!() };
    assert_eq!(flames.iter().map(|f| (f.slot, f.first.0)).collect::<Vec<_>>(), [(0, 8090), (1, 8096), (2, 8102)]);
    assert_eq!(flames[0].normal, Some([0.0, 0.0, 1.0]));
    let Effect::Flash(blinds) = source.tracks(&id("m1-flash")).unwrap().unwrap().effect else { panic!() };
    assert_eq!(blinds.iter().map(|b| (b.player.name.as_deref(), b.side, b.seconds)).collect::<Vec<_>>(), [(Some("ZywOo"), Some(Side::Ct), 2.4), (Some("torzsi"), Some(Side::T), 0.7)]);
    assert_eq!(source.tracks(&id("m1-he")).unwrap().unwrap().effect, Effect::Explosion);
    assert_eq!(source.tracks(&id("m1-decoy")).unwrap().unwrap().effect, Effect::Nothing);
    let jump = source.tracks(&id("m1-jungle-jump")).unwrap().unwrap();
    assert_eq!((jump.flight.len(), jump.effect), (0, Effect::Smoke(None)));
    assert_eq!(source.tracks(&id("nothing")).unwrap(), None);
    assert_eq!(source.map_of(&id("i3-minimum")).unwrap(), Some(MapName("de_inferno".into())));
}

#[test]
fn check_finds_no_problem_and_counts_what_the_fixture_holds() {
    let dir = scratch("check");
    let report = check(&fixture(&dir)).unwrap();
    assert!(report.errors.is_empty() && report.warnings.is_empty(), "{:?} {:?}", report.errors, report.warnings);
    assert!(report.meta.contains(&("format".into(), "cs2-throws".into())) && report.meta.contains(&("version".into(), "2.0".into())));
    let c = report.census;
    assert_eq!((c.demos, c.throws, c.landed, c.with_flight, c.with_view, c.with_release, c.airborne, c.airborne_with_takeoff), (3, 11, 11, 3, 3, 3, 1, 1));
    assert_eq!((c.smokes, c.smokes_with_voxels, c.fires, c.fires_with_flames, c.flashes, c.blinds), (5, 2, 2, 2, 2, 2));
    assert_eq!((c.demos_with_rounds, c.demos_with_samples, c.samples, c.samples_with_place), (2, 3, 403, 403));
    // an empty file is a file with nothing in it; a damaged one is named
    let empty = dir.join("empty.sqlite");
    write("schema", &empty);
    let report = check(&empty).unwrap();
    assert_eq!((report.census.throws, report.errors.len()), (0, 0));
    // a key that names no row is a warning: the reader reads it as unknown
    let dangling = fixture(&dir);
    let db = rusqlite::Connection::open(&dangling).unwrap();
    db.execute_batch("insert into player_samples values (1, 99999, 42, 'T', 0, 0, 0, null, null, null, 1, 77); insert into throw_tags values (12345, 'x.y', 'z')").unwrap();
    drop(db);
    let report = check(&dangling).unwrap();
    assert_eq!(report.errors, Vec::<String>::new());
    assert_eq!(report.warnings, ["1 player samples whose player is not in players", "1 player samples whose place is not in places", "1 throw tags of no throw in throws"]);
    let broken = dir.join("broken.sqlite");
    std::fs::write(&broken, b"not a database at all, just bytes that are long enough to look like a header").unwrap();
    assert!(matches!(check(&broken), Err(SourceError::Corrupt(_))));
}

#[test]
fn files_merge_taking_each_demo_once_and_another_major_version_is_refused() {
    let dir = scratch("merge");
    let first = fixture(&dir);
    let second = dir.join("copy.sqlite");
    std::fs::copy(&first, &second).unwrap();
    let source = FileSource::open(&[first.clone(), second.clone()]).unwrap();
    assert_eq!(source.map(&MapName("de_mirage".into())).unwrap().throws.len(), 10, "the copy's demos are already taken");
    // a folder gives its cs2-throws files
    std::fs::write(dir.join("notes.txt"), "not a throws file").unwrap();
    assert_eq!(nades_source_file::expand(std::slice::from_ref(&dir)).unwrap(), [second.clone(), first.clone()]);
    // version 1 is refused, naming both versions and what to do, before its tables are looked at
    rusqlite::Connection::open(&second).unwrap().execute_batch("update meta set value = '1.0' where key = 'version'; alter table demos rename column demo to x").unwrap();
    match FileSource::open(std::slice::from_ref(&second)) {
        Err(e @ SourceError::Incompatible { .. }) => assert_eq!(e.to_string(), format!("{} is cs2-throws 1.0, an older version: write it again with a current producer, and this sandbox reads cs2-throws 2.x", second.display())),
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("version 1 was read"),
    }
    assert!(matches!(FileSource::open(&[dir.join("missing.sqlite")]), Err(SourceError::Missing(_))));
    assert!(matches!(FileSource::open(&[dir.join("match.dem")]), Err(SourceError::Missing(_))), "a missing demo is missing");
    std::fs::write(dir.join("match.dem"), b"PBDEMS2").unwrap();
    assert!(matches!(FileSource::open(&[dir.join("match.dem")]), Err(SourceError::Incompatible { .. })), "demos come with the demo adapter");
}

#[test]
fn a_file_changed_on_disk_is_read_again() {
    let dir = scratch("changed");
    let path = fixture(&dir);
    let source = FileSource::open(std::slice::from_ref(&path)).unwrap();
    let before = source.revision().unwrap();
    assert_eq!(source.maps().unwrap().len(), 2);
    write("schema", &path);
    assert_ne!(source.revision().unwrap(), before);
    assert_eq!(source.maps().unwrap(), Vec::<MapName>::new());
    assert!(source.samples(&DemoId("fixture-demo-1".into()), Tick(0)..=Tick(10_000)).unwrap().is_empty());
}
