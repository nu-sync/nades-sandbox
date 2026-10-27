//! How the map's own physics (the Collider section's triangles) compares with awpy's published mesh:
//! `cargo run --release -p nades-packer --example physics_vs_awpy -- <map> [<awpy .mesh>]`. A triangle is matched by
//! its corners rounded to 1/16 unit, in any order; the area of each side's unmatched triangles says how much surface
//! one has that the other lacks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context as _, Result};
use nades_packer::Inputs;
use nades_packer::physics;

type Key = [[i32; 3]; 3];

fn key(t: [[f32; 3]; 3]) -> Key {
    let mut k = t.map(|p| p.map(|c| (c * 16.0).round() as i32));
    k.sort();
    k
}

fn area(t: [[f32; 3]; 3]) -> f64 {
    let a = glam::DVec3::from(t[0].map(f64::from));
    let b = glam::DVec3::from(t[1].map(f64::from));
    let c = glam::DVec3::from(t[2].map(f64::from));
    (b - a).cross(c - a).length() / 2.0
}

fn awmh(bytes: &[u8]) -> Vec<[[f32; 3]; 3]> {
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let (nv, nt) = (word(8), word(12));
    let v = |k: usize| [0, 4, 8].map(|o| f32::from_le_bytes(bytes[16 + k * 12 + o..16 + k * 12 + o + 4].try_into().unwrap()));
    (0..nt).map(|t| [0, 4, 8].map(|o| v(word(16 + nv * 12 + t * 12 + o)))).collect()
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let map = args.next().context("usage: physics_vs_awpy <map> [<awpy .mesh>]")?;
    let inputs = Inputs::for_map(&map)?;
    let awpy_path = args.next().map(PathBuf::from).unwrap_or_else(|| inputs.export.with_file_name(format!("{map}.mesh")));
    let started = Instant::now();
    let p = physics::of_map(inputs.cli().with_context(|| nades_packer::no_cli(&inputs))?, &inputs.vpk(), &map, &std::env::temp_dir())?;
    let seconds = started.elapsed().as_secs_f64();
    let our_bytes = p.to_awmh();
    let their_bytes = std::fs::read(&awpy_path).with_context(|| awpy_path.display().to_string())?;
    let ours: Vec<[[f32; 3]; 3]> = awmh(&our_bytes);
    let theirs = awmh(&their_bytes);
    let count = |tris: &[[[f32; 3]; 3]]| {
        let mut m: HashMap<Key, u32> = HashMap::new();
        for &t in tris {
            *m.entry(key(t)).or_default() += 1;
        }
        m
    };
    let (a, b) = (count(&ours), count(&theirs));
    let unmatched = |tris: &[[[f32; 3]; 3]], other: &HashMap<Key, u32>| {
        let (mut n, mut ar, mut total) = (0usize, 0f64, 0f64);
        for &t in tris {
            total += area(t);
            if !other.contains_key(&key(t)) {
                n += 1;
                ar += area(t);
            }
        }
        (n, ar, total)
    };
    let (n_ours, ar_ours, tot_ours) = unmatched(&ours, &b);
    let (n_theirs, ar_theirs, tot_theirs) = unmatched(&theirs, &a);
    println!(
        "{map}: physics {} triangles ({} exported, {} dropped, {} welded) in {seconds:.2} s; awpy {} triangles",
        ours.len(),
        p.exported,
        p.dropped,
        p.welded,
        theirs.len()
    );
    println!("  the same bytes as awpy's file: {}", our_bytes == their_bytes);
    println!("  physics not in awpy: {n_ours} triangles, {:.0} of {:.0} sq units ({:.3}%)", ar_ours, tot_ours, 100.0 * ar_ours / tot_ours);
    println!("  awpy not in physics: {n_theirs} triangles, {:.0} of {:.0} sq units ({:.3}%)", ar_theirs, tot_theirs, 100.0 * ar_theirs / tot_theirs);
    Ok(())
}
