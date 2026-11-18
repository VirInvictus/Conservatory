//! Phase 2d end-to-end import tests. Uses the committed per-format tag fixtures
//! (copied into a temp source so the originals are never touched), imports them
//! into a temp library, and checks the managed tree + DB, the conflict guarantee,
//! and the shelf-genre-set → organize move (the sub-phase's usable artifact).

use std::fs;
use std::path::{Path, PathBuf};

use conservatory_core::db::{
    ReadPool, library_counts, list_albums, spawn_worker, track_render_rows,
};
use conservatory_core::{ImportOptions, MoveMode, import_folder};
use tempfile::{TempDir, tempdir};

/// Copy the four committed fixture files into a fresh temp dir (the import source).
fn fixture_source() -> TempDir {
    let dir = tempdir().unwrap();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    for name in ["sample.flac", "sample.mp3", "sample.opus", "sample.m4a"] {
        fs::copy(src.join(name), dir.path().join(name)).unwrap();
    }
    dir
}

struct Lib {
    _dbdir: TempDir,
    _libdir: TempDir,
    db: PathBuf,
    root: PathBuf,
}

fn lib() -> Lib {
    let dbdir = tempdir().unwrap();
    let libdir = tempdir().unwrap();
    Lib {
        db: dbdir.path().join("library.db"),
        root: libdir.path().to_path_buf(),
        _dbdir: dbdir,
        _libdir: libdir,
    }
}

async fn import_into(lib: &Lib, source: &Path, mode: MoveMode) -> conservatory_core::ImportReport {
    let worker = spawn_worker(lib.db.clone()).unwrap();
    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let opts = ImportOptions {
        library_root: lib.root.clone(),
        mode,
    };
    let report = import_folder(&worker, &pool, source, &opts).await.unwrap();
    worker.shutdown_ack().await.unwrap();
    report
}

#[tokio::test]
async fn imports_an_album_into_the_managed_tree() {
    let src = fixture_source();
    let lib = lib();

    let report = import_into(&lib, src.path(), MoveMode::Copy).await;

    assert_eq!(report.files_scanned, 4);
    assert_eq!(report.tracks, 4);
    assert_eq!(report.albums, 1);
    // Two distinct artists: the album artist and the (different) track artist.
    assert_eq!(report.artists, 2);
    assert!(report.job_id.is_some());
    assert!(report.conflicts.is_empty());

    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let conn = pool.open().unwrap();

    let counts = library_counts(&conn).unwrap();
    assert_eq!(counts.albums, 1);
    assert_eq!(counts.tracks, 4);

    let albums = list_albums(&conn).unwrap();
    // Genres across the four fixtures: Ambient on all four, Electronic on three
    // (ID3 collapsed the mp3 to Ambient), so the most-common shelf genre is Ambient.
    assert_eq!(albums[0].shelf_genre.as_deref(), Some("Ambient"));
    assert_eq!(albums[0].year, Some(2021));

    // Every track's DB path exists on disk under the managed tree, and the
    // originals are untouched (copy mode).
    for row in track_render_rows(&conn).unwrap() {
        assert!(
            lib.root.join(&row.file_path).exists(),
            "missing managed file {}",
            row.file_path
        );
        assert!(
            row.file_path
                .starts_with("Music/Ambient/Test Album Artist/Test Album (2021)/")
        );
    }
    assert_eq!(fs::read_dir(src.path()).unwrap().count(), 4, "sources kept");
}

#[tokio::test]
async fn move_mode_consumes_the_sources() {
    let src = fixture_source();
    let lib = lib();
    import_into(&lib, src.path(), MoveMode::Move).await;
    assert_eq!(
        fs::read_dir(src.path()).unwrap().count(),
        0,
        "move should consume sources"
    );
}

#[tokio::test]
async fn re_importing_the_same_album_is_refused_and_changes_nothing() {
    let src = fixture_source();
    let lib = lib();
    import_into(&lib, src.path(), MoveMode::Copy).await;

    // A second import renders the same targets, which now exist: refused.
    let src2 = fixture_source();
    let report = import_into(&lib, src2.path(), MoveMode::Copy).await;
    assert!(!report.conflicts.is_empty());
    assert!(report.job_id.is_none());

    // The library still has exactly one album / four tracks (nothing added).
    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let counts = library_counts(&pool.open().unwrap()).unwrap();
    assert_eq!(counts.albums, 1);
    assert_eq!(counts.tracks, 4);
}

#[tokio::test]
async fn shelf_genre_set_then_organize_moves_the_album() {
    let src = fixture_source();
    let lib = lib();
    import_into(&lib, src.path(), MoveMode::Copy).await;

    let worker = spawn_worker(lib.db.clone()).unwrap();
    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();

    let album_id = list_albums(&pool.open().unwrap()).unwrap()[0].id;
    worker
        .set_album_shelf_genre(album_id, "Jazz".to_string())
        .await
        .unwrap();

    // Re-render and move: build the ops from the DB and apply (the `organize` path).
    let rows = track_render_rows(&pool.open().unwrap()).unwrap();
    let template = conservatory_core::PathTemplate::default_music();
    let ops = rows
        .iter()
        .map(|row| {
            let fields = conservatory_core::TrackFields {
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
            conservatory_core::MoveOp {
                track_id: Some(row.track_id),
                album_id: row.album_id,
                book_id: None,
                src: lib.root.join(&row.file_path),
                dst: lib.root.join(&rel),
                db_old: Some(row.file_path.clone()),
                db_new: Some(rel.to_string_lossy().into_owned()),
            }
        })
        .collect();

    conservatory_core::mover::apply(
        &worker,
        &pool,
        conservatory_core::MoveKind::Organize,
        MoveMode::Move,
        &lib.root,
        0,
        ops,
    )
    .await
    .unwrap();
    // The cover follows its album (Phase 5d), as the real organize does, so the
    // old genre folder is left with no files.
    conservatory_core::resync_album_covers(&worker, &pool, &lib.root)
        .await
        .unwrap();
    worker.shutdown_ack().await.unwrap();

    // Every file now lives under Jazz/, and no files remain under Ambient/
    // (organize leaves empty directories behind; only files matter).
    for row in track_render_rows(&pool.open().unwrap()).unwrap() {
        assert!(
            row.file_path.starts_with("Music/Jazz/"),
            "{}",
            row.file_path
        );
        assert!(lib.root.join(&row.file_path).exists());
    }
    assert_eq!(
        count_files(&lib.root.join("Music/Ambient")),
        0,
        "no files left in Ambient/ (the cover follows the album, Phase 5d)"
    );
    assert_eq!(
        count_files(&lib.root.join("Music/Jazz")),
        5,
        "four audio files + the cover under Jazz/ (Phase 5d)"
    );
}

fn count_files(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() { count_files(&p) } else { 1 }
        })
        .sum()
}

/// The fixture source, but with the embedded covers stripped from the copies
/// (so the sibling sidecar is what the import finds) plus a `cover.jpg`
/// sidecar beside the audio. The committed fixtures always carry embedded art;
/// lofty removes it the same way write-back writes.
fn fixture_source_with_sidecar() -> TempDir {
    use lofty::file::AudioFile as _;
    use lofty::prelude::TaggedFileExt as _;

    let dir = fixture_source();
    for name in ["sample.flac", "sample.mp3"] {
        let path = dir.path().join(name);
        let mut tagged = lofty::read_from_path(&path).unwrap();
        if let Some(tag) = tagged.primary_tag_mut() {
            tag.remove_picture_type(lofty::picture::PictureType::CoverFront);
        }
        tagged
            .save_to_path(&path, lofty::config::WriteOptions::default())
            .unwrap();
    }
    fs::write(dir.path().join("cover.jpg"), b"stand-in sidecar bytes").unwrap();
    // The other two fixtures would still carry embedded art and win over the
    // sidecar; the sidecar test imports a two-file album.
    fs::remove_file(dir.path().join("sample.opus")).unwrap();
    fs::remove_file(dir.path().join("sample.m4a")).unwrap();
    dir
}

#[tokio::test]
async fn move_mode_journals_the_sidecar_cover_and_undo_restores_both() {
    let src = fixture_source_with_sidecar();
    let lib = lib();
    let worker = spawn_worker(lib.db.clone()).unwrap();
    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let opts = ImportOptions {
        library_root: lib.root.clone(),
        mode: MoveMode::Move,
    };
    let report = import_folder(&worker, &pool, src.path(), &opts)
        .await
        .unwrap();
    assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);

    // The regression (roadmap 2026-09-11): the sidecar used to be duplicated
    // into the managed tree and left behind in the consumed source folder.
    assert_eq!(
        fs::read_dir(src.path()).unwrap().count(),
        0,
        "move consumes the sidecar with the audio"
    );

    // The managed album folder carries the sidecar, and the album points at it.
    let conn = pool.open().unwrap();
    let albums = list_albums(&conn).unwrap();
    assert_eq!(albums.len(), 1);
    let cover_rel = albums[0].cover_path.clone().expect("cover recorded");
    drop(conn);
    let managed = lib.root.join(&cover_rel);
    assert_eq!(
        cover_rel,
        "Music/Ambient/Test Album Artist/Test Album (2021)/cover.jpg"
    );
    assert!(managed.exists(), "the sidecar file moved into the tree");

    // Undo restores both: the audio and the sidecar return to the source, the
    // managed copy is gone, and the album no longer points at a moved file.
    conservatory_core::mover::undo(&worker, &pool, report.job_id.unwrap())
        .await
        .unwrap();
    assert_eq!(
        count_files(src.path()),
        3,
        "two audio files + the sidecar restored"
    );
    assert!(!managed.exists(), "managed sidecar removed by undo");
    let conn = pool.open().unwrap();
    let albums = list_albums(&conn).unwrap();
    assert!(
        albums[0].cover_path.is_none(),
        "cover pointer cleared by undo, not left dangling"
    );
    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn copy_mode_keeps_the_sidecar_and_writes_the_canonical_cover() {
    let src = fixture_source_with_sidecar();
    let lib = lib();
    let report = import_into(&lib, src.path(), MoveMode::Copy).await;
    assert_eq!(report.tracks, 2);

    // Copy mode consumes nothing: audio and sidecar stay in the source.
    assert_eq!(count_files(src.path()), 3, "sources kept in copy mode");

    // The managed folder carries the canonical written copy (the old path).
    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let conn = pool.open().unwrap();
    let albums = list_albums(&conn).unwrap();
    let cover_rel = albums[0].cover_path.clone().expect("cover recorded");
    assert!(lib.root.join(&cover_rel).exists());
}

/// Re-tag a copied fixture file with its own tags minus the year (issue #7's
/// year-less copy).
fn strip_year(path: &Path) {
    let draft = conservatory_core::read_track(path).unwrap();
    conservatory_core::write_track_tags(
        path,
        &conservatory_core::TagWrite {
            title: draft.title.clone().unwrap_or_else(|| "Untitled".into()),
            track_artist: draft.artist.clone(),
            track_artist_sort: draft.artist_sort.clone(),
            album: draft.album.clone(),
            album_artist: draft.album_artist.clone(),
            album_artist_sort: draft.album_artist_sort.clone(),
            year: None,
            track_no: draft.track_no,
            disc_no: draft.disc_no,
            genres: draft.genres.clone(),
            credits: draft.credits.clone(),
        },
    )
    .unwrap();
}

/// Retag a copied fixture file in place: same album and album artist as the
/// fixture's own tags, with the given title / track number / year / genres.
/// The vehicle for the two-copy album tests below (issue #7).
fn retag_as_second_copy(path: &Path, year: Option<i32>, genres: &[&str]) {
    let draft = conservatory_core::read_track(path).unwrap();
    conservatory_core::write_track_tags(
        path,
        &conservatory_core::TagWrite {
            title: "Second Track".into(),
            track_artist: draft.artist.clone(),
            track_artist_sort: draft.artist_sort.clone(),
            album: draft.album.clone(),
            album_artist: draft.album_artist.clone(),
            album_artist_sort: draft.album_artist_sort.clone(),
            year,
            track_no: Some(9),
            disc_no: draft.disc_no,
            genres: genres.iter().map(|g| g.to_string()).collect(),
            credits: Vec::new(),
        },
    )
    .unwrap();
}

/// Issue #7, the gate's shape: one album shelved under two genre folders, the
/// copy with the year tag imported first. Both copies resolve to one `albums`
/// row, and the second copy must render from the ROW's fields, landing beside
/// the first — not in its own `Test Album/` folder that the row does not
/// render. A fresh organize then has nothing to do: the tree an import leaves
/// is what the engine renders.
#[tokio::test]
async fn two_genre_copies_of_one_album_render_one_tree() {
    let src = fixture_source();
    let lib = lib();
    import_into(&lib, src.path(), MoveMode::Move).await;

    // Copy B: same album + album artist, the year stripped, a different shelf
    // genre, a different title — no collision with copy A.
    let src2 = tempdir().unwrap();
    let extra = src2.path().join("extra.flac");
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/sample.flac"),
        &extra,
    )
    .unwrap();
    retag_as_second_copy(&extra, None, &["Post-Rock"]);

    let report = import_into(&lib, src2.path(), MoveMode::Move).await;
    assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
    assert_eq!(report.tracks, 1);

    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let conn = pool.open().unwrap();
    let counts = library_counts(&conn).unwrap();
    assert_eq!(counts.albums, 1, "one album row, not one per copy");
    assert_eq!(counts.tracks, 5);

    let rows = track_render_rows(&conn).unwrap();
    let second = rows.iter().find(|r| r.title == "Second Track").unwrap();
    assert!(
        second
            .file_path
            .starts_with("Music/Ambient/Test Album Artist/Test Album (2021)/"),
        "the matched group rendered from the row: {}",
        second.file_path
    );
    assert!(
        lib.root.join(&second.file_path).exists(),
        "the file is where the row renders it"
    );

    // The end-to-end guard: organize re-renders from the same row and plans
    // nothing (every op is a src == dst skip).
    let ops = conservatory_core::organize_ops(&rows, &lib.root, None);
    let plan = conservatory_core::plan(ops);
    assert!(plan.conflicts.is_empty(), "{:?}", plan.conflicts);
    assert_eq!(
        plan.ops.len(),
        0,
        "no surprise relocations on a just-imported library"
    );
}

/// Issue #7's other side: the year-less copy arrives FIRST, so the row is
/// created without a year and copy A's tree has no `(<year>)` group. The
/// year-tagged copy's import fills the row's NULL hole (the writer's COALESCE
/// backfill) and renders from the upgraded row; the next organize then
/// converges copy A's files into the upgraded folder — a bounded, explainable
/// relocate (the same flow as a year edit), not the unbounded split.
#[tokio::test]
async fn reimport_backfills_the_matched_rows_null_year() {
    let src = fixture_source();
    for name in ["sample.flac", "sample.mp3", "sample.opus", "sample.m4a"] {
        strip_year(&src.path().join(name));
    }
    let lib = lib();
    import_into(&lib, src.path(), MoveMode::Move).await;

    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let year_before = list_albums(&pool.open().unwrap()).unwrap()[0].year;
    assert_eq!(year_before, None, "the year-less copy created the row");

    let src2 = tempdir().unwrap();
    let extra = src2.path().join("extra.flac");
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/sample.flac"),
        &extra,
    )
    .unwrap();
    retag_as_second_copy(&extra, Some(2021), &["Post-Rock"]);

    let report = import_into(&lib, src2.path(), MoveMode::Move).await;
    assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);

    let conn = pool.open().unwrap();
    let album = &list_albums(&conn).unwrap()[0];
    assert_eq!(album.year, Some(2021), "the row's NULL year was backfilled");

    // The second copy sits in the upgraded folder; the first copy's files
    // still sit in the old one, and exactly they are what a fresh organize
    // relocates (bounded: one album, one pass).
    let rows = track_render_rows(&conn).unwrap();
    let second = rows.iter().find(|r| r.title == "Second Track").unwrap();
    assert!(second.file_path.contains("Test Album (2021)"));
    let ops = conservatory_core::organize_ops(&rows, &lib.root, None);
    let plan = conservatory_core::plan(ops);
    assert!(plan.conflicts.is_empty(), "{:?}", plan.conflicts);
    assert_eq!(plan.ops.len(), 4, "only the first copy's four tracks move");
    for op in &plan.ops {
        assert!(
            op.db_old
                .as_deref()
                .is_some_and(|old| old.contains("Test Album/")),
            "the relocate targets come from the year-less tree: {:?}",
            op.db_old
        );
    }
}

/// Issue #9: a move-mode import consumes the sibling `.lrc` beside its audio
/// (the lyrics reader derives the sidecar path from the managed audio path),
/// and undo brings both back. The gate's source copy held 9.5k of these
/// strays in otherwise-consumed folders.
#[tokio::test]
async fn move_mode_journals_lrc_sidecars_and_undo_restores_them() {
    let src = fixture_source();
    fs::write(src.path().join("sample.lrc"), "[00:01.00]hello\n").unwrap();
    let lib = lib();
    let worker = spawn_worker(lib.db.clone()).unwrap();
    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let opts = ImportOptions {
        library_root: lib.root.clone(),
        mode: MoveMode::Move,
    };
    let report = import_folder(&worker, &pool, src.path(), &opts)
        .await
        .unwrap();
    assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
    assert_eq!(
        fs::read_dir(src.path()).unwrap().count(),
        0,
        "move consumes the audio with its lyrics"
    );

    // The managed tree carries the .lrc beside the audio (the rendered file
    // name is the track title's, so find the sidecar by existence), and the
    // lyrics reader finds it through the managed path (the consumer the fix
    // serves).
    let conn = pool.open().unwrap();
    let rows = track_render_rows(&conn).unwrap();
    drop(conn);
    let audio_path = rows
        .iter()
        .map(|r| lib.root.join(&r.file_path))
        .find(|p| p.with_extension("lrc").exists())
        .expect("one imported track carries its .lrc");
    let managed_lrc = audio_path.with_extension("lrc");
    assert!(managed_lrc.exists(), "the sidecar moved with its track");
    let lyrics = conservatory_core::lyrics::load_for(&audio_path).unwrap();
    match lyrics {
        conservatory_core::lyrics::Lyrics::Synced(lines) => {
            assert_eq!(lines[0].text, "hello");
        }
        other => panic!("expected synced lyrics from the moved sidecar: {other:?}"),
    }

    // Undo restores the audio and the sidecar together.
    conservatory_core::mover::undo(&worker, &pool, report.job_id.unwrap())
        .await
        .unwrap();
    assert_eq!(
        count_files(src.path()),
        5,
        "four audio files + the .lrc restored"
    );
    assert!(!managed_lrc.exists(), "the managed sidecar moved back out");
    worker.shutdown_ack().await.unwrap();
}

/// Copy mode's contract is unchanged (the issue is move-mode): sources stay,
/// and no `.lrc` appears in the managed tree (the lyrics ride the source).
#[tokio::test]
async fn copy_mode_leaves_lrc_sources_in_place() {
    fn count_lrc(dir: &Path) -> usize {
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|e| {
                let p = e.path();
                if p.is_dir() {
                    count_lrc(&p)
                } else if p.extension().is_some_and(|x| x == "lrc") {
                    1
                } else {
                    0
                }
            })
            .sum()
    }

    let src = fixture_source();
    fs::write(src.path().join("sample.lrc"), "[00:01.00]hello\n").unwrap();
    let lib = lib();
    let report = import_into(&lib, src.path(), MoveMode::Copy).await;
    assert_eq!(report.tracks, 4);
    assert_eq!(count_files(src.path()), 5, "copy consumes nothing");
    assert_eq!(count_lrc(&lib.root), 0, "no .lrc in the managed tree");
}
