//! Filesystem-touching tests for the Phase 8c cover-art audit: the dimension
//! decode (low-res) and the on-disk existence check (missing). The pure
//! tag/bitrate/ReplayGain tiers are unit-tested inside `src/audit.rs`.

use std::fs;

use conservatory_core::audit::audit_art;
use conservatory_core::db::AuditAlbumRow;
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
