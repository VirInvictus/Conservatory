//! Filesystem-touching tests for the Phase 8c cover-art audit: the dimension
//! decode (low-res) and the on-disk existence check (missing). The pure
//! tag/bitrate/ReplayGain tiers are unit-tested inside `src/audit.rs`.

use std::fs;

use conservatory_core::audit::{audit_ape, audit_art};
use conservatory_core::db::{AuditAlbumRow, AuditTrackRow};
use image::{Rgb, RgbImage};
use tempfile::tempdir;

fn album(album_id: i64, title: &str, cover_path: Option<&str>) -> AuditAlbumRow {
    AuditAlbumRow {
        album_id,
        artist: Some("Artist".into()),
        title: title.to_string(),
        cover_path: cover_path.map(str::to_string),
        folder_path: format!("Music/Artist/{title}"),
    }
}

fn write_png(path: &std::path::Path, w: u32, h: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    RgbImage::from_pixel(w, h, Rgb([0x40, 0x80, 0xC0]))
        .save(path)
        .unwrap();
}

#[test]
fn low_res_flags_small_cover_not_large() {
    let root = tempdir().unwrap();
    write_png(&root.path().join("small/cover.png"), 300, 300);
    write_png(&root.path().join("large/cover.png"), 600, 600);

    let albums = vec![
        album(1, "Small", Some("small/cover.png")),
        album(2, "Large", Some("large/cover.png")),
    ];
    let (missing, low_res) = audit_art(&albums, Some(root.path()), (500, 500), true);

    assert!(missing.is_empty(), "both covers exist on disk");
    assert_eq!(
        low_res.len(),
        1,
        "only the 300x300 cover is under the floor"
    );
    assert_eq!(low_res[0].album_id, 1);
    assert_eq!((low_res[0].width, low_res[0].height), (300, 300));
}

#[test]
fn missing_flags_null_and_absent_covers() {
    let root = tempdir().unwrap();
    write_png(&root.path().join("present/cover.png"), 600, 600);

    let albums = vec![
        album(1, "NullCover", None),
        album(2, "GoneCover", Some("present/missing.png")),
        album(3, "Present", Some("present/cover.png")),
    ];
    let (missing, _low_res) = audit_art(&albums, Some(root.path()), (500, 500), true);

    let ids: Vec<i64> = missing.iter().map(|m| m.album_id).collect();
    assert!(ids.contains(&1), "NULL cover_path is missing");
    assert!(ids.contains(&2), "a recorded-but-absent cover is missing");
    assert!(!ids.contains(&3), "the present cover is fine");
}

fn ape_footer(item_bytes: usize) -> Vec<u8> {
    let mut b = vec![0u8; 32];
    b[..8].copy_from_slice(b"APETAGEX");
    b[8..12].copy_from_slice(&2000u32.to_le_bytes());
    b[12..16].copy_from_slice(&((item_bytes + 32) as u32).to_le_bytes()); // items + footer
    b[16..20].copy_from_slice(&1u32.to_le_bytes()); // count
    // flags 0 => footer, no header
    b
}

fn track(track_id: i64, file_path: &str) -> AuditTrackRow {
    AuditTrackRow {
        track_id,
        album_id: Some(1),
        title: "t".into(),
        artist: Some("A".into()),
        track_no: Some(1),
        genre_count: 1,
        format: Some("mp3".into()),
        bitrate: Some(320),
        replaygain_track: None,
        replaygain_album: None,
        file_path: file_path.to_string(),
    }
}

#[test]
fn ape_tier_flags_only_files_with_a_stray_ape() {
    let root = tempdir().unwrap();
    // A clean mp3 (just fake audio).
    fs::write(root.path().join("clean.mp3"), vec![0xAAu8; 500]).unwrap();
    // An mp3 with a stray APE tag appended (items + footer).
    let mut tagged = vec![0xBBu8; 500];
    let items = vec![0x22u8; 20];
    tagged.extend_from_slice(&items);
    tagged.extend_from_slice(&ape_footer(items.len()));
    fs::write(root.path().join("tagged.mp3"), tagged).unwrap();

    let rows = vec![track(1, "clean.mp3"), track(2, "tagged.mp3")];
    let found = audit_ape(&rows, root.path());

    assert_eq!(found.len(), 1, "only the APE-tagged file flags");
    assert_eq!(found[0].track_id, 2);
}
