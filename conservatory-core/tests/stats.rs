//! Filesystem-touching test for the Phase 8c-ii statistics size pass: the
//! `stat()` over each track's file under the root. The pure aggregation is
//! unit-tested inside `src/stats.rs`.

use std::fs;

use conservatory_core::compute_stats;
use conservatory_core::db::{LibraryCounts, StatsTrackRow};
use tempfile::tempdir;

fn trow(format: &str, file_path: &str) -> StatsTrackRow {
    StatsTrackRow {
        format: Some(format.to_string()),
        bitrate: Some(320),
        duration: Some(100.0),
        rating: 0,
        file_path: file_path.to_string(),
        title: "t".into(),
        artist: Some("A".into()),
        track_no: Some(1),
        genre_count: 1,
    }
}

#[test]
fn size_pass_sums_per_format_and_total() {
    let root = tempdir().unwrap();
    // Two mp3s (10 + 20 bytes) and one flac (100 bytes).
    fs::write(root.path().join("a.mp3"), vec![0u8; 10]).unwrap();
    fs::write(root.path().join("b.mp3"), vec![0u8; 20]).unwrap();
    fs::write(root.path().join("c.flac"), vec![0u8; 100]).unwrap();

    let tracks = vec![
        trow("mp3", "a.mp3"),
        trow("mp3", "b.mp3"),
        trow("flac", "c.flac"),
    ];
    let counts = LibraryCounts {
        artists: 1,
        albums: 1,
        tracks: 3,
    };
    let stats = compute_stats(&tracks, &[], counts, Some(root.path()));

    assert_eq!(stats.total_size_bytes, Some(130));
    let mp3 = stats.formats.iter().find(|f| f.format == "mp3").unwrap();
    assert_eq!(mp3.size_bytes, Some(30));
    let flac = stats.formats.iter().find(|f| f.format == "flac").unwrap();
    assert_eq!(flac.size_bytes, Some(100));
}

#[test]
fn size_none_without_root() {
    let tracks = vec![trow("mp3", "a.mp3")];
    let counts = LibraryCounts {
        artists: 1,
        albums: 1,
        tracks: 1,
    };
    let stats = compute_stats(&tracks, &[], counts, None);
    assert_eq!(stats.total_size_bytes, None);
    assert!(stats.formats[0].size_bytes.is_none());
}
