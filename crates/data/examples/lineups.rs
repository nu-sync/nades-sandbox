//! The most thrown lineups of a map, through nades' own `lineup_report` macro:
//!   cargo run --release -p nades-data --example lineups -- <path to a nades database> [de_cache]
use nades_data::{DataError, Database};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("a database path");
    let map = args.next().unwrap_or_else(|| "de_cache".into());
    let started = std::time::Instant::now();
    let rows = Database::at(path).read(|c| {
        let mut query = c.prepare("select lineup_id, nade, side, from_place, to_place, technique, throws from lineup_report(p_map := ?) order by throws desc limit 5")?;
        query
            .query_map([&map], |r| Ok(format!("{:>4}  {:<20} {:<7} {:<3} {} -> {} ({})", r.get::<_, i64>(6)?, r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?)))?
            .collect::<duckdb::Result<Vec<_>>>()
    });
    match rows {
        Ok(rows) => {
            for row in rows {
                println!("{row}");
            }
            println!("open + query + close: {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
        }
        Err(DataError::Busy) => println!("busy: nades (or another process) holds the database"),
        Err(e) => println!("error: {e}"),
    }
}
