//! A map's most thrown lineups, as the sandbox lists them (`nades_data::lineups`), with each one's example throw:
//!   cargo run --release -p nades-data --example lineups -- <path to a nades database> [de_cache] [count] [nade]
use nades_data::{DataError, Database, Filter, Nade};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("a database path");
    let map = args.next().unwrap_or_else(|| "de_cache".into());
    let count: i64 = args.next().and_then(|n| n.parse().ok()).unwrap_or(5);
    let nade = args.next().as_deref().and_then(Nade::parse);
    let started = std::time::Instant::now();
    let filter = Filter { nade, limit: count, ..Filter::map(&map) };
    match Database::at(path).read(|c| nades_data::lineups(c, &filter)) {
        Ok(rows) => {
            for l in &rows {
                println!("{:>4}  {:<20} {:<7} {:<3} {} -> {} ({})  {}", l.throws, l.lineup_id, l.nade, l.side.as_deref().unwrap_or("?"), l.from_place.as_deref().unwrap_or("?"), l.to_place.as_deref().unwrap_or("?"), l.technique.as_deref().unwrap_or("?"), l.example_throw_id);
            }
            println!("open + query + close: {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
        }
        Err(DataError::Busy) => println!("busy: nades (or another process) holds the database"),
        Err(e) => println!("error: {e}"),
    }
}
