//! Phase 7a-i integration tests: the audiobook schema (migration 0011) and the
//! core worker CRUD that backs the `conservatory-audiobooks` plugin (spec §4.5).
//! No reader/import here (that is 7a-ii/iii); these exercise the worker write
//! path and the read-pool round-trip, the role-tagged author/narrator links, the
//! `book_fts` triggers that denormalize from those link tables, and the
//! structural change to the unified queue (the deferred `book_id` foreign key).

use chrono::{DateTime, TimeZone, Utc};
use conservatory_core::db::{
    Book, BookChapter, BookPlayback, BookState, ReadPool, WorkerHandle, book_authors,
    book_chapters, book_narrators, get_book, get_book_playback, list_book_rows, list_books,
    series_for_book, sort_shelf, spawn_worker,
};
use tempfile::tempdir;

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().unwrap()
}

fn sample_book(title: &str, folder: &str) -> Book {
    Book {
        id: 0,
        title: title.to_string(),
        subtitle: Some("Day One".to_string()),
        series_id: None,
        series_sequence: None,
        year: Some(2009),
        publisher: Some("Brilliance Audio".to_string()),
        isbn: None,
        asin: Some("B0036WMOG2".to_string()),
        description: Some("Kvothe tells his story to a chronicler.".to_string()),
        language: Some("en".to_string()),
        shelf_genre: Some("Audiobook".to_string()),
        cover_path: None,
        accent_rgb: Some(0x3366cc),
        folder_path: folder.to_string(),
        rating: 5,
        starred: true,
        added_at: Some(ts(1_700_000_000)),
    }
}

fn chapter(idx: i64, title: &str, file: &str) -> BookChapter {
    BookChapter {
        id: 0,
        book_id: 0, // set by the worker against the book it replaces under
        idx,
        title: Some(title.to_string()),
        file_path: file.to_string(),
        file_offset: 0.0,
        duration: Some(600.0),
    }
}

async fn fresh() -> (tempfile::TempDir, WorkerHandle, ReadPool) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    let pool = ReadPool::new(path, 3).unwrap();
    (dir, worker, pool)
}

#[tokio::test]
async fn book_people_series_and_chapters_round_trip() {
    let (_dir, worker, pool) = fresh().await;

    // Author and narrator are distinct roles over the shared people table.
    let author = worker
        .get_or_create_book_person("Patrick Rothfuss", "Rothfuss, Patrick")
        .await
        .unwrap();
    let narrator = worker
        .get_or_create_book_person("Nick Podehl", "Podehl, Nick")
        .await
        .unwrap();
    assert_ne!(author, narrator);

    // Dedup is on sort_name: a second get_or_create returns the same row.
    let author_again = worker
        .get_or_create_book_person("Patrick Rothfuss", "Rothfuss, Patrick")
        .await
        .unwrap();
    assert_eq!(author, author_again);

    let series = worker
        .get_or_create_series("The Kingkiller Chronicle")
        .await
        .unwrap();
    let series_again = worker
        .get_or_create_series("The Kingkiller Chronicle")
        .await
        .unwrap();
    assert_eq!(series, series_again);

    let mut book = sample_book(
        "The Name of the Wind",
        "Audiobooks/Rothfuss, Patrick/The Kingkiller Chronicle/1. The Name of the Wind (2009)",
    );
    book.series_id = Some(series);
    book.series_sequence = Some(1.0);
    let book_id = worker.insert_book(book).await.unwrap();

    worker.link_book_author(book_id, author).await.unwrap();
    worker.link_book_narrator(book_id, narrator).await.unwrap();
    // Linking the same author twice is a no-op (idempotent m2m).
    worker.link_book_author(book_id, author).await.unwrap();

    worker
        .replace_book_chapters(
            book_id,
            vec![
                chapter(0, "Prologue", "00.mp3"),
                chapter(1, "A Place for Demons", "01.mp3"),
                chapter(2, "A Beautiful Day", "02.mp3"),
            ],
        )
        .await
        .unwrap();

    worker
        .upsert_book_playback(BookPlayback {
            book_id,
            position: 742.5,
            finished: false,
            last_played: Some(ts(1_700_100_000)),
            speed: Some(1.4),
            smart_speed: Some(true),
            voice_boost: None,
        })
        .await
        .unwrap();

    let conn = pool.open().unwrap();

    let got = get_book(&conn, book_id).unwrap().unwrap();
    assert_eq!(got.title, "The Name of the Wind");
    assert_eq!(got.series_id, Some(series));
    assert_eq!(got.series_sequence, Some(1.0));
    assert_eq!(got.asin.as_deref(), Some("B0036WMOG2"));
    assert_eq!(got.rating, 5);
    assert!(got.starred);
    assert_eq!(got.accent_rgb, Some(0x3366cc));

    assert_eq!(list_books(&conn).unwrap().len(), 1);

    let authors = book_authors(&conn, book_id).unwrap();
    assert_eq!(
        authors.len(),
        1,
        "duplicate link must not create a second row"
    );
    assert_eq!(authors[0].sort_name, "Rothfuss, Patrick");
    let narrators = book_narrators(&conn, book_id).unwrap();
    assert_eq!(narrators.len(), 1);
    assert_eq!(narrators[0].name, "Nick Podehl");

    let series_row = series_for_book(&conn, book_id).unwrap().unwrap();
    assert_eq!(series_row.name, "The Kingkiller Chronicle");

    let chapters = book_chapters(&conn, book_id).unwrap();
    assert_eq!(chapters.len(), 3);
    assert_eq!(chapters[0].idx, 0);
    assert_eq!(chapters[0].title.as_deref(), Some("Prologue"));
    assert_eq!(chapters[2].title.as_deref(), Some("A Beautiful Day"));

    let pb = get_book_playback(&conn, book_id).unwrap().unwrap();
    assert!((pb.position - 742.5).abs() < 1e-9);
    assert!(!pb.finished);
    assert_eq!(pb.speed, Some(1.4));
    assert_eq!(pb.smart_speed, Some(true));
    assert_eq!(
        pb.voice_boost, None,
        "NULL override inherits the global default"
    );

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn replace_book_chapters_is_clean_not_append() {
    let (_dir, worker, pool) = fresh().await;
    let book_id = worker
        .insert_book(sample_book(
            "Standalone",
            "Audiobooks/Gaiman, Neil/Standalone/The Graveyard Book",
        ))
        .await
        .unwrap();

    worker
        .replace_book_chapters(
            book_id,
            vec![
                chapter(0, "Part 1", "p1.m4b"),
                chapter(1, "Part 2", "p2.m4b"),
            ],
        )
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        assert_eq!(book_chapters(&conn, book_id).unwrap().len(), 2);
    }

    worker
        .replace_book_chapters(book_id, vec![chapter(0, "Only one now", "p1.m4b")])
        .await
        .unwrap();
    let conn = pool.open().unwrap();
    let chapters = book_chapters(&conn, book_id).unwrap();
    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0].title.as_deref(), Some("Only one now"));

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn book_position_and_completion_writes() {
    let (_dir, worker, pool) = fresh().await;
    let book_id = worker
        .insert_book(sample_book("A Book", "Audiobooks/A/Standalone/A Book"))
        .await
        .unwrap();

    // A resume tick records the absolute position and is not finished.
    worker
        .set_book_position(book_id, 333.0, Some(10))
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let pb = get_book_playback(&conn, book_id).unwrap().unwrap();
        assert!((pb.position - 333.0).abs() < 1e-9);
        assert!(!pb.finished);
        assert_eq!(pb.last_played, Some(ts(10)));
    }

    // Completion marks finished and rewinds the position to 0.
    worker.complete_book(book_id, Some(20)).await.unwrap();
    {
        let conn = pool.open().unwrap();
        let pb = get_book_playback(&conn, book_id).unwrap().unwrap();
        assert!(pb.finished);
        assert_eq!(pb.position, 0.0);
        assert_eq!(pb.last_played, Some(ts(20)));
    }

    // Resuming a finished book un-finishes it (you can re-listen).
    worker
        .set_book_position(book_id, 5.0, Some(30))
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let pb = get_book_playback(&conn, book_id).unwrap().unwrap();
        assert!(!pb.finished);
        assert!((pb.position - 5.0).abs() < 1e-9);
    }

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn book_fts_denormalizes_author_narrator_and_series_from_links() {
    let (_dir, worker, pool) = fresh().await;

    let author = worker
        .get_or_create_book_person("Brandon Sanderson", "Sanderson, Brandon")
        .await
        .unwrap();
    let narrator = worker
        .get_or_create_book_person("Michael Kramer", "Kramer, Michael")
        .await
        .unwrap();
    let series = worker
        .get_or_create_series("The Stormlight Archive")
        .await
        .unwrap();

    let mut book = sample_book(
        "The Way of Kings",
        "Audiobooks/Sanderson, Brandon/The Stormlight Archive/1. The Way of Kings",
    );
    book.series_id = Some(series);
    let book_id = worker.insert_book(book).await.unwrap();

    let fts_count = |q: &str| -> i64 {
        let conn = pool.open().unwrap();
        conn.query_row(
            "SELECT count(*) FROM book_fts WHERE book_fts MATCH ?1",
            [q],
            |r| r.get(0),
        )
        .unwrap()
    };

    // Title and series denormalize at insert (the books_ai trigger).
    assert_eq!(fts_count("title:Kings"), 1);
    assert_eq!(fts_count("series:Stormlight"), 1);
    // Author / narrator are empty until the role links are added.
    assert_eq!(fts_count("author:Sanderson"), 0);

    worker.link_book_author(book_id, author).await.unwrap();
    worker.link_book_narrator(book_id, narrator).await.unwrap();
    assert_eq!(
        fts_count("author:Sanderson"),
        1,
        "link re-aggregates author"
    );
    assert_eq!(fts_count("narrator:Kramer"), 1);

    // A second author re-aggregates the column (both searchable).
    let coauthor = worker
        .get_or_create_book_person("Kaladin Stormblessed", "Stormblessed, Kaladin")
        .await
        .unwrap();
    worker.link_book_author(book_id, coauthor).await.unwrap();
    assert_eq!(fts_count("author:Sanderson"), 1);
    assert_eq!(fts_count("author:Stormblessed"), 1);

    // A person / series rename propagates back into the index (the *_au triggers).
    // No worker rename verb exists in 7a-i, so fire the triggers via a raw writer.
    {
        let conn = rusqlite::Connection::open(_dir.path().join("t.db")).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        conn.execute(
            "UPDATE book_people SET name = 'Anonymous' WHERE id = ?1",
            [author],
        )
        .unwrap();
        conn.execute(
            "UPDATE series SET name = 'Stormlight' WHERE id = ?1",
            [series],
        )
        .unwrap();
    }
    assert_eq!(
        fts_count("author:Sanderson"),
        0,
        "renamed person leaves the index"
    );
    assert_eq!(fts_count("author:Anonymous"), 1, "the new name is indexed");
    assert_eq!(fts_count("series:Stormlight"), 1);

    // Deleting the book cascades it out of the index (books_ad + link cascades).
    {
        let conn = rusqlite::Connection::open(_dir.path().join("t.db")).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute("DELETE FROM books WHERE id = ?1", [book_id])
            .unwrap();
    }
    assert_eq!(fts_count("title:Kings"), 0);

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn shelf_rows_denormalize_and_sort_by_state() {
    let (_dir, worker, pool) = fresh().await;

    // Book A: a series entry, author + narrator, two chapters, in progress.
    let author = worker
        .get_or_create_book_person("Patrick Rothfuss", "Rothfuss, Patrick")
        .await
        .unwrap();
    let narrator = worker
        .get_or_create_book_person("Nick Podehl", "Podehl, Nick")
        .await
        .unwrap();
    let series = worker
        .get_or_create_series("The Kingkiller Chronicle")
        .await
        .unwrap();
    let mut a = sample_book("AAA In Progress", "Audiobooks/Rothfuss, Patrick/.../a");
    a.series_id = Some(series);
    a.series_sequence = Some(1.0);
    let a_id = worker.insert_book(a).await.unwrap();
    worker.link_book_author(a_id, author).await.unwrap();
    worker.link_book_narrator(a_id, narrator).await.unwrap();
    worker
        .replace_book_chapters(
            a_id,
            vec![chapter(0, "One", "0.mp3"), chapter(1, "Two", "1.mp3")],
        )
        .await
        .unwrap();
    worker
        .set_book_position(a_id, 120.0, Some(500))
        .await
        .unwrap();

    // Book B: standalone, never started (New). Title sorts before A.
    let b_id = worker
        .insert_book(sample_book("AAA New", "Audiobooks/B/Standalone/b"))
        .await
        .unwrap();

    // Book C: finished. Title sorts first of all.
    let c_id = worker
        .insert_book(sample_book("AAA Finished", "Audiobooks/C/Standalone/c"))
        .await
        .unwrap();
    worker.complete_book(c_id, Some(600)).await.unwrap();

    let conn = pool.open().unwrap();
    let mut rows = list_book_rows(&conn).unwrap();
    assert_eq!(rows.len(), 3);

    // The in-progress book carries its denormalized credits, series, duration,
    // and resume state in one read.
    let a = rows.iter().find(|r| r.id == a_id).unwrap();
    assert_eq!(a.author_display.as_deref(), Some("Patrick Rothfuss"));
    assert_eq!(a.narrator_display.as_deref(), Some("Nick Podehl"));
    assert_eq!(a.series_name.as_deref(), Some("The Kingkiller Chronicle"));
    assert_eq!(a.series_sequence, Some(1.0));
    assert!(
        (a.total_duration - 1200.0).abs() < 1e-9,
        "sum of chapter durations"
    );
    assert!((a.position - 120.0).abs() < 1e-9);
    assert_eq!(a.state(), BookState::InProgress);

    let b = rows.iter().find(|r| r.id == b_id).unwrap();
    assert_eq!(b.state(), BookState::New);
    assert!(b.author_display.is_none(), "no credits linked");
    let c = rows.iter().find(|r| r.id == c_id).unwrap();
    assert_eq!(c.state(), BookState::Finished);

    // Shelf order: in-progress first, then new, then finished (state beats the
    // alphabetical title order, which would otherwise put "Finished" first).
    sort_shelf(&mut rows);
    let order: Vec<i64> = rows.iter().map(|r| r.id).collect();
    assert_eq!(order, vec![a_id, b_id, c_id], "in-progress, new, finished");

    // The 16.5g sort picker's keys reorder the same rows.
    use conservatory_core::db::{ShelfSort, sort_shelf_by};
    sort_shelf_by(&mut rows, ShelfSort::Title);
    assert_eq!(
        rows[0].id, c_id,
        "plain title order ignores state, so AAA Finished leads"
    );
    sort_shelf_by(&mut rows, ShelfSort::RecentlyPlayed);
    assert_eq!(
        rows.last().unwrap().id,
        b_id,
        "the never-played book sorts last under recency"
    );
    sort_shelf_by(&mut rows, ShelfSort::InProgress);
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![a_id, b_id, c_id],
        "the default key is sort_shelf itself"
    );

    worker.shutdown_ack().await.unwrap();
}

#[test]
fn book_state_derives_from_position_and_finished() {
    assert_eq!(BookState::derive(0.0, false), BookState::New);
    assert_eq!(BookState::derive(12.5, false), BookState::InProgress);
    assert_eq!(BookState::derive(0.0, true), BookState::Finished);
    // Finished wins even with a position (a re-listen resets it elsewhere).
    assert_eq!(BookState::derive(99.0, true), BookState::Finished);
}

#[tokio::test]
async fn queue_gained_the_book_foreign_key() {
    // Migration 0011 rebuilt `queue` to add the deferred `book_id` FK now that
    // `books` exists (the 0006 note). Confirm the structural change and that the
    // FK + CHECK actually enforce: an audiobook row needs a real book.
    let (_dir, worker, pool) = fresh().await;

    let referenced: Vec<String> = {
        let conn = pool.open().unwrap();
        let mut stmt = conn.prepare("PRAGMA foreign_key_list(queue)").unwrap();
        let rows: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>("table"))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows
    };
    assert!(
        referenced.iter().any(|t| t == "books"),
        "queue should reference books after 0011: {referenced:?}"
    );
    assert!(referenced.iter().any(|t| t == "tracks"), "{referenced:?}");
    assert!(referenced.iter().any(|t| t == "episodes"), "{referenced:?}");

    let book_id = worker
        .insert_book(sample_book(
            "Queued Book",
            "Audiobooks/A/Standalone/Queued Book",
        ))
        .await
        .unwrap();

    // A raw writer with FK enforcement on: a dangling book_id is rejected, a real
    // one is accepted.
    let conn = rusqlite::Connection::open(_dir.path().join("t.db")).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();

    let dangling = conn.execute(
        "INSERT INTO queue (position, kind, book_id) VALUES (0, 'audiobook', 999999)",
        [],
    );
    assert!(
        dangling.is_err(),
        "a queue row pointing at a missing book must be rejected"
    );

    conn.execute(
        "INSERT INTO queue (position, kind, book_id) VALUES (0, 'audiobook', ?1)",
        [book_id],
    )
    .expect("an audiobook queue row referencing a real book is accepted");

    worker.shutdown_ack().await.unwrap();
}
