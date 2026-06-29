//! Phase 8a integration tests: the decode-verify path against real `flac` /
//! `ffmpeg` (availability-gated, the rsgain/GTK-display precedent) and the
//! path-keyed verify cache round-trip through the single-writer worker (no
//! binaries needed).

use std::path::PathBuf;

use conservatory_core::db::{
    ReadPool, VerifyResultRow, corrupt_or_suspect, read_verify_results, spawn_worker,
};
use conservatory_core::verify::{VerifyVerdict, ffmpeg_available, flac_available, verify_file};
use tempfile::tempdir;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

#[test]
fn flac_clean_passes_and_truncated_is_corrupt() {
    if !flac_available() {
        eprintln!("skipping flac_clean_passes_and_truncated_is_corrupt: flac not on PATH");
        return;
    }
    let dir = tempdir().unwrap();
    let src = fixture("sample.flac");

    // A clean copy verifies OK.
    let good = dir.path().join("good.flac");
    std::fs::copy(&src, &good).unwrap();
    let (verdict, _) = verify_file(&good, Some("flac")).unwrap();
    assert_eq!(verdict, VerifyVerdict::Ok, "the clean fixture decodes OK");

    // The same bytes cut in half: `flac -t` fails (truncation / bad metadata), so
    // the file is CORRUPT and carries the tool's first error line as detail.
    let data = std::fs::read(&src).unwrap();
    let bad = dir.path().join("bad.flac");
    std::fs::write(&bad, &data[..data.len() / 2]).unwrap();
    let (verdict, detail) = verify_file(&bad, Some("flac")).unwrap();
    assert_eq!(
        verdict,
        VerifyVerdict::Corrupt,
        "a truncated FLAC is corrupt"
    );
    assert!(detail.is_some(), "a corrupt verdict carries a detail line");
}

#[test]
fn ffmpeg_clean_mp3_passes() {
    if !ffmpeg_available() {
        eprintln!("skipping ffmpeg_clean_mp3_passes: ffmpeg not on PATH");
        return;
    }
    let dir = tempdir().unwrap();
    let good = dir.path().join("good.mp3");
    std::fs::copy(fixture("sample.mp3"), &good).unwrap();
    let (verdict, _) = verify_file(&good, Some("mp3")).unwrap();
    assert_eq!(verdict, VerifyVerdict::Ok, "the clean mp3 decodes OK");
}

#[tokio::test]
async fn verify_cache_round_trips_and_overwrites() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("library.db");
    let worker = spawn_worker(db.clone()).unwrap();

    // Store a CORRUPT verdict for a file.
    let row = VerifyResultRow {
        file_path: "Music/X/Album/01.flac".to_string(),
        file_size: 100,
        file_mtime: 5,
        verdict: VerifyVerdict::Corrupt,
        detail: Some("ERROR while decoding".to_string()),
        checked_at: 9,
    };
    worker
        .upsert_verify_results(vec![row.clone()])
        .await
        .unwrap();

    let pool = ReadPool::new(db, 2).unwrap();
    let conn = pool.open().unwrap();
    let map = read_verify_results(&conn, std::slice::from_ref(&row.file_path)).unwrap();
    let got = map.get(&row.file_path).expect("the row reads back");
    assert_eq!(got.verdict, VerifyVerdict::Corrupt);
    assert_eq!(got.file_size, 100);
    assert_eq!(got.detail.as_deref(), Some("ERROR while decoding"));
    assert_eq!(
        corrupt_or_suspect(&conn).unwrap().len(),
        1,
        "it shows in the report"
    );
    drop(conn);

    // Re-verifying after a fix (new size/mtime) overwrites the same path's row.
    let fixed = VerifyResultRow {
        file_path: row.file_path.clone(),
        file_size: 200,
        file_mtime: 6,
        verdict: VerifyVerdict::Ok,
        detail: None,
        checked_at: 10,
    };
    worker.upsert_verify_results(vec![fixed]).await.unwrap();

    let conn = pool.open().unwrap();
    let map = read_verify_results(&conn, std::slice::from_ref(&row.file_path)).unwrap();
    let got = map.get(&row.file_path).unwrap();
    assert_eq!(
        got.verdict,
        VerifyVerdict::Ok,
        "the verdict was overwritten"
    );
    assert_eq!(got.file_size, 200, "the staleness key was overwritten");
    assert!(
        corrupt_or_suspect(&conn).unwrap().is_empty(),
        "the now-OK file no longer shows in the report"
    );

    worker.shutdown_ack().await.unwrap();
}
