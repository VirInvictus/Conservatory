//! Audiobook import orchestration (Phase 7a-iii, spec §5.4, §5.7).
//!
//! The plugin counterpart to `conservatory-core`'s music import: it resolves a
//! [`BookDraft`] (the 7a-ii reader's output) into `books` / `book_people` /
//! `series` / `book_chapters` rows and **moves the book's files into the managed
//! tree** through the core file mover, then writes the cover. The path template,
//! the journaled mover, and the cover writer are all core (this is the §2.2
//! boundary: audiobook *logic* is plugin code calling core machinery).
//!
//! Two passes, the shape of the music importer: a pure **resolve** pass renders
//! the book folder and pre-checks for move conflicts (no DB writes), then a
//! **persist** pass creates the rows and runs the move job only if the plan is
//! clear, so a conflicting import leaves the database untouched.
//!
//! One physical file can back many chapters (a single M4B), so move ops are
//! built **per unique source file**, not per chapter: each op carries the
//! `book_id`, and the mover rewrites every chapter of the book whose `file_path`
//! matches the moved file (spec §5.7, migration 0012). Scope is **one book per
//! call** (a folder or a single `.m4b`); a whole-`Author/*`-tree batch is a
//! later add.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::Utc;
use conservatory_core::db::models::{Book, BookChapter};
use conservatory_core::db::{ReadPool, WorkerHandle};
use conservatory_core::mover::{self, Conflict, MoveKind, MoveMode, MoveOp};
use conservatory_core::{BookFields, PathTemplate, compute_accent, sync_album_cover};

use crate::error::{ReadError, Result};
use crate::read_book;

/// How a book import runs (the audiobook analogue of core's `ImportOptions`).
#[derive(Debug, Clone)]
pub struct BookImportOptions {
    /// The managed library root the rendered `Audiobooks/` tree hangs off.
    pub library_root: PathBuf,
    /// Copy (leave originals) or move (consume them). The CLI defaults to copy.
    pub mode: MoveMode,
}

/// What an import did (or, when blocked, why it did nothing).
#[derive(Debug, Default)]
pub struct BookImportReport {
    pub title: Option<String>,
    pub authors: usize,
    pub narrators: usize,
    pub chapters: usize,
    /// The number of physical files moved (one per chapter file, or one for a
    /// single M4B that backs every chapter).
    pub files: usize,
    pub book_id: Option<i64>,
    pub job_id: Option<i64>,
    /// Non-empty means the import was refused; no rows were created.
    pub conflicts: Vec<Conflict>,
}

/// One physical source file and where it lands. `db_old` is the source path the
/// chapter rows are first written with; `db_new` is the rendered managed path.
struct FileMove {
    src: PathBuf,
    dst: PathBuf,
    db_old: String,
    db_new: String,
}

/// Import a single book (a folder or a single audio file) into the library.
pub async fn import_book(
    worker: &WorkerHandle,
    pool: &ReadPool,
    source: &Path,
    opts: &BookImportOptions,
) -> Result<BookImportReport> {
    let draft = read_book(source)?;
    if draft.chapters.is_empty() {
        return Err(ReadError::NoAudio(source.display().to_string()));
    }

    // --- Resolve pass (pure): render the book folder + the per-file move list ---
    let folder_rel = render_book_folder(&draft);
    let folder_rel_str = folder_rel.to_string_lossy().into_owned();
    let book_dir_abs = opts.library_root.join(&folder_rel);
    let accent = draft.cover.as_deref().and_then(|b| compute_accent(b).ok());

    let files = plan_file_moves(&draft, &folder_rel, &book_dir_abs);

    // Pre-check the move before any DB write: a folder-exists or duplicate-target
    // conflict refuses the whole import (the trust guarantee, spec §5.4).
    let pre = mover::plan(provisional_ops(&files));
    if pre.is_blocked() {
        return Ok(BookImportReport {
            title: draft.title.clone(),
            conflicts: pre.conflicts,
            ..Default::default()
        });
    }

    // --- Persist pass (rows, then move) ---
    let now = Utc::now();

    let mut author_ids = Vec::with_capacity(draft.authors.len());
    for p in &draft.authors {
        author_ids.push(
            worker
                .get_or_create_book_person(p.name.clone(), p.sort_name.clone())
                .await?,
        );
    }
    let mut narrator_ids = Vec::with_capacity(draft.narrators.len());
    for p in &draft.narrators {
        narrator_ids.push(
            worker
                .get_or_create_book_person(p.name.clone(), p.sort_name.clone())
                .await?,
        );
    }
    let series_id = match &draft.series {
        Some(name) => Some(worker.get_or_create_series(name.clone()).await?),
        None => None,
    };

    let book = Book {
        id: 0,
        title: draft.title.clone().unwrap_or_else(|| "Untitled".into()),
        subtitle: draft.subtitle.clone(),
        series_id,
        series_sequence: draft.series_sequence,
        year: draft.year,
        publisher: draft.publisher.clone(),
        isbn: draft.isbn.clone(),
        asin: draft.asin.clone(),
        description: draft.description.clone(),
        language: draft.language.clone(),
        shelf_genre: None,
        cover_path: None,
        accent_rgb: accent,
        folder_path: folder_rel_str.clone(),
        rating: 0,
        starred: false,
        added_at: Some(now),
    };
    let book_id = worker.insert_book(book).await?;
    for id in &author_ids {
        worker.link_book_author(book_id, *id).await?;
    }
    for id in &narrator_ids {
        worker.link_book_narrator(book_id, *id).await?;
    }

    // Chapters are written with their *source* file paths; the mover flips each
    // to the managed path on completion (matching by book_id + source path, so a
    // single M4B's chapters all follow the one moved file).
    let chapters: Vec<BookChapter> = draft
        .chapters
        .iter()
        .map(|ch| BookChapter {
            id: 0,
            book_id,
            idx: ch.idx,
            title: ch.title.clone(),
            file_path: ch.file_path.to_string_lossy().into_owned(),
            file_offset: ch.file_offset,
            duration: ch.duration,
        })
        .collect();
    worker.replace_book_chapters(book_id, chapters).await?;

    // Heal any interrupted job, then run this one (the music-import ordering).
    mover::recover(worker, pool).await?;
    let ops = files
        .iter()
        .map(|f| MoveOp {
            track_id: None,
            album_id: None,
            book_id: Some(book_id),
            src: f.src.clone(),
            dst: f.dst.clone(),
            db_old: Some(f.db_old.clone()),
            db_new: Some(f.db_new.clone()),
        })
        .collect();
    let job_id = mover::apply(
        worker,
        pool,
        MoveKind::Import,
        opts.mode,
        &opts.library_root,
        now.timestamp(),
        ops,
    )
    .await?;

    // Cover to disk (the move created the book folder). Best-effort: a cover
    // failure never fails an otherwise-successful import (covers re-derive), but
    // a DB-write failure means the worker is wedged, so surface it. The accent is
    // already on the book row, so the cover write keeps it (`None`).
    if let Some(bytes) = &draft.cover
        && let Ok(cover_path) = sync_album_cover(&opts.library_root, &folder_rel_str, bytes, None)
        && let Err(e) = worker
            .set_book_cover_path(book_id, Some(cover_path), None)
            .await
    {
        tracing::warn!(book_id, error = %e, "book cover path not recorded");
    }

    Ok(BookImportReport {
        title: draft.title.clone(),
        authors: author_ids.len(),
        narrators: narrator_ids.len(),
        chapters: draft.chapters.len(),
        files: files.len(),
        book_id: Some(book_id),
        job_id: Some(job_id),
        conflicts: Vec::new(),
    })
}

/// Render the book's managed folder (relative to the root) from the default
/// audiobook template. The author is the first credited author's sort name; a
/// standalone book renders under the literal `Standalone` (spec §5.7).
fn render_book_folder(draft: &crate::BookDraft) -> PathBuf {
    let fields = BookFields {
        shelf_genre: None,
        author: draft.authors.first().map(|p| p.sort_name.as_str()),
        narrator: draft.narrators.first().map(|p| p.sort_name.as_str()),
        series: draft.series.as_deref(),
        series_index: draft.series_sequence,
        title: draft.title.as_deref(),
        year: draft.year,
    };
    PathTemplate::default_audiobook().render_book(&fields)
}

/// Build the per-unique-source-file move list. Chapters that share one physical
/// file (a single M4B) collapse to a single move; the destination keeps the
/// source filename inside the rendered book folder.
fn plan_file_moves(
    draft: &crate::BookDraft,
    folder_rel: &Path,
    book_dir_abs: &Path,
) -> Vec<FileMove> {
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for ch in &draft.chapters {
        let db_old = ch.file_path.to_string_lossy().into_owned();
        if !seen.insert(db_old.clone()) {
            continue;
        }
        let name = ch
            .file_path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        files.push(FileMove {
            src: ch.file_path.clone(),
            dst: book_dir_abs.join(&name),
            db_old,
            db_new: folder_rel.join(&name).to_string_lossy().into_owned(),
        });
    }
    files
}

/// The conflict-check ops (no `book_id` needed; `plan` only stats the paths).
fn provisional_ops(files: &[FileMove]) -> Vec<MoveOp> {
    files
        .iter()
        .map(|f| MoveOp {
            track_id: None,
            album_id: None,
            book_id: None,
            src: f.src.clone(),
            dst: f.dst.clone(),
            db_old: Some(f.db_old.clone()),
            db_new: Some(f.db_new.clone()),
        })
        .collect()
}
