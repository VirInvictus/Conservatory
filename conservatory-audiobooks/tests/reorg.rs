//! Phase 7b-iii integration suite for the book reorganize path (spec §5.7): a
//! path-affecting edit re-renders the folder and re-shelves the book's files
//! through the journaled mover. Books are owned like music, so the same trust
//! guarantees apply (the move is the risk): a multi-file move, the single-M4B
//! case where one moved file backs many chapters, undo, an in-place no-op, and a
//! conflict refusal are all checked end to end against real files on disk.

use std::fs;
use std::path::Path;

use conservatory_audiobooks::edit::BookEdit;
use conservatory_audiobooks::{apply_book_edit, apply_book_reorg, plan_book_reorg};
use conservatory_core::db::{
    Book, BookChapter, ReadPool, WorkerHandle, book_chapters, get_book, spawn_worker,
};
use conservatory_core::mover::{self, MoveMode};
use tempfile::{TempDir, tempdir};

struct Fixture {
    _libdir: TempDir,
    _dbdir: TempDir,
    root: std::path::PathBuf,
    worker: WorkerHandle,
    pool: ReadPool,
}

async fn fixture() -> Fixture {
    let libdir = tempdir().unwrap();
    let dbdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db, 3).unwrap();
    Fixture {
        root: libdir.path().to_path_buf(),
        _libdir: libdir,
        _dbdir: dbdir,
        worker,
        pool,
    }
}

fn book_row(title: &str, folder: &str, series_id: Option<i64>, seq: Option<f64>) -> Book {
    Book {
        id: 0,
        title: title.into(),
        subtitle: None,
        series_id,
        series_sequence: seq,
        year: Some(2010),
        publisher: None,
        isbn: None,
        asin: None,
        description: None,
        language: None,
        shelf_genre: None,
        cover_path: None,
        accent_rgb: None,
        folder_path: folder.into(),
        rating: 0,
        starred: false,
        added_at: None,
    }
}

fn chapter(book_id: i64, idx: i64, rel: &str, offset: f64) -> BookChapter {
    BookChapter {
        id: 0,
        book_id,
        idx,
        title: Some(format!("Chapter {idx}")),
        file_path: rel.into(),
        file_offset: offset,
        duration: Some(60.0),
    }
}

fn stage(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn chapter_paths(fx: &Fixture, book_id: i64) -> Vec<String> {
    let conn = fx.pool.open().unwrap();
    book_chapters(&conn, book_id)
        .unwrap()
        .into_iter()
        .map(|c| c.file_path)
        .collect()
}

fn folder_path(fx: &Fixture, book_id: i64) -> String {
    let conn = fx.pool.open().unwrap();
    get_book(&conn, book_id).unwrap().unwrap().folder_path
}

/// Seed a Sanderson / Stormlight book with `n` per-chapter files staged under its
/// current managed folder, the author linked, returning the book id.
async fn seed_series_book(fx: &Fixture, n: usize) -> i64 {
    let author = fx
        .worker
        .get_or_create_book_person(
            "Brandon Sanderson".to_string(),
            "Sanderson, Brandon".to_string(),
        )
        .await
        .unwrap();
    let series = fx
        .worker
        .get_or_create_series("The Stormlight Archive".to_string())
        .await
        .unwrap();
    let folder = "Audiobooks/Sanderson, Brandon/The Stormlight Archive/01. The Way of Kings (2010)";
    let book = fx
        .worker
        .insert_book(book_row(
            "The Way of Kings",
            folder,
            Some(series),
            Some(1.0),
        ))
        .await
        .unwrap();
    fx.worker.link_book_author(book, author).await.unwrap();

    let mut chapters = Vec::new();
    for i in 0..n {
        let rel = format!("{folder}/{i}.mp3");
        stage(&fx.root, &rel, format!("audio-{i}").as_bytes());
        chapters.push(chapter(book, i as i64, &rel, 0.0));
    }
    fx.worker
        .replace_book_chapters(book, chapters)
        .await
        .unwrap();
    book
}

/// A title edit re-renders the leaf folder and moves every chapter file there;
/// undo restores both the tree and the chapter rows.
#[tokio::test]
async fn title_edit_reorganizes_and_undo_restores() {
    let fx = fixture().await;
    let book = seed_series_book(&fx, 3).await;

    apply_book_edit(
        &fx.worker,
        book,
        &BookEdit {
            title: Some("Words of Radiance".into()),
            series_index: Some(2.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let plan = plan_book_reorg(&fx.pool, book, &fx.root).unwrap();
    assert!(!plan.unchanged && plan.conflicts.is_empty());
    assert_eq!(plan.ops.len(), 3);
    let new_folder =
        "Audiobooks/Sanderson, Brandon/The Stormlight Archive/02. Words of Radiance (2010)";
    assert_eq!(plan.new_folder, new_folder);

    let job = apply_book_reorg(&fx.worker, &fx.pool, book, &fx.root, MoveMode::Move)
        .await
        .unwrap()
        .expect("a move ran");

    for i in 0..3 {
        let old = format!(
            "Audiobooks/Sanderson, Brandon/The Stormlight Archive/01. The Way of Kings (2010)/{i}.mp3"
        );
        assert!(!fx.root.join(&old).exists(), "old file gone");
        assert!(
            fx.root.join(format!("{new_folder}/{i}.mp3")).exists(),
            "new file present"
        );
    }
    let paths = chapter_paths(&fx, book);
    for (i, p) in paths.iter().enumerate() {
        assert_eq!(p, &format!("{new_folder}/{i}.mp3"));
    }
    assert_eq!(folder_path(&fx, book), new_folder);

    // Undo puts everything back.
    mover::undo(&fx.worker, &fx.pool, job).await.unwrap();
    assert_eq!(
        folder_path(&fx, book),
        "Audiobooks/Sanderson, Brandon/The Stormlight Archive/01. The Way of Kings (2010)"
    );
    for i in 0..3 {
        assert!(fx.root.join(format!("Audiobooks/Sanderson, Brandon/The Stormlight Archive/01. The Way of Kings (2010)/{i}.mp3")).exists());
    }

    fx.worker.shutdown_ack().await.unwrap();
}

/// An author edit on a single-M4B book moves the one file; all its chapters
/// follow, and the folder re-renders under the new author.
#[tokio::test]
async fn author_edit_single_m4b_rewrites_all_chapters() {
    let fx = fixture().await;
    let author = fx
        .worker
        .get_or_create_book_person(
            "Brandon Sanderson".to_string(),
            "Sanderson, Brandon".to_string(),
        )
        .await
        .unwrap();
    let folder = "Audiobooks/Sanderson, Brandon/Standalone/Warbreaker (2010)";
    let book = fx
        .worker
        .insert_book(book_row("Warbreaker", folder, None, None))
        .await
        .unwrap();
    fx.worker.link_book_author(book, author).await.unwrap();
    let m4b = format!("{folder}/book.m4b");
    stage(&fx.root, &m4b, b"one-big-file");
    let chapters = vec![
        chapter(book, 0, &m4b, 0.0),
        chapter(book, 1, &m4b, 60.0),
        chapter(book, 2, &m4b, 120.0),
    ];
    fx.worker
        .replace_book_chapters(book, chapters)
        .await
        .unwrap();

    apply_book_edit(
        &fx.worker,
        book,
        &BookEdit {
            authors: Some(vec!["Neil Gaiman".into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let plan = plan_book_reorg(&fx.pool, book, &fx.root).unwrap();
    assert_eq!(
        plan.ops.len(),
        1,
        "one physical file moves, not three chapters"
    );
    let new_folder = "Audiobooks/Gaiman, Neil/Standalone/Warbreaker (2010)";
    assert_eq!(plan.new_folder, new_folder);

    apply_book_reorg(&fx.worker, &fx.pool, book, &fx.root, MoveMode::Move)
        .await
        .unwrap()
        .expect("a move ran");

    assert!(!fx.root.join(&m4b).exists());
    assert!(fx.root.join(format!("{new_folder}/book.m4b")).exists());
    let paths = chapter_paths(&fx, book);
    assert_eq!(paths.len(), 3);
    assert!(paths.iter().all(|p| p == &format!("{new_folder}/book.m4b")));
    assert_eq!(folder_path(&fx, book), new_folder);

    fx.worker.shutdown_ack().await.unwrap();
}

/// A non-path edit (or no edit) leaves the folder unchanged: the plan is a no-op
/// and apply moves nothing.
#[tokio::test]
async fn unchanged_folder_is_a_no_op() {
    let fx = fixture().await;
    let book = seed_series_book(&fx, 2).await;

    // Rating is not path-affecting, so the rendered folder still matches.
    apply_book_edit(
        &fx.worker,
        book,
        &BookEdit {
            rating: Some(5),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let plan = plan_book_reorg(&fx.pool, book, &fx.root).unwrap();
    assert!(plan.unchanged, "folder unchanged");
    assert!(plan.ops.is_empty());

    let job = apply_book_reorg(&fx.worker, &fx.pool, book, &fx.root, MoveMode::Move)
        .await
        .unwrap();
    assert!(job.is_none(), "nothing moved");

    fx.worker.shutdown_ack().await.unwrap();
}

/// A reorganize whose destination already holds a file is refused (no overwrite):
/// the plan carries a conflict and apply moves nothing.
#[tokio::test]
async fn conflicting_destination_is_refused() {
    let fx = fixture().await;
    let book = seed_series_book(&fx, 1).await;

    apply_book_edit(
        &fx.worker,
        book,
        &BookEdit {
            title: Some("Oathbringer".into()),
            series_index: Some(3.0),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Pre-occupy the destination file the move would create.
    let dest = "Audiobooks/Sanderson, Brandon/The Stormlight Archive/03. Oathbringer (2010)/0.mp3";
    stage(&fx.root, dest, b"already here");

    let plan = plan_book_reorg(&fx.pool, book, &fx.root).unwrap();
    assert!(!plan.conflicts.is_empty(), "destination conflict detected");

    // apply refuses the job (the mover bails on a conflicting plan); the source
    // stays put.
    let result = apply_book_reorg(&fx.worker, &fx.pool, book, &fx.root, MoveMode::Move).await;
    assert!(result.is_err(), "a conflicting move is refused");
    assert_eq!(
        chapter_paths(&fx, book)[0],
        "Audiobooks/Sanderson, Brandon/The Stormlight Archive/01. The Way of Kings (2010)/0.mp3",
        "source chapter path unchanged"
    );

    fx.worker.shutdown_ack().await.unwrap();
}
