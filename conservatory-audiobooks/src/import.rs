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
//! clear, so a conflicting import leaves the database untouched. The one window
//! the two passes cannot close is a conflict that appears *between* the
//! pre-check and `apply`'s own re-plan; there the persist pass rolls the book
//! rows back, so the guarantee holds against the race too.
//!
//! One physical file can back many chapters (a single M4B), so move ops are
//! built **per unique source file**, not per chapter: each op carries the
//! `book_id`, and the mover rewrites every chapter of the book whose `file_path`
//! matches the moved file (spec §5.7, migration 0012). Scope is **one book per
//! call** for [`import_book`] (a folder or a single `.m4b`);
//! [`import_book_tree`] walks a directory tree and imports every book folder
//! it discovers, one single-book pipeline per root (the 2026-08-23 sweep).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::Utc;
use conservatory_core::db::models::{Book, BookChapter};
use conservatory_core::db::{ReadPool, WorkerHandle};
use conservatory_core::errors::Error;
use conservatory_core::mover::{self, Conflict, MoveKind, MoveMode, MoveOp};
use conservatory_core::{BookFields, CoverSource, PathTemplate, compute_accent, sync_album_cover};

use crate::error::{ReadError, Result};
use crate::{discover_book_roots, read_book};

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
    /// Non-empty means the import was refused: either nothing was created (the
    /// pre-check caught it) or the rows were rolled back (a conflict appeared
    /// between the pre-check and apply). Either way no rows remain.
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

/// Import a source that may hold many books (the 2026-08-23 sweep's recursive
/// importer). A single file imports as its one book; a folder imports as one
/// book when it holds audio directly; a directory *tree* imports every
/// [`discover_book_roots`] folder under it (an `Author/` shelf lands as many
/// books). Each book rides the normal single-book pipeline, so its report
/// carries its own conflicts (a conflicting book imports nothing; the other
/// books still import), and the walk stops at the first book the reader
/// cannot read (books before it stay imported; the error names the folder).
pub async fn import_book_tree(
    worker: &WorkerHandle,
    pool: &ReadPool,
    source: &Path,
    opts: &BookImportOptions,
) -> Result<Vec<BookImportReport>> {
    let roots = if source.is_dir() {
        let roots = discover_book_roots(source)?;
        // A folder that holds audio directly (or is empty of audio) is the
        // single-book call this function wraps; only a multi-book tree splits.
        if roots.len() <= 1 {
            vec![source.to_path_buf()]
        } else {
            roots
        }
    } else {
        vec![source.to_path_buf()]
    };

    let mut reports = Vec::with_capacity(roots.len());
    for root in &roots {
        reports.push(import_book(worker, pool, root, opts).await?);
    }
    Ok(reports)
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
    let accent = draft
        .cover
        .as_ref()
        .and_then(|c| compute_accent(c.bytes()).ok());

    let files = plan_file_moves(&draft, &folder_rel, &book_dir_abs);

    // The cover's provenance decides the move-mode shape (the music import's
    // `CoverSource` split): a sidecar rides the journal — consumed with the
    // audio, so the source folder is left clean, a crash rolls forward, and
    // undo restores it — while embedded art has no file to consume and writes
    // the canonical copy after the move. Copy mode never claims the sidecar.
    let claimed_sidecar = match (&draft.cover, opts.mode) {
        (Some(CoverSource::Sidecar { path, .. }), MoveMode::Move) => Some(path.clone()),
        _ => None,
    };
    let cover_rel = claimed_sidecar.as_ref().and_then(|p| {
        p.file_name()
            .map(|name| folder_rel.join(name))
            .map(|rel| rel.to_string_lossy().into_owned())
    });

    // Heal any interrupted job BEFORE planning: recovery rolls interrupted
    // moves forward, so the pre-check and the apply must both see the
    // post-recovery tree. It also runs before any DB write, so a recovery
    // failure can no longer leave a half-persisted book behind (the
    // partial-commit window the final audit flagged: recovery used to run
    // after the rows were written, and apply's re-plan could still refuse).
    mover::recover(worker, pool).await?;

    // Pre-check the move before any DB write: a folder-exists, duplicate-target,
    // or vanished-source conflict refuses the whole import (the trust guarantee,
    // spec §5.4). The claimed sidecar rides the same list, so a cover collision
    // refuses before any row exists.
    let mut provisional = provisional_ops(&files);
    if let (Some(src), Some(rel)) = (&claimed_sidecar, &cover_rel) {
        provisional.push(MoveOp {
            track_id: None,
            album_id: None,
            book_id: None,
            src: src.clone(),
            dst: opts.library_root.join(rel),
            db_old: None,
            db_new: Some(rel.clone()),
        });
    }
    let pre = mover::plan(provisional);
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

    let mut ops: Vec<MoveOp> = files
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
    // The claimed sidecar rides the same journal (one cover-shaped op: `book_id`
    // set, no `track_id`/`album_id`). No managed cover existed before the
    // import, so `db_old` is `None`: the post-move write below records the
    // pointer, and undo clears it.
    if let (Some(src), Some(rel)) = (&claimed_sidecar, &cover_rel) {
        ops.push(MoveOp {
            track_id: None,
            album_id: None,
            book_id: Some(book_id),
            src: src.clone(),
            dst: opts.library_root.join(rel),
            db_old: None,
            db_new: Some(rel.clone()),
        });
    }
    let job_id = match apply_import_job(worker, pool, book_id, ops, opts, now.timestamp()).await? {
        Applied::Job(job_id) => job_id,
        Applied::Refused(conflicts) => {
            return Ok(BookImportReport {
                title: draft.title.clone(),
                conflicts,
                ..Default::default()
            });
        }
    };

    // Cover bookkeeping (the move created the book folder). A claimed sidecar
    // already sits in the book folder (the move moved it): point the book at
    // the moved file instead of writing a second, canonical copy — undo then
    // clears the pointer and restores the file. Embedded art, and a copy-mode
    // import whose sidecar stayed at the source, still write the canonical
    // copy. Best-effort: a cover failure never fails an otherwise-successful
    // import (covers re-derive), but a DB-write failure means the worker is
    // wedged, so surface it. The accent is already on the book row, so the
    // cover write keeps it (`None`).
    if let Some(rel) = &cover_rel {
        if let Err(e) = worker
            .set_book_cover_path(book_id, Some(rel.clone()), None)
            .await
        {
            tracing::warn!(book_id, error = %e, "book cover path not recorded");
        }
    } else if let Some(bytes) = draft.cover.as_ref().map(|c| c.bytes())
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
        files: files.len() + usize::from(claimed_sidecar.is_some()),
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

/// What running the import's move job did.
enum Applied {
    /// The job was journaled and is running (or completed).
    Job(i64),
    /// `apply`'s own re-plan refused: a conflict appeared after the pre-check,
    /// with the book rows already written. [`apply_import_job`] rolled them
    /// back, so the refusal leaves nothing behind.
    Refused(Vec<Conflict>),
}

/// Run the import's move job, rolling the freshly-written book rows back if
/// `apply`'s re-plan refuses. The pre-check twin ran before the rows were
/// written, so a refusal here can only be a conflict that appeared in between;
/// the module contract ("a conflicting import leaves the database untouched")
/// means those rows come back out. The delete cascades the chapters and the
/// author/narrator links, and no file was touched because a refusal happens
/// before the job is journaled.
async fn apply_import_job(
    worker: &WorkerHandle,
    pool: &ReadPool,
    book_id: i64,
    ops: Vec<MoveOp>,
    opts: &BookImportOptions,
    created_at: i64,
) -> Result<Applied> {
    match mover::apply(
        worker,
        pool,
        MoveKind::Import,
        opts.mode,
        &opts.library_root,
        created_at,
        ops,
    )
    .await
    {
        Ok(job_id) => Ok(Applied::Job(job_id)),
        Err(Error::MoveRefused(conflicts)) => {
            worker.delete_books(vec![book_id]).await?;
            Ok(Applied::Refused(conflicts))
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conservatory_core::db::models::Book;
    use conservatory_core::db::{list_books, spawn_worker};
    use tempfile::tempdir;

    #[tokio::test]
    async fn an_apply_refusal_after_the_rows_rolls_the_book_back() {
        // The partial-commit window (the final audit): the pre-check ran
        // before the rows were written, so a conflict that appears in between
        // refuses in `apply`, with the book already persisted. The rollback
        // must leave the database exactly as it was and touch no file.
        let dir = tempdir().unwrap();
        let db = dir.path().join("lib.db");
        let worker = spawn_worker(db.clone()).unwrap();
        let pool = ReadPool::new(db, 1).unwrap();

        let book_id = worker
            .insert_book(Book {
                id: 0,
                title: "Race Loser".into(),
                subtitle: None,
                series_id: None,
                series_sequence: None,
                year: Some(2021),
                publisher: None,
                isbn: None,
                asin: None,
                description: None,
                language: None,
                shelf_genre: None,
                cover_path: None,
                accent_rgb: None,
                folder_path: "Audiobooks/Author, Test/Standalone/Race Loser (2021)".into(),
                rating: 0,
                starred: false,
                added_at: None,
            })
            .await
            .unwrap();

        // The destination appears after any pre-check would have run.
        let src = dir.path().join("src.m4b");
        std::fs::write(&src, b"audio").unwrap();
        let root = dir.path().join("lib");
        let dst = root.join("book.m4b");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&dst, b"someone else got here first").unwrap();

        let opts = BookImportOptions {
            library_root: root,
            mode: MoveMode::Move,
        };
        let applied = apply_import_job(
            &worker,
            &pool,
            book_id,
            vec![MoveOp {
                track_id: None,
                album_id: None,
                book_id: Some(book_id),
                src: src.clone(),
                dst: dst.clone(),
                db_old: Some("source.m4b".into()),
                db_new: Some("book.m4b".into()),
            }],
            &opts,
            0,
        )
        .await
        .unwrap();

        assert!(
            matches!(applied, Applied::Refused(_)),
            "the conflicting apply must refuse"
        );
        let conn = pool.open().unwrap();
        assert!(
            list_books(&conn).unwrap().is_empty(),
            "the book rows were rolled back"
        );
        drop(conn);
        // Nothing was consumed and nothing was clobbered: the source survives
        // and the foreign destination is exactly as the interloper left it.
        assert_eq!(std::fs::read(&src).unwrap(), b"audio");
        assert_eq!(std::fs::read(&dst).unwrap(), b"someone else got here first");

        worker.shutdown_ack().await.unwrap();
    }
}
