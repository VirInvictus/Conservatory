//! Phase 2a integration: render target paths from real DB rows. The exhaustive
//! template/sanitization coverage lives in the `path_template` unit tests; this
//! checks the DB read helper (`track_render_rows`) feeds the engine correctly,
//! which is the sub-phase's "given DB rows" usable artifact.

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{ReadPool, spawn_worker, track_render_rows};
use conservatory_core::{PathTemplate, TrackFields, find_collisions};
use tempfile::tempdir;

fn fields<'a>(row: &'a conservatory_core::db::TrackRenderRow) -> TrackFields<'a> {
    TrackFields {
        shelf_genre: row.shelf_genre.as_deref(),
        albumartist: row.album_artist_sort.as_deref(),
        album: row.album.as_deref(),
        year: row.year,
        track_no: row.track_no,
        disc_no: row.disc_no,
        title: Some(row.title.as_str()),
        artist: row.track_artist.as_deref(),
        ext: row.format.as_deref(),
    }
}

#[tokio::test]
async fn fixture_library_renders_unique_paths() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");

    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();

    let pool = ReadPool::new(path, 3).unwrap();
    let conn = pool.open().unwrap();
    let rows = track_render_rows(&conn).unwrap();
    assert!(!rows.is_empty(), "fixture should have tracks");

    let template = PathTemplate::default_music();
    let paths: Vec<_> = rows.iter().map(|r| template.render(&fields(r))).collect();

    // The fixture is well-formed (distinct albums, sequential tracks), so every
    // rendered path is unique.
    assert!(
        find_collisions(&paths).is_empty(),
        "fixture rendered colliding paths"
    );

    // Spot-check the rendered shape: genre / artist / album (year) / NN - title.ext.
    let sample = paths[0].to_string_lossy();
    assert!(sample.ends_with(".flac"), "{sample}");
    assert_eq!(sample.matches('/').count(), 3, "{sample}");

    worker.shutdown_ack().await.unwrap();
}
