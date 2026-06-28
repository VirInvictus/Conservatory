//! Audiobook reorganize (Phase 7b-iii, spec §5.7): re-render an already-imported
//! book's managed folder from its **current** database state and move the book's
//! files there through the core journaled mover — the books analogue of music's
//! `organize`. The caller writes the metadata edits first; this then re-shelves.
//!
//! Two halves, mirroring import's resolve/persist split and music's plan/apply:
//! [`plan_book_reorg`] is a pure read + dry-run (the move preview), and
//! [`apply_book_reorg`] runs the journaled move and follows the cover. The mover
//! needs no audiobook-specific change: a `MoveOp` carrying `book_id` rewrites every
//! `book_chapters` row of the book whose `file_path` matches the moved file and
//! sets `books.folder_path`, the same under `Organize` as under `Import`.
//!
//! Move ops are built **per unique physical file** (a single M4B backs many
//! chapters, so it moves once), exactly like the importer.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::Utc;
use conservatory_core::db::models::{Book, BookChapter, BookPerson, Series};
use conservatory_core::db::{
    ReadPool, WorkerHandle, book_authors, book_chapters, get_book, series_for_book,
};
use conservatory_core::errors::Error as CoreError;
use conservatory_core::mover::{self, Conflict, MoveKind, MoveMode, MoveOp};
use conservatory_core::{BookFields, PathTemplate, sync_album_cover};

use crate::edit::{BookEdit, SeriesEdit};
use crate::error::Result;
use crate::person_sort_name;

/// Apply a [`BookEdit`]'s metadata to the database through the single-writer
/// worker (Phase 7b-iii). Authors / narrators are resolved to person ids
/// (last-name-first sort names) and replace the credited set; a series name is
/// resolved or cleared to standalone; the scalar fields go through `update_book`.
/// This writes metadata only — the caller follows a path-affecting edit with
/// [`plan_book_reorg`] / [`apply_book_reorg`] to re-shelve the files. Shared by
/// the CLI `audiobook set` and the GTK bulk-edit dialog.
pub async fn apply_book_edit(worker: &WorkerHandle, book_id: i64, edit: &BookEdit) -> Result<()> {
    if let Some(names) = &edit.authors {
        let mut ids = Vec::with_capacity(names.len());
        for name in names {
            ids.push(
                worker
                    .get_or_create_book_person(name.clone(), person_sort_name(name))
                    .await?,
            );
        }
        worker.set_book_authors(book_id, ids).await?;
    }
    if let Some(names) = &edit.narrators {
        let mut ids = Vec::with_capacity(names.len());
        for name in names {
            ids.push(
                worker
                    .get_or_create_book_person(name.clone(), person_sort_name(name))
                    .await?,
            );
        }
        worker.set_book_narrators(book_id, ids).await?;
    }
    match &edit.series {
        Some(SeriesEdit::Set(name)) => {
            let series_id = worker.get_or_create_series(name.clone()).await?;
            worker.set_book_series(book_id, Some(series_id)).await?;
        }
        Some(SeriesEdit::Clear) => worker.set_book_series(book_id, None).await?,
        None => {}
    }
    worker
        .update_book(
            book_id,
            edit.title.clone(),
            edit.year,
            edit.series_index,
            edit.shelf_genre.clone(),
            edit.rating,
            edit.starred,
        )
        .await?;
    Ok(())
}

/// The dry-run of a book reorganize: the per-file move ops, any conflicts that
/// would refuse it, and whether the rendered folder already matches (a no-op).
#[derive(Debug)]
pub struct BookReorgPlan {
    /// The operations that would actually move (in-place files are excluded).
    pub ops: Vec<MoveOp>,
    /// Conflicts (an existing destination, a missing source); any refuses the job.
    pub conflicts: Vec<Conflict>,
    /// The rendered target folder, relative to the library root.
    pub new_folder: String,
    /// True when nothing needs to move (the folder is already correct).
    pub unchanged: bool,
}

/// Plan a reorganize: read the book's current state, render its new folder, and
/// build the per-unique-file move ops (a dry-run, no side effects).
pub fn plan_book_reorg(pool: &ReadPool, book_id: i64, root: &Path) -> Result<BookReorgPlan> {
    let conn = pool.open()?;
    let book = get_book(&conn, book_id)?
        .ok_or_else(|| CoreError::Edit(format!("no book with id {book_id}")))?;
    let authors = book_authors(&conn, book_id)?;
    let series = series_for_book(&conn, book_id)?;
    let chapters = book_chapters(&conn, book_id)?;
    drop(conn);

    let (ops_all, new_folder) =
        build_ops(&book, &authors, series.as_ref(), &chapters, root, book_id);
    let plan = mover::plan(ops_all);
    Ok(BookReorgPlan {
        ops: plan.ops,
        conflicts: plan.conflicts,
        new_folder,
        unchanged: book.folder_path == new_folder_relative(&book, &authors, series.as_ref()),
    })
}

/// Apply a reorganize: re-read the (now edited) book, run the journaled move, and
/// follow the cover into the new folder. Returns the move job id, or `None` when
/// nothing moved (the folder was already correct). Heals any interrupted job
/// first, the import / music-organize ordering.
pub async fn apply_book_reorg(
    worker: &WorkerHandle,
    pool: &ReadPool,
    book_id: i64,
    root: &Path,
    mode: MoveMode,
) -> Result<Option<i64>> {
    let (book, authors, series, chapters) = {
        let conn = pool.open()?;
        let book = get_book(&conn, book_id)?
            .ok_or_else(|| CoreError::Edit(format!("no book with id {book_id}")))?;
        let authors = book_authors(&conn, book_id)?;
        let series = series_for_book(&conn, book_id)?;
        let chapters = book_chapters(&conn, book_id)?;
        (book, authors, series, chapters)
    };

    let (ops, new_folder) = build_ops(&book, &authors, series.as_ref(), &chapters, root, book_id);
    let plan = mover::plan(ops.clone());
    // Nothing to move only when the plan is genuinely empty (every op is an
    // in-place skip) *and* clean. A conflict also empties `plan.ops`, so it must
    // not short-circuit here — `mover::apply` re-plans and refuses it.
    if plan.ops.is_empty() && plan.conflicts.is_empty() {
        return Ok(None);
    }

    mover::recover(worker, pool).await?;
    let job_id = mover::apply(
        worker,
        pool,
        MoveKind::Organize,
        mode,
        root,
        Utc::now().timestamp(),
        ops,
    )
    .await?;

    follow_cover(worker, &book, &new_folder, root).await;
    Ok(Some(job_id))
}

/// Move the book's `cover.jpg` into the new folder and update `books.cover_path`
/// (best-effort: covers re-derive, and the accent already lives on the book row,
/// so a failure here never fails the move).
async fn follow_cover(worker: &WorkerHandle, book: &Book, new_folder: &str, root: &Path) {
    let Some(old_rel) = book.cover_path.as_deref() else {
        return;
    };
    let bytes = match std::fs::read(root.join(old_rel)) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(book_id = book.id, error = %e, "book cover not found to follow the move");
            return;
        }
    };
    match sync_album_cover(root, new_folder, &bytes, Some(old_rel)) {
        Ok(new_cover) => {
            if let Err(e) = worker
                .set_book_cover_path(book.id, Some(new_cover), None)
                .await
            {
                tracing::warn!(book_id = book.id, error = %e, "book cover path not updated after move");
            }
        }
        Err(e) => {
            tracing::warn!(book_id = book.id, error = %e, "book cover not written to new folder")
        }
    }
}

/// Build the per-unique-file move ops + the rendered folder string. The author
/// component is the first author by sort name (the `book_authors` order); a single
/// M4B that backs many chapters collapses to one op.
fn build_ops(
    book: &Book,
    authors: &[BookPerson],
    series: Option<&Series>,
    chapters: &[BookChapter],
    root: &Path,
    book_id: i64,
) -> (Vec<MoveOp>, String) {
    let folder_rel = render_folder(book, authors, series);
    let folder_str = folder_rel.to_string_lossy().into_owned();

    let mut seen = HashSet::new();
    let mut ops = Vec::new();
    for ch in chapters {
        if !seen.insert(ch.file_path.clone()) {
            continue;
        }
        let name = Path::new(&ch.file_path)
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        let new_rel = folder_rel.join(&name).to_string_lossy().into_owned();
        ops.push(MoveOp {
            track_id: None,
            album_id: None,
            book_id: Some(book_id),
            src: root.join(&ch.file_path),
            dst: root.join(&new_rel),
            db_old: Some(ch.file_path.clone()),
            db_new: Some(new_rel),
        });
    }
    (ops, folder_str)
}

/// Render the book's managed folder from its current DB fields.
fn render_folder(book: &Book, authors: &[BookPerson], series: Option<&Series>) -> PathBuf {
    let fields = BookFields {
        shelf_genre: None,
        author: authors.first().map(|p| p.sort_name.as_str()),
        narrator: None,
        series: series.map(|s| s.name.as_str()),
        series_index: book.series_sequence,
        title: Some(book.title.as_str()),
        year: book.year,
    };
    PathTemplate::default_audiobook().render_book(&fields)
}

/// The rendered folder as a string (the `unchanged` comparison key).
fn new_folder_relative(book: &Book, authors: &[BookPerson], series: Option<&Series>) -> String {
    render_folder(book, authors, series)
        .to_string_lossy()
        .into_owned()
}
