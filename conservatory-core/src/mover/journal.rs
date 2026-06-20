//! The move journal: the SQLite ledger behind the crash-safe mover (spec §5.4).
//!
//! Writes (`create_job`, `complete_operation`, `revert_operation`,
//! `set_job_state`) run on the single writer via the worker; reads
//! (`in_progress_jobs`, `job_operations`, `get_job`) run on the read pool. Each
//! multi-statement write is one transaction so the journal row and the DB path
//! update it implies never diverge (docs/mover.md).

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::errors::{Error, Result};
use crate::mover::{MoveKind, MoveMode, MoveOp};

/// Lifecycle of a move job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    InProgress,
    Completed,
    Undone,
    Failed,
}

impl JobState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Undone => "undone",
            Self::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "undone" => Self::Undone,
            "failed" => Self::Failed,
            other => return Err(Error::Move(format!("unknown job state {other:?}"))),
        })
    }
}

/// Whether an operation has been applied to disk yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpState {
    Pending,
    Done,
}

impl OpState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Done => "done",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "pending" => Self::Pending,
            "done" => Self::Done,
            other => return Err(Error::Move(format!("unknown op state {other:?}"))),
        })
    }
}

/// A `move_jobs` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveJobRow {
    pub id: i64,
    pub kind: String,
    pub mode: String,
    pub library_root: String,
    pub state: JobState,
    pub created_at: i64,
}

/// A `move_operations` row, in `seq` order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveOpRow {
    pub id: i64,
    pub seq: i64,
    pub track_id: Option<i64>,
    pub album_id: Option<i64>,
    pub src_path: String,
    pub dst_path: String,
    pub db_old_path: Option<String>,
    pub db_new_path: Option<String>,
    pub state: OpState,
}

// --- Writes (single writer, via the worker) ---

/// Insert a job and all its operations as `pending`, atomically. This is the
/// durable record written *before* any file is touched.
pub(crate) fn create_job(
    conn: &mut Connection,
    kind: MoveKind,
    mode: MoveMode,
    library_root: &str,
    created_at: i64,
    ops: &[MoveOp],
) -> Result<i64> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO move_jobs (kind, mode, library_root, state, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            kind.as_str(),
            mode.as_str(),
            library_root,
            JobState::InProgress.as_str(),
            created_at,
        ],
    )?;
    let job_id = tx.last_insert_rowid();
    for (i, op) in ops.iter().enumerate() {
        let src = op.src.to_string_lossy();
        let dst = op.dst.to_string_lossy();
        tx.execute(
            "INSERT INTO move_operations
                (job_id, seq, track_id, album_id, src_path, dst_path, db_old_path, db_new_path, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                job_id,
                i as i64,
                op.track_id,
                op.album_id,
                src.as_ref(),
                dst.as_ref(),
                op.db_old,
                op.db_new,
                OpState::Pending.as_str(),
            ],
        )?;
    }
    tx.commit()?;
    Ok(job_id)
}

/// Mark an operation `done` and apply the DB path it implies (the track's
/// `file_path` and the album's `folder_path`), in one transaction.
pub(crate) fn complete_operation(
    conn: &mut Connection,
    op_id: i64,
    track_id: Option<i64>,
    album_id: Option<i64>,
    db_new_path: Option<&str>,
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE move_operations SET state = ?2 WHERE id = ?1",
        params![op_id, OpState::Done.as_str()],
    )?;
    apply_db_path(&tx, track_id, album_id, db_new_path)?;
    tx.commit()?;
    Ok(())
}

/// Restore the pre-move DB path and reset the operation to `pending` (so undo is
/// itself replayable), in one transaction.
pub(crate) fn revert_operation(
    conn: &mut Connection,
    op_id: i64,
    track_id: Option<i64>,
    album_id: Option<i64>,
    db_old_path: Option<&str>,
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE move_operations SET state = ?2 WHERE id = ?1",
        params![op_id, OpState::Pending.as_str()],
    )?;
    apply_db_path(&tx, track_id, album_id, db_old_path)?;
    tx.commit()?;
    Ok(())
}

/// Point the track's `file_path` and the album's `folder_path` at `path` (the
/// album folder is the file's parent). A `None` path leaves the rows untouched.
fn apply_db_path(
    tx: &Connection,
    track_id: Option<i64>,
    album_id: Option<i64>,
    path: Option<&str>,
) -> Result<()> {
    let Some(path) = path else { return Ok(()) };
    if let Some(track_id) = track_id {
        tx.execute(
            "UPDATE tracks SET file_path = ?2 WHERE id = ?1",
            params![track_id, path],
        )?;
    }
    if let Some(album_id) = album_id {
        let folder = Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        tx.execute(
            "UPDATE albums SET folder_path = ?2 WHERE id = ?1",
            params![album_id, folder],
        )?;
    }
    Ok(())
}

pub(crate) fn set_job_state(conn: &Connection, job_id: i64, state: JobState) -> Result<()> {
    conn.execute(
        "UPDATE move_jobs SET state = ?2 WHERE id = ?1",
        params![job_id, state.as_str()],
    )?;
    Ok(())
}

// --- Reads (read pool) ---

/// All jobs still `in_progress` (interrupted by a crash), oldest first.
pub fn in_progress_jobs(conn: &Connection) -> Result<Vec<MoveJobRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, mode, library_root, state, created_at
         FROM move_jobs WHERE state = 'in_progress' ORDER BY id",
    )?;
    let rows = stmt.query_map([], row_to_job)?;
    rows.map(|r| r?).collect()
}

pub fn get_job(conn: &Connection, job_id: i64) -> Result<Option<MoveJobRow>> {
    conn.query_row(
        "SELECT id, kind, mode, library_root, state, created_at FROM move_jobs WHERE id = ?1",
        params![job_id],
        row_to_job,
    )
    .optional()?
    .transpose()
}

/// A job's operations in `seq` order.
pub fn job_operations(conn: &Connection, job_id: i64) -> Result<Vec<MoveOpRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, seq, track_id, album_id, src_path, dst_path, db_old_path, db_new_path, state
         FROM move_operations WHERE job_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![job_id], row_to_op)?;
    rows.map(|r| r?).collect()
}

// The row mappers return `rusqlite::Result<Result<Row>>`: the outer is a query
// error, the inner an enum-parse error. The callers flatten with `r?`.
fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<MoveJobRow>> {
    let state: String = row.get("state")?;
    Ok((|| {
        Ok(MoveJobRow {
            id: row.get("id")?,
            kind: row.get("kind")?,
            mode: row.get("mode")?,
            library_root: row.get("library_root")?,
            state: JobState::parse(&state)?,
            created_at: row.get("created_at")?,
        })
    })())
}

fn row_to_op(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<MoveOpRow>> {
    let state: String = row.get("state")?;
    Ok((|| {
        Ok(MoveOpRow {
            id: row.get("id")?,
            seq: row.get("seq")?,
            track_id: row.get("track_id")?,
            album_id: row.get("album_id")?,
            src_path: row.get("src_path")?,
            dst_path: row.get("dst_path")?,
            db_old_path: row.get("db_old_path")?,
            db_new_path: row.get("db_new_path")?,
            state: OpState::parse(&state)?,
        })
    })())
}
