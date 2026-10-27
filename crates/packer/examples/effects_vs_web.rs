//! The effects the packer reads from the game beside the ones show-me-the-nades' `cs2_effects.py web` wrote:
//! `cargo run --release -p nades-packer --example effects_vs_web -- [<effects/web folder>]`. Prints, for the noise and
//! each flipbook, whether the layout is the same and how far the bytes are apart.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use nades_packer::{Inputs, effects};

fn png(path: &std::path::Path) -> Result<(u32, u32, Vec<u8>)> {
    let mut reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).with_context(|| path.display().to_string())?)).read_info()?;
    let mut buffer = vec![0u8; reader.output_buffer_size().context("size")?];
    let info = reader.next_frame(&mut buffer)?;
    buffer.truncate(info.buffer_size());
    Ok((info.width, info.height, buffer))
}

fn difference(a: &[u8], b: &[u8]) -> String {
    if a.len() != b.len() {
        return format!("{} bytes against {}", a.len(), b.len());
    }
    let differ = a.iter().zip(b).filter(|(x, y)| x != y).count();
    let max = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
    let mean = a.iter().zip(b).map(|(x, y)| f64::from(x.abs_diff(*y))).sum::<f64>() / a.len().max(1) as f64;
    format!("{differ} of {} bytes differ, by at most {max} (mean {mean:.4})", a.len())
}

fn main() -> Result<()> {
    let inputs = Inputs::for_map("de_cache")?;
    let web = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache/nades/cs2/effects/web"));
    let started = std::time::Instant::now();
    let fx = effects::build(inputs.cli().with_context(|| nades_packer::no_cli(&inputs))?, &inputs.pak(), &std::env::temp_dir())?;
    println!("built in {:.2} s", started.elapsed().as_secs_f64());
    println!("noise: {}", difference(&fx.noise, &std::fs::read(web.join("worley_perlin.bin"))?));
    let layout: serde_json::Value = serde_json::from_slice(&std::fs::read(web.join("flipbooks.json"))?)?;
    for f in &fx.flipbooks {
        let theirs = &layout[&f.name];
        let sequences: Vec<(u32, u32)> = theirs["sequences"].as_array().into_iter().flatten().map(|s| (s[0].as_u64().unwrap_or(0) as u32, s[1].as_u64().unwrap_or(0) as u32)).collect();
        let same = theirs["columns"].as_u64() == Some(u64::from(f.columns)) && theirs["rows"].as_u64() == Some(u64::from(f.rows)) && theirs["cell"].as_u64() == Some(u64::from(f.cell)) && sequences == f.sequences;
        let (w, h, rgba) = png(&web.join(format!("{}.png", f.name)))?;
        println!("{}: {}×{} cells, layout {}; {w}×{h}: {}", f.name, f.columns, f.rows, if same { "the same" } else { "DIFFERS" }, difference(&f.rgba, &rgba));
    }
    Ok(())
}
