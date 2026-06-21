//! Phase 5c tests: the ReplayGain DB-sync half is hermetic (write a known tag,
//! read it back, feed the profile); the rsgain scan itself is a skip-if-absent
//! integration test (the libmpv-smoke precedent), since rsgain is an external
//! tool that may be missing in CI.

use std::path::PathBuf;

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{ReadPool, get_track, spawn_worker};
use conservatory_core::{
    DEFAULT_TARGET_LUFS, PlaybackConfig, ReplayGain, replaygain_from_file, resolve_music_profile,
    rsgain_available, scan_album_files,
};
use lofty::config::WriteOptions;
use lofty::prelude::{AudioFile, ItemKey, TaggedFileExt};
use tempfile::tempdir;

fn fixture_audio(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

#[test]
fn replaygain_from_file_reads_written_tags() {
    let dir = tempdir().unwrap();
    let flac = dir.path().join("s.flac");
    std::fs::copy(fixture_audio("sample.flac"), &flac).unwrap();
    {
        let mut tagged = lofty::read_from_path(&flac).unwrap();
        let tag = tagged.primary_tag_mut().expect("flac fixture has a tag");
        tag.insert_text(ItemKey::ReplayGainTrackGain, "-6.00 dB".into());
        tag.insert_text(ItemKey::ReplayGainAlbumGain, "-7.00 dB".into());
        tagged.save_to_path(&flac, WriteOptions::default()).unwrap();
    }
    let (track_gain, album_gain) = replaygain_from_file(&flac).unwrap();
    assert_eq!(track_gain, Some(-6.0));
    assert_eq!(album_gain, Some(-7.0));
}

#[tokio::test]
async fn worker_set_replaygain_feeds_the_profile() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    worker
        .set_track_replaygain(1, Some(-6.0), Some(-7.0))
        .await
        .unwrap();
    let track = {
        let conn = pool.open().unwrap();
        get_track(&conn, 1).unwrap().unwrap()
    };
    assert_eq!(track.replaygain_track, Some(-6.0));
    assert_eq!(track.replaygain_album, Some(-7.0));

    // With album gain present, the default (album) profile stays album.
    let profile = resolve_music_profile(&track, &PlaybackConfig::default());
    assert_eq!(profile.replaygain, ReplayGain::Album);
    worker.shutdown_ack().await.unwrap();
}

#[test]
fn rsgain_scan_writes_gain_when_available() {
    if !rsgain_available() {
        eprintln!("skipping rsgain scan test: rsgain not on PATH");
        return;
    }
    let dir = tempdir().unwrap();
    let mut files = Vec::new();
    // FLAC + Opus, proving rsgain handles Opus (the reason it was chosen).
    for f in ["sample.flac", "sample.opus"] {
        let p = dir.path().join(f);
        std::fs::copy(fixture_audio(f), &p).unwrap();
        files.push(p);
    }
    scan_album_files(&files, DEFAULT_TARGET_LUFS).expect("rsgain scan");
    let (track_gain, _album) = replaygain_from_file(&files[0]).unwrap();
    assert!(track_gain.is_some(), "rsgain wrote a track gain");
}
