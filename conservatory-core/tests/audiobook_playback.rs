//! Phase 7c integration tests: a book plays as ONE queue item through the
//! threaded engine and a null audio output. 7c-i — the **internal file advance**
//! (a multi-file book advances file to file without advancing the queue, and only
//! the last file's EOF completes the book, so a `finished` book is proof every
//! segment played; an M4B is the degenerate one-segment case). 7c-ii — the
//! book-absolute timeline: chapters progress across files, the resume position +
//! cursor persist, a cross-file seek lands in the right file, and a completed
//! book writes one `listening_sessions` row keyed by `book_id`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use conservatory_core::db::{
    Book, BookChapter, MediaKind, ReadPool, get_book_playback, read_playback_state, spawn_worker,
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
    chapter_dur(idx, file, offset, 2.0)
}

fn chapter_dur(idx: i64, file: &str, offset: f64, duration: f64) -> BookChapter {
    BookChapter {
        id: 0,
        book_id: 0,
        idx,
        title: Some(format!("Chapter {}", idx + 1)),
        file_path: file.to_string(),
        file_offset: offset,
        duration: Some(duration),
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

// --- 7c-ii: the book-absolute timeline (resume, cross-file seek, session). ---

/// A three-file book whose declared chapter durations (`dur`) sit far past the
/// short fixture, so each file occupies its own chapter region on the
/// book-absolute timeline (chapter starts `0`, `dur`, `2*dur`). Returns the
/// worker, pool, book id, and the chapters the engine will plan.
fn three_file_book(
    runtime: &tokio::runtime::Runtime,
    db: PathBuf,
    dur: f64,
) -> (
    conservatory_core::db::WorkerHandle,
    ReadPool,
    i64,
    Vec<BookChapter>,
) {
    let worker = {
        let _g = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };
    let pool = ReadPool::new(db, 2).unwrap();
    let chapters = vec![
        chapter_dur(0, "Book/01.mp3", 0.0, dur),
        chapter_dur(1, "Book/02.mp3", 0.0, dur),
        chapter_dur(2, "Book/03.mp3", 0.0, dur),
    ];
    let book_id = runtime.block_on(async {
        let id = worker.insert_book(book_row("Book")).await.unwrap();
        worker
            .replace_book_chapters(id, chapters.clone())
            .await
            .unwrap();
        id
    });
    (worker, pool, book_id, chapters)
}

const FILES_3: [&str; 3] = ["Book/01.mp3", "Book/02.mp3", "Book/03.mp3"];

#[test]
fn chapters_progress_across_files_on_the_book_timeline() {
    let (_dir, runtime, root, db) = setup(&FILES_3);
    let (worker, _pool, book_id, chapters) = three_file_book(&runtime, db, 60.0);
    let item =
        player::build_book_item(book_id, &chapters, &root, resolve_episode_profile(None)).unwrap();

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item], 0);

    // As the engine advances file to file, the snapshot's current chapter (read
    // from the book-absolute position against the absolute marks) should visit
    // every chapter, even though each file's own `time_pos` restarts at 0.
    let mut seen = std::collections::BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snap = player.snapshot();
        if let Some(c) = snap.current_chapter {
            seen.insert(c);
        }
        if snap.ended {
            break;
        }
        assert!(Instant::now() < deadline, "did not finish in time");
        std::thread::sleep(Duration::from_millis(20));
    }
    player.shutdown();
    assert!(
        seen.contains(&0) && seen.contains(&1) && seen.contains(&2),
        "the playhead crossed all three chapters on the book timeline, saw {seen:?}"
    );
    runtime.block_on(worker.shutdown_ack()).ok();
}

#[test]
fn resume_position_and_cursor_persist_mid_book() {
    let (_dir, runtime, root, db) = setup(&FILES_3);
    let (worker, pool, book_id, chapters) = three_file_book(&runtime, db, 60.0);
    let item =
        player::build_book_item(book_id, &chapters, &root, resolve_episode_profile(None)).unwrap();

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item], 0);

    // Play into the second file: its book-absolute position starts at 60 (the
    // declared first-chapter duration), so position > 50 means the engine crossed
    // the file boundary. Pause there so the resume position is captured.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snap = player.snapshot();
        if snap.position > 50.0 {
            break;
        }
        assert!(
            !snap.ended && Instant::now() < deadline,
            "never reached the second file"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    player.pause();
    player.shutdown(); // terminal flush lands the cursor + book position

    let conn = pool.open().unwrap();
    // The per-book resume position (spec §6.4) is the absolute book offset.
    let pb = get_book_playback(&conn, book_id).unwrap().unwrap();
    assert!(
        pb.position > 50.0 && !pb.finished,
        "the absolute resume position persisted mid-book, got {}",
        pb.position
    );
    // The transport cursor reopens the book (kind + book_id), not a track/episode.
    let cur = read_playback_state(&conn).unwrap().unwrap();
    assert_eq!(cur.kind, MediaKind::Audiobook);
    assert_eq!(cur.book_id, Some(book_id));
    assert_eq!(cur.track_id, None);
    assert_eq!(cur.episode_id, None);
    assert!(cur.position > 50.0);

    runtime.block_on(worker.shutdown_ack()).ok();
}

#[test]
fn seek_crosses_a_file_boundary() {
    let (_dir, runtime, root, db) = setup(&FILES_3);
    let (worker, _pool, book_id, chapters) = three_file_book(&runtime, db, 60.0);
    let item =
        player::build_book_item(book_id, &chapters, &root, resolve_episode_profile(None)).unwrap();

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item], 0);
    // Seek to the third file's book-absolute start (2 * 60). The engine must load
    // that file and seek within it, so the reported position jumps into [120, …).
    player.seek(120.0);

    let mut max_pos = 0.0_f64;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snap = player.snapshot();
        max_pos = max_pos.max(snap.position);
        if snap.ended {
            break;
        }
        assert!(Instant::now() < deadline, "did not finish in time");
        std::thread::sleep(Duration::from_millis(20));
    }
    player.shutdown();
    assert!(
        max_pos >= 119.0,
        "the cross-file seek reached the third file's book-absolute range, max {max_pos}"
    );
    runtime.block_on(worker.shutdown_ack()).ok();
}

#[test]
fn completed_book_writes_one_listening_session() {
    let (_dir, runtime, root, db) = setup(&FILES_3);
    let (worker, pool, book_id, chapters) = three_file_book(&runtime, db, 60.0);
    let item =
        player::build_book_item(book_id, &chapters, &root, resolve_episode_profile(None)).unwrap();

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item], 0);
    play_to_end(&player);
    player.shutdown();

    // The session closed at completion and wrote one append-only row keyed by the
    // book (book_id, not episode_id), spec §6.3.
    let conn = pool.open().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM listening_sessions WHERE book_id = ?1",
            [book_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "a completed book writes exactly one book session");

    runtime.block_on(worker.shutdown_ack()).ok();
}
