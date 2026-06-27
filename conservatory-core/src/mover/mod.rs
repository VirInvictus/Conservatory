//! Crash-safe file mover (spec §5.4, docs/mover.md, roadmap Phase 2c).
//!
//! Moving the user's files is the headline risk: a move bug damages a real
//! library (CLAUDE.md hard rule). Every relocation is a journaled **job** with a
//! dry-run preview, an undo journal written *before* any file is touched, and
//! crash-safe roll-forward replay on restart. The journal lives in SQLite
//! (migration `0002`, written through the single-writer worker); the file I/O
//! itself runs off the worker.
//!
//! The engine layers are: [`fsops`] (the idempotent per-file primitive),
//! [`journal`] (the DB ledger), and the orchestration in this module
//! ([`plan`] / [`apply`] / [`undo`] / [`recover`]).

pub mod fsops;
pub mod journal;

use std::path::{Path, PathBuf};

use crate::db::{ReadPool, TrackRenderRow, WorkerHandle};
use crate::errors::{Error, Result};
use crate::mover::journal::{JobState, MoveJobRow, OpState};
use crate::path_template::{PathTemplate, TrackFields, find_collisions};

/// Whether a job consumes its sources (`Move`) or leaves them in place (`Copy`).
/// Copy-vs-move is a per-import user choice (spec §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveMode {
    Move,
    Copy,
}

impl MoveMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Copy => "copy",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "move" => Ok(Self::Move),
            "copy" => Ok(Self::Copy),
            other => Err(Error::Move(format!("unknown move mode {other:?}"))),
        }
    }
}

/// One file relocation within a job, built by the caller (the engine renders
/// `dst` via Phase 2a). Paths are absolute for the filesystem; `db_old`/`db_new`
/// are the relative-to-root values stored in the DB, recorded for the update and
/// the undo.
#[derive(Debug, Clone)]
pub struct MoveOp {
    pub track_id: Option<i64>,
    pub album_id: Option<i64>,
    /// The book this moved file belongs to (spec §5.7). On completion every
    /// `book_chapters` row of the book whose `file_path` matches the moved file
    /// is rewritten (one file can back many chapters in a single M4B). A music op
    /// leaves it `None`, and a book op leaves `track_id` / `album_id` `None`.
    pub book_id: Option<i64>,
    pub src: PathBuf,
    pub dst: PathBuf,
    pub db_old: Option<String>,
    pub db_new: Option<String>,
}

/// Build the organize move ops by re-rendering each track's target path from the
/// DB (the `organize` flow, spec §5.1). `albums = None` covers the whole library;
/// `Some(set)` scopes to the given album ids (a path-affecting tag edit). This is
/// the single source of the render-to-`MoveOp` mapping shared by the CLI
/// (`organize`, `tag set`) and the GUI; a new `TrackFields` field is then added
/// in exactly one place.
pub fn organize_ops(rows: &[TrackRenderRow], root: &Path, albums: Option<&[i64]>) -> Vec<MoveOp> {
    let template = PathTemplate::default_music();
    rows.iter()
        .filter(|row| match albums {
            Some(set) => row.album_id.map(|a| set.contains(&a)).unwrap_or(false),
            None => true,
        })
        .map(|row| {
            let fields = TrackFields {
                shelf_genre: row.shelf_genre.as_deref(),
                albumartist: row.album_artist_sort.as_deref(),
                album: row.album.as_deref(),
                year: row.year,
                track_no: row.track_no,
                disc_no: row.disc_no,
                title: Some(row.title.as_str()),
                artist: row.track_artist.as_deref(),
                ext: row.format.as_deref(),
            };
            let rel = template.render(&fields);
            MoveOp {
                track_id: Some(row.track_id),
                album_id: row.album_id,
                book_id: None,
                src: root.join(&row.file_path),
                dst: root.join(&rel),
                db_old: Some(row.file_path.clone()),
                db_new: Some(rel.to_string_lossy().into_owned()),
            }
        })
        .collect()
}

/// Why a job is running: a fresh import, or a re-render of the managed tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveKind {
    Import,
    Organize,
}

impl MoveKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Organize => "organize",
        }
    }
}

/// A reason a planned operation cannot run. A plan with any conflict is refused
/// by [`apply`] (no silent overwrite or auto-rename, the safe default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Two or more operations target the same path. Indices are into the input.
    DuplicateTarget { dst: PathBuf, ops: Vec<usize> },
    /// The source file does not exist.
    MissingSource { src: PathBuf, op: usize },
    /// The destination already exists (refusing to overwrite).
    TargetExists { dst: PathBuf, op: usize },
}

/// The dry-run preview: the operations that would run, the conflicts that block
/// them, and how many no-op (`src == dst`) operations were skipped. Pure: built
/// without touching anything but `stat`.
#[derive(Debug)]
pub struct MovePlan {
    pub ops: Vec<MoveOp>,
    pub conflicts: Vec<Conflict>,
    pub skipped: usize,
}

impl MovePlan {
    pub fn is_blocked(&self) -> bool {
        !self.conflicts.is_empty()
    }
}

/// Build the dry-run plan for a set of operations. No side effects: it reads the
/// filesystem (existence checks) but changes nothing.
pub fn plan(ops: Vec<MoveOp>) -> MovePlan {
    let mut conflicts = Vec::new();

    // Duplicate targets across the batch (reuses the Phase 2a collision finder).
    let dsts: Vec<PathBuf> = ops.iter().map(|o| o.dst.clone()).collect();
    for (dst, idxs) in find_collisions(&dsts) {
        conflicts.push(Conflict::DuplicateTarget { dst, ops: idxs });
    }

    let mut actionable = Vec::new();
    let mut skipped = 0;
    for (i, op) in ops.into_iter().enumerate() {
        if op.src == op.dst {
            skipped += 1;
            continue;
        }
        if !op.src.exists() {
            conflicts.push(Conflict::MissingSource {
                src: op.src.clone(),
                op: i,
            });
            continue;
        }
        if op.dst.exists() {
            conflicts.push(Conflict::TargetExists {
                dst: op.dst.clone(),
                op: i,
            });
            continue;
        }
        actionable.push(op);
    }

    MovePlan {
        ops: actionable,
        conflicts,
        skipped,
    }
}

/// Apply a move job: journal it (durable) **before** any file moves, then run
/// each operation and finalize. Refuses a plan with any conflict, moving nothing.
/// Returns the new job id.
pub async fn apply(
    worker: &WorkerHandle,
    pool: &ReadPool,
    kind: MoveKind,
    mode: MoveMode,
    library_root: &Path,
    created_at: i64,
    ops: Vec<MoveOp>,
) -> Result<i64> {
    let plan = plan(ops);
    if plan.is_blocked() {
        return Err(Error::Move(format!(
            "{} conflict(s); refusing to move (run a dry-run plan to inspect)",
            plan.conflicts.len()
        )));
    }

    let root = library_root.to_string_lossy().into_owned();
    let job_id = worker
        .create_move_job(kind, mode, root, created_at, plan.ops)
        .await?;

    // The journal is now the source of truth for execution; drive it.
    let job = read_job(pool, job_id)?;
    drive_job(worker, pool, &job).await?;
    Ok(job_id)
}

/// Roll-forward recovery: drive every `in_progress` job to completion. Called at
/// startup before normal operation. Idempotent (already-moved files are no-ops).
/// Returns the number of jobs recovered.
pub async fn recover(worker: &WorkerHandle, pool: &ReadPool) -> Result<usize> {
    let jobs = {
        let conn = pool.open()?;
        journal::in_progress_jobs(&conn)?
    };
    let n = jobs.len();
    for job in &jobs {
        drive_job(worker, pool, job).await?;
    }
    Ok(n)
}

/// Undo a job: revert each `done` operation in reverse, then mark the job
/// `undone`. Idempotent.
pub async fn undo(worker: &WorkerHandle, pool: &ReadPool, job_id: i64) -> Result<()> {
    let job = read_job(pool, job_id)?;
    let mode = MoveMode::parse(&job.mode)?;
    let mut ops = {
        let conn = pool.open()?;
        journal::job_operations(&conn, job_id)?
    };
    ops.reverse();

    for op in ops {
        if op.state == OpState::Done {
            fsops::revert(Path::new(&op.src_path), Path::new(&op.dst_path), mode)?;
        }
        worker
            .revert_operation(
                op.id,
                op.track_id,
                op.album_id,
                op.book_id,
                op.db_old_path,
                op.db_new_path,
            )
            .await?;
    }
    worker.set_job_state(job_id, JobState::Undone).await?;
    Ok(())
}

/// Execute a job's still-`pending` operations and finalize it. Shared by
/// [`apply`] and [`recover`]; idempotent per operation.
async fn drive_job(worker: &WorkerHandle, pool: &ReadPool, job: &MoveJobRow) -> Result<()> {
    let mode = MoveMode::parse(&job.mode)?;
    let ops = {
        let conn = pool.open()?;
        journal::job_operations(&conn, job.id)?
    };

    for op in ops {
        if op.state == OpState::Done {
            continue;
        }
        fsops::relocate(Path::new(&op.src_path), Path::new(&op.dst_path), mode)?;
        worker
            .complete_operation(
                op.id,
                op.track_id,
                op.album_id,
                op.book_id,
                op.db_old_path,
                op.db_new_path,
            )
            .await?;
    }

    worker.set_job_state(job.id, JobState::Completed).await?;
    Ok(())
}

fn read_job(pool: &ReadPool, job_id: i64) -> Result<MoveJobRow> {
    let conn = pool.open()?;
    journal::get_job(&conn, job_id)?.ok_or_else(|| Error::Move(format!("no move job {job_id}")))
}
