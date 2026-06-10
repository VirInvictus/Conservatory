//! Open SQLite connections with the right PRAGMAs (spec §4, docs/schema.md).
//!
//! The writer enables WAL, `synchronous = NORMAL`, foreign keys, an in-memory
//! temp store, and bounds the WAL size and mmap window. Reader connections open
//! `SQLITE_OPEN_READ_ONLY` so a buggy query attempting a write errors at the
//! engine level: no caller can corrupt the library through a read path.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::errors::Result;

/// Cap on the WAL file before SQLite truncates it back down (64 MiB). Keeps a
/// burst of writes from leaving an unbounded `-wal` sidecar on disk.
const JOURNAL_SIZE_LIMIT: i64 = 64 * 1024 * 1024;

/// Bounded memory-map window for the writer (256 MiB). Bounded deliberately:
/// the library can grow large, and an unbounded `mmap_size` would let resident
/// memory track the file size and blow the spec §13 budget.
const MMAP_SIZE: i64 = 256 * 1024 * 1024;

/// How long a reader waits out the brief exclusive window of a WAL checkpoint
/// before failing with `SQLITE_BUSY`.
const READER_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn open_writer(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    apply_writer_pragmas(&conn)?;
    Ok(conn)
}

pub(crate) fn open_reader(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(READER_BUSY_TIMEOUT)?;
    Ok(conn)
}

fn apply_writer_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA temp_store = MEMORY;
         PRAGMA journal_size_limit = {JOURNAL_SIZE_LIMIT};
         PRAGMA mmap_size = {MMAP_SIZE};",
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn writer_applies_pragmas() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let conn = open_writer(&path).unwrap();

        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");

        let foreign_keys: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);

        let synchronous: i64 = conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(synchronous, 1); // 1 == NORMAL

        let journal_size_limit: i64 = conn
            .query_row("PRAGMA journal_size_limit", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal_size_limit, JOURNAL_SIZE_LIMIT);
    }

    #[test]
    fn writer_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested/sub/dir/test.db");
        let _conn = open_writer(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn reader_opens_after_writer_creates_db() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let _writer = open_writer(&path).unwrap();
        let _reader = open_reader(&path).unwrap();
    }

    #[test]
    fn reader_rejects_writes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let _writer = open_writer(&path).unwrap();
        let reader = open_reader(&path).unwrap();
        // Engine-level rejection: no schema needed, the read-only flag alone
        // is what stops the write.
        let result = reader.execute("CREATE TABLE foo (id INTEGER)", []);
        assert!(result.is_err());
    }
}
