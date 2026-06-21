//! Phase 5a integration tests: field edits through the single-writer worker,
//! FTS sync on rename, raw-genre relink, and a path-affecting edit that
//! re-renders and moves (then undoes). DB-only cases run on the synthetic
//! fixtures; the move case imports the committed audio fixtures. CI stays
//! hermetic (the gitignored `testdata/` real albums are for manual checks).

use std::path::PathBuf;

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{ReadPool, get_album, get_track, spawn_worker, track_render_rows};
use conservatory_core::mover::{self, MoveKind, MoveMode, MoveOp};
use conservatory_core::{
    AlbumEdit, ImportOptions, PathTemplate, TrackEdit, TrackFields, import_folder,
};
use tempfile::tempdir;

fn fixture_audio(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

async fn synthetic_lib() -> (
    tempfile::TempDir,
    ReadPool,
    conservatory_core::db::WorkerHandle,
) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();
    (dir, pool, worker)
}

#[tokio::test]
async fn update_track_fields_and_fts_follow() {
    let (_dir, pool, worker) = synthetic_lib().await;

    worker
        .update_track(
            1,
            TrackEdit {
                title: Some("Zzz Edited Title".into()),
                rating: Some(5),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let conn = pool.open().unwrap();
    let t = get_track(&conn, 1).unwrap().unwrap();
    assert_eq!(t.title, "Zzz Edited Title");
    assert_eq!(t.rating, 5);

    let fts_title: String = conn
        .query_row("SELECT title FROM track_fts WHERE rowid = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        fts_title, "Zzz Edited Title",
        "FTS must follow a title edit"
    );
}

#[tokio::test]
async fn edit_track_artist_reassigns_and_fts_follows() {
    let (_dir, pool, worker) = synthetic_lib().await;

    worker
        .update_track(
            1,
            TrackEdit {
                artist: Some("New Artist Person".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let conn = pool.open().unwrap();
    let t = get_track(&conn, 1).unwrap().unwrap();
    let name: String = conn
        .query_row(
            "SELECT name FROM artists WHERE id = ?1",
            [t.artist_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "New Artist Person");

    let fts_artist: String = conn
        .query_row("SELECT artist FROM track_fts WHERE rowid = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fts_artist, "New Artist Person");
}

#[tokio::test]
async fn update_album_fields_and_fts_follow() {
    let (_dir, pool, worker) = synthetic_lib().await;

    worker
        .update_album(
            1,
            AlbumEdit {
                title: Some("New Album Name".into()),
                year: Some(1999),
                album_artist: Some("AA Person".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let conn = pool.open().unwrap();
    let alb = get_album(&conn, 1).unwrap().unwrap();
    assert_eq!(alb.title, "New Album Name");
    assert_eq!(alb.year, Some(1999));

    let (ft, faa): (String, String) = conn
        .query_row(
            "SELECT title, album_artist FROM album_fts WHERE rowid = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(ft, "New Album Name");
    assert_eq!(faa, "AA Person");

    // The albums_au trigger also fixes the denormalized album column on tracks.
    let track_album: String = conn
        .query_row(
            "SELECT album FROM track_fts
             WHERE rowid IN (SELECT id FROM tracks WHERE album_id = 1) LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(track_album, "New Album Name");
}

#[tokio::test]
async fn set_track_genres_replaces_not_appends() {
    let (_dir, pool, worker) = synthetic_lib().await;

    worker
        .set_track_genres(1, vec!["Jazz".into(), "Fusion".into()])
        .await
        .unwrap();
    let names = genres_of(&pool, 1);
    assert_eq!(names, vec!["Fusion".to_string(), "Jazz".to_string()]);

    // A second set replaces the whole set (does not accumulate).
    worker
        .set_track_genres(1, vec!["Ambient".into()])
        .await
        .unwrap();
    assert_eq!(genres_of(&pool, 1), vec!["Ambient".to_string()]);
}

fn genres_of(pool: &ReadPool, track_id: i64) -> Vec<String> {
    let conn = pool.open().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
             WHERE tg.track_id = ?1 ORDER BY g.name",
        )
        .unwrap();
    stmt.query_map([track_id], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[tokio::test]
async fn path_affecting_edit_re_renders_moves_and_undoes() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("lib.db");
    let lib = dir.path().join("lib");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    for f in ["sample.flac", "sample.mp3", "sample.m4a", "sample.opus"] {
        std::fs::copy(fixture_audio(f), src.join(f)).unwrap();
    }

    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db.clone(), 3).unwrap();
    let report = import_folder(
        &worker,
        &pool,
        &src,
        &ImportOptions {
            library_root: lib.clone(),
            mode: MoveMode::Copy,
        },
    )
    .await
    .unwrap();
    assert!(report.conflicts.is_empty());
    assert!(tree_has(&lib, "(2021)"), "fixtures import under year 2021");

    // A path-affecting edit (year) then organize re-renders and moves.
    worker
        .update_album(
            1,
            AlbumEdit {
                year: Some(1999),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let job = organize_all(&worker, &pool, &lib).await;
    assert!(tree_has(&lib, "(1999)"), "the album moved to its new year");

    // Undo reverts both the tree and the DB path.
    mover::undo(&worker, &pool, job).await.unwrap();
    assert!(tree_has(&lib, "(2021)"), "undo restored the original path");
}

/// Re-render every track from the DB and move to match (the `organize` flow).
async fn organize_all(
    worker: &conservatory_core::db::WorkerHandle,
    pool: &ReadPool,
    root: &std::path::Path,
) -> i64 {
    let rows = {
        let conn = pool.open().unwrap();
        track_render_rows(&conn).unwrap()
    };
    let template = PathTemplate::default_music();
    let ops: Vec<MoveOp> = rows
        .iter()
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
                src: root.join(&row.file_path),
                dst: root.join(&rel),
                db_old: Some(row.file_path.clone()),
                db_new: Some(rel.to_string_lossy().into_owned()),
            }
        })
        .collect();
    mover::apply(
        worker,
        pool,
        MoveKind::Organize,
        MoveMode::Move,
        root,
        0,
        ops,
    )
    .await
    .unwrap()
}

/// Whether any file under `root` has a path component containing `needle`.
fn tree_has(root: &std::path::Path, needle: &str) -> bool {
    fn walk(dir: &std::path::Path, needle: &str) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.to_string_lossy().contains(needle) {
                return true;
            }
            if p.is_dir() && walk(&p, needle) {
                return true;
            }
        }
        false
    }
    walk(root, needle)
}
