//! The nades database (show-me-the-nades' `nades.duckdb`), read-only and briefly.
//!
//! `nades` locks its database file for as long as a command runs (an ingest holds it for minutes), and an open
//! connection, read-only included, makes `nades` fail; the reverse holds too. So the sandbox never keeps a connection:
//! every read opens the file read-only, runs its queries and closes it, and a file held by `nades` is `Busy`, which the
//! app answers by trying again later. The queries use the views and table macros `nades` keeps in the file
//! (`throws_in`, `lineup_report`, `throw_scene`, `scene_viewers`, from show-me-the-nades' `sql/views.sql`). The DuckDB
//! here must be the one `nades` writes with (`duckdb ~1.20000.0`), or the file does not open.

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DataError {
    #[error("the database is in use by nades (an ingest?): try again when it finishes")]
    Busy,
    #[error("no database at {0}")]
    Missing(PathBuf),
    #[error("database: {0}")]
    Duck(#[from] duckdb::Error),
}

/// Where the database is; nothing is open between reads.
#[derive(Debug, Clone)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Opens the database read-only, runs `read` and closes it.
    pub fn read<T>(&self, read: impl FnOnce(&duckdb::Connection) -> duckdb::Result<T>) -> Result<T, DataError> {
        if !self.path.exists() {
            return Err(DataError::Missing(self.path.clone()));
        }
        let config = duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly)?;
        let connection = duckdb::Connection::open_with_flags(&self.path, config).map_err(busy_or)?;
        Ok(read(&connection)?)
    }
}

/// DuckDB reports another process's lock on the file as an I/O error that names the lock.
fn busy_or(error: duckdb::Error) -> DataError {
    if error.to_string().contains("lock") { DataError::Busy } else { DataError::Duck(error) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nades-data-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nades.duckdb");
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn reads_rows_and_leaves_nothing_open() {
        let path = scratch("read");
        {
            let writer = duckdb::Connection::open(&path).unwrap();
            writer.execute_batch("create table throws (throw_id varchar, nade varchar); insert into throws values ('a', 'smoke'), ('b', 'flash');").unwrap();
        }
        let db = Database::at(&path);
        let smokes: i64 = db.read(|c| c.query_row("select count(*) from throws where nade = 'smoke'", [], |r| r.get(0))).unwrap();
        assert_eq!(smokes, 1);
        // nothing stays open: a writer gets the file straight away
        let writer = duckdb::Connection::open(&path).unwrap();
        writer.execute_batch("insert into throws values ('c', 'smoke')").unwrap();
    }

    #[test]
    fn a_missing_file_is_named_and_writes_are_refused() {
        assert!(matches!(Database::at("/nonexistent/nades.duckdb").read(|_| Ok(())), Err(DataError::Missing(_))));
        let path = scratch("read-only");
        duckdb::Connection::open(&path).unwrap().execute_batch("create table t (x int)").unwrap();
        assert!(matches!(Database::at(&path).read(|c| c.execute_batch("insert into t values (1)")), Err(DataError::Duck(_))));
    }
}
