//! Phase 7a-iii integration suite for the book side of the file mover (spec
//! §5.4, §5.7). Books are owned and moved like music, so the same release
//! blocking guarantees apply: a per-chapter-file move round-trip, the single-M4B
//! case where one moved file backs many chapters, undo, and crash replay are
//! verified end to end against real files on disk.

use std::fs;
use std::path::Path;

use conservatory_core::db::{
    Book, BookChapter, ReadPool, WorkerHandle, book_chapters, get_book, spawn_worker,
};
use conservatory_core::mover::journal::{self, JobState};
use conservatory_core::mover::{self, MoveKind, MoveMode, MoveOp, fsops};
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

fn book_row(folder: &str) -> Book {
    Book {
        id: 0,
        title: "The Book".into(),
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
        folder_path: folder.into(),
        rating: 0,
        starred: false,
        added_at: None,
    }
}

fn chapter(book_id: i64, idx: i64, file: &str, offset: f64) -> BookChapter {
    BookChapter {
        id: 0,
        book_id,
        idx,
        title: Some(format!("Chapter {idx}")),
        file_path: file.into(),
        file_offset: offset,
        duration: Some(60.0),
    }
}

fn stage(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn op(root: &Path, book_id: i64, old: &str, new: &str) -> MoveOp {
    MoveOp {
        track_id: None,
        album_id: None,
        book_id: Some(book_id),
        src: root.join(old),
        dst: root.join(new),
        db_old: Some(root.join(old).to_string_lossy().into_owned()),
        db_new: Some(new.to_string()),
    }
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

/// Multi-file book: one chapter per file, each relocating independently.
#[tokio::test]
async fn multi_file_book_move_updates_tree_and_db() {
    let fx = fixture().await;
    let book = fx.worker.insert_book(book_row("incoming")).await.unwrap();

    let n = 3;
    let mut chapters = Vec::new();
    for i in 0..n {
        let src_abs = fx.root.join(format!("incoming/{i}.mp3"));
        stage(
            &fx.root,
            &format!("incoming/{i}.mp3"),
            format!("audio-{i}").as_bytes(),
        );
        chapters.push(chapter(book, i as i64, &src_abs.to_string_lossy(), 0.0));
    }
    fx.worker
        .replace_book_chapters(book, chapters)
        .await
        .unwrap();

    let ops: Vec<MoveOp> = (0..n)
        .map(|i| {
            op(
                &fx.root,
                book,
                &format!("incoming/{i}.mp3"),
                &format!("Audiobooks/Author/Standalone/The Book (2021)/{i}.mp3"),
            )
        })
        .collect();
    let job = mover::apply(
        &fx.worker,
        &fx.pool,
        MoveKind::Import,
        MoveMode::Move,
        &fx.root,
        0,
        ops,
    )
    .await
    .unwrap();

    for i in 0..n {
        assert!(!fx.root.join(format!("incoming/{i}.mp3")).exists());
        let dst = format!("Audiobooks/Author/Standalone/The Book (2021)/{i}.mp3");
        assert_eq!(
            fs::read(fx.root.join(&dst)).unwrap(),
            format!("audio-{i}").as_bytes()
        );
    }
    // Every chapter's file_path flipped to the managed rel path.
    let paths = chapter_paths(&fx, book);
    for (i, p) in paths.iter().enumerate() {
        assert_eq!(
            p,
            &format!("Audiobooks/Author/Standalone/The Book (2021)/{i}.mp3")
        );
    }
    assert_eq!(
        folder_path(&fx, book),
        "Audiobooks/Author/Standalone/The Book (2021)"
    );
    assert_eq!(job_state(&fx, job), JobState::Completed);

    fx.worker.shutdown_ack().await.unwrap();
}

/// A single M4B that backs many chapters: one moved file, all chapter rows
/// rewritten by the (book_id, path) match.
#[tokio::test]
async fn single_m4b_move_rewrites_all_its_chapters() {
    let fx = fixture().await;
    let book = fx.worker.insert_book(book_row("incoming")).await.unwrap();

    let src_abs = fx.root.join("incoming/book.m4b");
    stage(&fx.root, "incoming/book.m4b", b"one-big-file");
    let src_str = src_abs.to_string_lossy().into_owned();
    // Three chapters, all addressing the one file at different offsets.
    let chapters = vec![
        chapter(book, 0, &src_str, 0.0),
        chapter(book, 1, &src_str, 60.0),
        chapter(book, 2, &src_str, 120.0),
    ];
    fx.worker
        .replace_book_chapters(book, chapters)
        .await
        .unwrap();

    let dst_rel = "Audiobooks/Author/Standalone/The Book (2021)/book.m4b";
    let ops = vec![op(&fx.root, book, "incoming/book.m4b", dst_rel)];
    let job = mover::apply(
        &fx.worker,
        &fx.pool,
        MoveKind::Import,
        MoveMode::Move,
        &fx.root,
        0,
        ops,
    )
    .await
    .unwrap();

    assert!(!fx.root.join("incoming/book.m4b").exists());
    assert!(fx.root.join(dst_rel).exists());
    // All three chapter rows now point at the one managed file.
    let paths = chapter_paths(&fx, book);
    assert_eq!(paths.len(), 3);
    assert!(
        paths.iter().all(|p| p == dst_rel),
        "every chapter follows the file: {paths:?}"
    );
    assert_eq!(
        folder_path(&fx, book),
        "Audiobooks/Author/Standalone/The Book (2021)"
    );
    assert_eq!(job_state(&fx, job), JobState::Completed);

    fx.worker.shutdown_ack().await.unwrap();
}

/// Undo restores both the files and every chapter's DB path.
#[tokio::test]
async fn undo_restores_book_tree_and_db() {
    let fx = fixture().await;
    let book = fx.worker.insert_book(book_row("incoming")).await.unwrap();

    let src_abs = fx.root.join("incoming/book.m4b");
    stage(&fx.root, "incoming/book.m4b", b"one-big-file");
    let src_str = src_abs.to_string_lossy().into_owned();
    let chapters = vec![
        chapter(book, 0, &src_str, 0.0),
        chapter(book, 1, &src_str, 60.0),
    ];
    fx.worker
        .replace_book_chapters(book, chapters)
        .await
        .unwrap();

    let dst_rel = "Audiobooks/Author/Standalone/The Book (2021)/book.m4b";
    let ops = vec![op(&fx.root, book, "incoming/book.m4b", dst_rel)];
    // Copy mode: the source stays, so undo just clears the managed copy and the
    // DB paths revert to the source.
    let job = mover::apply(
        &fx.worker,
        &fx.pool,
        MoveKind::Import,
        MoveMode::Copy,
        &fx.root,
        0,
        ops,
    )
    .await
    .unwrap();
    assert!(chapter_paths(&fx, book).iter().all(|p| p == dst_rel));

    mover::undo(&fx.worker, &fx.pool, job).await.unwrap();

    assert!(
        fx.root.join("incoming/book.m4b").exists(),
        "copy source intact"
    );
    assert!(!fx.root.join(dst_rel).exists(), "managed copy removed");
    let paths = chapter_paths(&fx, book);
    assert!(
        paths.iter().all(|p| p == &src_str),
        "chapters reverted to source: {paths:?}"
    );
    assert_eq!(job_state(&fx, job), JobState::Undone);

    fx.worker.shutdown_ack().await.unwrap();
}

/// A crash mid-job rolls forward on recovery.
#[tokio::test]
async fn crash_mid_book_job_rolls_forward() {
    let fx = fixture().await;
    let book = fx.worker.insert_book(book_row("incoming")).await.unwrap();

    let n = 3;
    let mut chapters = Vec::new();
    for i in 0..n {
        let src_abs = fx.root.join(format!("incoming/{i}.mp3"));
        stage(
            &fx.root,
            &format!("incoming/{i}.mp3"),
            format!("audio-{i}").as_bytes(),
        );
        chapters.push(chapter(book, i as i64, &src_abs.to_string_lossy(), 0.0));
    }
    fx.worker
        .replace_book_chapters(book, chapters)
        .await
        .unwrap();

    let ops: Vec<MoveOp> = (0..n)
        .map(|i| {
            op(
                &fx.root,
                book,
                &format!("incoming/{i}.mp3"),
                &format!("Audiobooks/Author/Standalone/The Book (2021)/{i}.mp3"),
            )
        })
        .collect();

    // Journal the job, then simulate a crash: move the first file by hand and
    // never mark it done, leaving the job in_progress with all ops pending.
    let job = fx
        .worker
        .create_move_job(
            MoveKind::Import,
            MoveMode::Move,
            fx.root.to_string_lossy().into_owned(),
            0,
            ops.clone(),
        )
        .await
        .unwrap();
    fsops::relocate(&ops[0].src, &ops[0].dst, MoveMode::Move).unwrap();
    assert_eq!(job_state(&fx, job), JobState::InProgress);

    let recovered = mover::recover(&fx.worker, &fx.pool).await.unwrap();
    assert_eq!(recovered, 1);

    for i in 0..n {
        assert!(!fx.root.join(format!("incoming/{i}.mp3")).exists());
        assert!(
            fx.root
                .join(format!(
                    "Audiobooks/Author/Standalone/The Book (2021)/{i}.mp3"
                ))
                .exists()
        );
    }
    let paths = chapter_paths(&fx, book);
    for (i, p) in paths.iter().enumerate() {
        assert_eq!(
            p,
            &format!("Audiobooks/Author/Standalone/The Book (2021)/{i}.mp3")
        );
    }
    assert_eq!(job_state(&fx, job), JobState::Completed);

    fx.worker.shutdown_ack().await.unwrap();
}

fn job_state(fx: &Fixture, job_id: i64) -> JobState {
    let conn = fx.pool.open().unwrap();
    journal::get_job(&conn, job_id).unwrap().unwrap().state
}
