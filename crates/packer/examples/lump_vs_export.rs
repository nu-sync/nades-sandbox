//! The records the packer reads from a map's own files ([`nades_packer::lump`]) beside the ones show-me-the-nades'
//! `export_map.py` wrote: `cargo run --release -p nades-packer --example lump_vs_export -- <map>...`. Numbers compare as
//! f64; the entity lump against export_map.py's decompile in `~/.cache/nades/cs2/export`.

use std::path::Path;

use anyhow::{Context as _, Result};
use nades_packer::{Inputs, lump};
use serde_json::Value;

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

fn file(path: &Path) -> Value {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

fn main() -> Result<()> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    for map in std::env::args().skip(1) {
        let inputs = Inputs::for_map(&map)?;
        let cli = inputs.cli().with_context(|| nades_packer::no_cli(&inputs))?;
        let pak = nades_source2::vpk::Vpk::open(&inputs.pak())?;
        let started = std::time::Instant::now();
        let mut d = lump::Decompiler::new(cli, &std::env::temp_dir(), &format!("compare-{map}"))?;
        let l = lump::read(&mut d, &lump::Game { maps: inputs.csgo().join("maps"), pak: Some(&pak), pak_path: inputs.pak() }, &map)?;
        let seconds = started.elapsed().as_secs_f64();
        let ents = std::fs::read_to_string(home.join(format!(".cache/nades/cs2/export/{map}/sky/maps/{map}/entities/default_ents.vents"))).unwrap_or_default();
        let lut = std::fs::read(inputs.beside(".lut.bin")).ok();
        let verdict = |a: &Value, b: &Value| if same(a, b) { "same".to_string() } else { format!("DIFFERS\n      ours   {a}\n      theirs {b}") };
        println!("{map}: {} CLI runs in {seconds:.2} s", l.runs);
        println!("  entities: {}", if l.entities == ents { "same" } else { "DIFFER" });
        println!("  post.json: {}", verdict(&l.post, &file(&inputs.beside(".post.json"))));
        println!("  lut.bin: {}", if l.lut == lut { "same" } else { "DIFFERS" });
        println!("  sky.json: {}", verdict(&l.sky, &file(&inputs.beside(".sky.json"))));
        println!("  skybox.json: {}", verdict(&l.skybox, &file(&inputs.beside(".skybox.json"))));
    }
    Ok(())
}
