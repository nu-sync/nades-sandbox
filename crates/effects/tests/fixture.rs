//! The simulated effects hold their scores against recorded ones (S33): a smoke and two fires a map, from the committed
//! fixture `examples/effects_check.rs --fixture` writes (derived numbers only: the voxelized map around each, the
//! recorded cloud and flames). The full measure over hundreds of smokes and fires runs locally with that example.

use glam::DVec3;
use nades_effects::measure::{cloud_at, coverage, iou};
use nades_effects::{smoke, voxels};
use nades_scene::fire::{FireKind, Flame};
use nades_scene::smoke::{CELLS, HALF_SPAN, fill_cloud};
use serde_json::Value;

/// How far a score may fall below the fixture's before the test fails.
const SLACK: f64 = 0.01;

fn fixture() -> Value {
    let v: Value = serde_json::from_str(include_str!("../fixtures/effects.json")).expect("the fixture is JSON");
    assert_eq!((v["format"].as_str(), v["version"].as_u64()), (Some("nades-effects-fixture"), Some(1)));
    v
}

/// A voxel set from its hex LZ4 bitset.
fn unpack(hex: &Value) -> Vec<bool> {
    let hex = hex.as_str().expect("a hex string");
    let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex")).collect();
    let bits = lz4_flex::decompress_size_prepended(&bytes).expect("an LZ4 block");
    (0..CELLS).map(|c| bits[c / 8] >> (c % 8) & 1 == 1).collect()
}

fn det(v: &Value) -> DVec3 {
    DVec3::new(v["det"][0].as_f64().unwrap(), v["det"][1].as_f64().unwrap(), v["det"][2].as_f64().unwrap())
}

fn solid(v: &Value) -> Vec<u8> {
    unpack(&v["solid"]).into_iter().map(u8::from).collect()
}

#[test]
fn simulated_smokes_hold_their_iou_with_recorded_clouds() {
    let mut scores = Vec::new();
    for s in fixture()["smokes"].as_array().unwrap() {
        let cloud = fill_cloud(&smoke::grid(&solid(s), &smoke::Model::default()), det(s));
        let full = iou(&cloud_at(cloud.as_ref(), None), &unpack(&s["cloud"]));
        let early = iou(&cloud_at(cloud.as_ref(), Some(2.0)), &unpack(&s["cloud_2s"]));
        let map = s["map"].as_str().unwrap();
        assert!(full >= s["iou"].as_f64().unwrap() - SLACK, "{map}: IoU {full:.3} against {}", s["iou"]);
        assert!(early >= s["iou_2s"].as_f64().unwrap() - SLACK, "{map}: IoU at 2 s {early:.3} against {}", s["iou_2s"]);
        scores.push(full);
    }
    let mean = scores.iter().sum::<f64>() / scores.len() as f64;
    assert!(scores.len() == 7 && mean > 0.7, "{scores:?}");
}

#[test]
fn simulated_fires_hold_their_coverage_of_recorded_flames() {
    let mut scores = Vec::new();
    for f in fixture()["fires"].as_array().unwrap() {
        let kind = if f["kind"] == "incendiary" { FireKind::Incendiary } else { FireKind::Molotov };
        let recorded: Vec<Flame> = f["flames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| {
                let n = |k: usize| x[k].as_f64().unwrap();
                Flame { pos: [n(0), n(1), n(2)], normal: [0.0, 0.0, 1.0], start: n(3), end: n(4) }
            })
            .collect();
        let origin = det(f) - DVec3::splat(HALF_SPAN);
        let simulated = nades_effects::fire_flames_of(kind, det(f), &voxels::collider_of(origin, &solid(f)));
        let (rec, sim) = (coverage(&recorded, &simulated, None).unwrap(), coverage(&simulated, &recorded, None).unwrap_or(0.0));
        let map = f["map"].as_str().unwrap();
        assert!(rec >= f["recorded_covered"].as_f64().unwrap() - SLACK, "{map} {kind:?}: covers {rec:.3} of the recorded fire against {}", f["recorded_covered"]);
        assert!(sim >= f["simulated_covered"].as_f64().unwrap() - SLACK, "{map} {kind:?}: {sim:.3} of it recorded against {}", f["simulated_covered"]);
        scores.push((rec + sim) / 2.0);
    }
    let mean = scores.iter().sum::<f64>() / scores.len() as f64;
    assert!(scores.len() == 14 && mean > 0.6, "{scores:?}");
}
