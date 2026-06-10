//! Debug-only round-trip helper for the Phase 1a usable artifact.
//!
//! The writer creates the `_probe` table on demand and upserts a key/value;
//! a read-pool connection reads it back. This exercises the full
//! writer -> file -> read-only-pool path before any real schema exists. It is
//! scaffolding: the underscore-prefixed table is ignored by the real
//! migrations, and this whole module is replaced by genuine CRUD in Phase 1b.

use rusqlite::Connection;

use crate::db::pool::ReadPool;
use crate::errors::Result;

/// Write a key/value through the writer connection, creating the probe table
/// if it does not yet exist. Runs on the worker thread.
pub(crate) fn write(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS _probe (k TEXT PRIMARY KEY, v TEXT NOT NULL);")?;
    conn.execute(
        "INSERT INTO _probe (k, v) VALUES (?1, ?2)
         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        (key, value),
    )?;
    Ok(())
}

/// Read a probe value back through a read-only pool connection. Returns `None`
/// if the key (or the table) is absent.
pub fn probe_read(pool: &ReadPool, key: &str) -> Result<Option<String>> {
    let conn = pool.open()?;

    // The reader is read-only, so it cannot create the table; if the writer
    // hasn't run yet the table is missing and the lookup is simply empty.
    let table_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_probe'",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !table_exists {
        return Ok(None);
    }

    let value = conn
        .query_row("SELECT v FROM _probe WHERE k = ?1", [key], |r| r.get(0))
        .ok();
    Ok(value)
}
