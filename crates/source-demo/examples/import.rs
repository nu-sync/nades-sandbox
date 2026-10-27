//! Imports demos into a cache folder and says how long each took.
//!
//!   cargo run --release -p nades-source-demo --example import -- <cache folder> <demo or folder>...

use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let cache = args.next().expect("usage: import <cache folder> <demo or folder>...");
    for path in args {
        for demo in nades_source_demo::demos_in(&path)? {
            let started = Instant::now();
            let done = nades_source_demo::import(&demo, &cache, &mut |_| {})?;
            println!("{}: {} throws, {} in {:.1} s -> {}", demo.display(), done.throws, if done.parsed { "parsed" } else { "cached" }, started.elapsed().as_secs_f64(), done.file.display());
        }
    }
    Ok(())
}
