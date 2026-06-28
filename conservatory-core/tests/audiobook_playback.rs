//! Phase 7c-i integration tests: a book plays as ONE queue item through the
//! threaded engine and a null audio output. The novel behaviour is the
//! **internal file advance** — a multi-file book advances file to file without
//! advancing the queue, and only the last file's EOF completes the book
//! (`book_playback.finished`), so a `finished` book is proof that every segment
//! played. An M4B (single file) is the degenerate one-segment case.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use conservatory_core::db::{
    Book, BookChapter, MediaKind, ReadPool, get_book_playback, spawn_worker,
};
use conservatory_core::player;
use conservatory_core::resolve_episode_profile;
use tempfile::tempdir;

fn book_row(folder: &str) -> Book {
    Book {
        id: 0,
        title: "A Test Book".to_string(),
        subtitle: None,
        series_id: None,
        series_sequence: None,
        year: Some(2024),
        publisher: None,
        isbn: None,
        asin: None,
        description: None,
        language: Some("en".to_string()),
        shelf_genre: None,
        cover_path: None,
        accent_rgb: None,
        folder_path: folder.to_string(),
        rating: 0,
        starred: false,
        added_at: None,
    }
}

fn chapter(idx: i64, file: &str, offset: f64) -> BookChapter {
    BookChapter {
        id: 0,
        book_id: 0,
        idx,
        title: Some(format!("Chapter {}", idx + 1)),
        file_path: file.to_string(),
        file_offset: offset,
        duration: Some(2.0),
    }
}

/// A multi-thread runtime and a root dir seeded with copies of the short fixture
/// mp3 under `Book/`. The DB path is returned unopened: the worker (spawned by
/// the test) creates and migrates it, and the read pool opens after that.
fn setup(files: &[&str]) -> (tempfile::TempDir, tokio::runtime::Runtime, PathBuf, PathBuf) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let db = root.join("library.db");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/sample.mp3");
    std::fs::create_dir_all(root.join("Book")).unwrap();
    for f in files {
        std::fs::copy(&fixture, root.join(f)).unwrap();
    }
    (dir, runtime, root, db)
}

fn play_to_end(player: &conservatory_core::PlayerHandle) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if player.snapshot().ended {
            break;
        }
        assert!(Instant::now() < deadline, "engine did not finish in time");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn multi_file_book_advances_through_every_file_then_completes() {
    let (_dir, runtime, root, db) = setup(&["Book/01.mp3", "Book/02.mp3", "Book/03.mp3"]);
    let worker = {
        let _g = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };
    let pool = ReadPool::new(db, 2).unwrap();

    let chapters = vec![
        chapter(0, "Book/01.mp3", 0.0),
        chapter(1, "Book/02.mp3", 0.0),
        chapter(2, "Book/03.mp3", 0.0),
    ];
    let book_id = runtime.block_on(async {
        let id = worker.insert_book(book_row("Book")).await.unwrap();
        worker
            .replace_book_chapters(id, chapters.clone())
            .await
            .unwrap();
        id
    });

    let item = player::build_book_item(book_id, &chapters, &root, resolve_episode_profile(None))
        .expect("a book with chapters builds an item");
    assert_eq!(item.kind, MediaKind::Audiobook);
    assert_eq!(item.segments.len(), 3, "three files → three segments");

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item], 0);
    play_to_end(&player);
    player.shutdown();

    // `finished` is set only at the LAST file's EOF, so it proves the engine
    // advanced through all three files inside the one queue item.
    let conn = pool.open().unwrap();
    let pb = get_book_playback(&conn, book_id).unwrap().unwrap();
    assert!(pb.finished, "the book completed after its last file");
    assert_eq!(pb.position, 0.0, "completion clears the resume position");

    runtime.block_on(worker.shutdown_ack()).ok();
}

#[test]
fn m4b_single_file_book_with_chapter_marks_completes() {
    let (_dir, runtime, root, db) = setup(&["Book/book.m4b"]);
    let worker = {
        let _g = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };
    let pool = ReadPool::new(db, 2).unwrap();

    // One file, three chapter offsets (the M4B shape) — a single segment.
    let chapters = vec![
        chapter(0, "Book/book.m4b", 0.0),
        chapter(1, "Book/book.m4b", 0.5),
        chapter(2, "Book/book.m4b", 1.0),
    ];
    let book_id = runtime.block_on(async {
        let id = worker.insert_book(book_row("Book")).await.unwrap();
        worker
            .replace_book_chapters(id, chapters.clone())
            .await
            .unwrap();
        id
    });

    let item = player::build_book_item(book_id, &chapters, &root, resolve_episode_profile(None))
        .expect("a book with chapters builds an item");
    assert_eq!(item.segments.len(), 1, "one file → one segment");
    assert_eq!(item.chapters.len(), 3, "all three marks attached, absolute");

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item], 0);
    play_to_end(&player);
    player.shutdown();

    let conn = pool.open().unwrap();
    let pb = get_book_playback(&conn, book_id).unwrap().unwrap();
    assert!(pb.finished, "a single-file book completes at EOF");

    runtime.block_on(worker.shutdown_ack()).ok();
}
