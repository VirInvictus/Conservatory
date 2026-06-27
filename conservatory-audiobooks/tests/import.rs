//! Phase 7a-iii end-to-end import test. Imports the committed multi-file fixture
//! in copy mode into a temp root and asserts the resolved rows, the moved files
//! under the rendered `Audiobooks/...` folder, and that a second import (the
//! folder already exists) is refused with no partial writes. CI-hermetic: copy
//! mode never touches the committed fixtures.

use std::path::PathBuf;

use conservatory_audiobooks::{BookImportOptions, import_book};
use conservatory_core::db::{
    ReadPool, book_authors, book_chapters, book_narrators, get_book, list_books, spawn_worker,
};
use conservatory_core::mover::MoveMode;
use tempfile::tempdir;

fn multi_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi/Test Author/Test Book")
}

#[tokio::test]
async fn import_multi_file_book_copies_and_records_rows() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let root = dir.path().join("lib");
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db, 3).unwrap();

    let report = import_book(
        &worker,
        &pool,
        &multi_fixture(),
        &BookImportOptions {
            library_root: root.clone(),
            mode: MoveMode::Copy,
        },
    )
    .await
    .unwrap();

    assert!(report.conflicts.is_empty(), "clean import");
    let book_id = report.book_id.expect("a book was created");
    assert_eq!(report.chapters, 2);
    assert_eq!(report.files, 2);

    let conn = pool.open().unwrap();

    // The book row, under the rendered standalone folder (sort-name author).
    let book = get_book(&conn, book_id).unwrap().unwrap();
    assert_eq!(book.title, "Test Book");
    assert_eq!(book.year, Some(2021));
    assert_eq!(
        book.folder_path,
        "Audiobooks/Author, Test/Standalone/Test Book (2021)"
    );
    assert_eq!(book.cover_path, None, "fixture has no cover");

    // People, by role.
    let authors = book_authors(&conn, book_id).unwrap();
    assert_eq!(
        authors
            .iter()
            .map(|p| p.sort_name.as_str())
            .collect::<Vec<_>>(),
        vec!["Author, Test"]
    );
    let narrators = book_narrators(&conn, book_id).unwrap();
    assert_eq!(
        narrators
            .iter()
            .map(|p| p.sort_name.as_str())
            .collect::<Vec<_>>(),
        vec!["Reader, Test"]
    );

    // Ordered chapter rows, file_path flipped to the managed tree.
    let chapters = book_chapters(&conn, book_id).unwrap();
    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[0].idx, 0);
    assert_eq!(chapters[1].idx, 1);
    for ch in &chapters {
        assert!(
            ch.file_path
                .starts_with("Audiobooks/Author, Test/Standalone/Test Book (2021)/"),
            "chapter managed: {}",
            ch.file_path
        );
        assert!(
            root.join(&ch.file_path).exists(),
            "file on disk: {}",
            ch.file_path
        );
    }
    drop(conn);

    // The committed fixture is untouched by a copy import.
    assert!(
        multi_fixture().join("01.mp3").exists(),
        "copy leaves source"
    );

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn second_import_is_refused_without_partial_writes() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let root = dir.path().join("lib");
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db, 3).unwrap();
    let opts = BookImportOptions {
        library_root: root.clone(),
        mode: MoveMode::Copy,
    };

    import_book(&worker, &pool, &multi_fixture(), &opts)
        .await
        .unwrap();

    // The book folder now exists: a re-import collides and is refused.
    let again = import_book(&worker, &pool, &multi_fixture(), &opts)
        .await
        .unwrap();
    assert!(!again.conflicts.is_empty(), "re-import is blocked");
    assert_eq!(again.book_id, None, "no second book row");

    let conn = pool.open().unwrap();
    assert_eq!(
        list_books(&conn).unwrap().len(),
        1,
        "no partial second book"
    );

    worker.shutdown_ack().await.unwrap();
}
