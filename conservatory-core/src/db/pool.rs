//! Read-only connection pool (spec §2.1).
//!
//! Distinct from the writer task: read commands open `SQLITE_OPEN_READ_ONLY`
//! handles so a timeline read, search, or count never queues behind a long
//! write. Phase 1a ships a "pool" that opens a fresh handle on every `open()`;
//! the structure is in place for a bounded handle ring once profiling shows the
//! per-call open overhead matters (the Belfry/Viaduct staging).

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::db::connection;
use crate::errors::Result;

/// Cheap to clone: backed by a `PathBuf`.
#[derive(Clone)]
pub struct ReadPool {
    path: PathBuf,
}

impl ReadPool {
    /// Construct a pool. `_capacity` is reserved for a future bounded handle
    /// ring; Phase 1a ignores it and opens fresh connections per call. Must be
    /// called after the writer has created the file, so the read-only open
    /// finds an existing, WAL-configured database.
    pub fn new(path: PathBuf, _capacity: usize) -> Result<Self> {
        // Smoke-test: confirm we can open a reader against the file.
        let _conn = connection::open_reader(&path)?;
        Ok(Self { path })
    }

    /// The database file this pool reads from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Open a read-only connection.
    ///
    /// Phase 1a opens a fresh handle per call; post-1.0 this returns a pooled
    /// handle from a bounded ring sized via the `new` capacity argument.
    pub fn open(&self) -> Result<Connection> {
        connection::open_reader(&self.path)
    }
}
