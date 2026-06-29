//! Phase 8c-iii strip tests. The byte-level splice (plan / restore) is tested
//! unconditionally on a synthesized buffer; the full crash-safe write path
//! (which includes a lofty decode check, so it needs a real MP3) runs only when
//! a testdata fixture is present, mirroring the availability-gated verify tests.

use std::fs;
use std::path::PathBuf;

use conservatory_core::ape::{commit_strip, locate_ape, plan_strip, restore_bytes, strip_bytes};
use tempfile::tempdir;

/// items + a footer, no header.
fn build_ape(items: &[u8]) -> Vec<u8> {
    let size = (items.len() + 32) as u32;
    let mut foot = vec![0u8; 32];
    foot[..8].copy_from_slice(b"APETAGEX");
    foot[8..12].copy_from_slice(&2000u32.to_le_bytes());
    foot[12..16].copy_from_slice(&size.to_le_bytes());
    foot[16..20].copy_from_slice(&1u32.to_le_bytes());
    let mut out = items.to_vec();
    out.extend_from_slice(&foot);
    out
}

#[test]
fn plan_and_restore_roundtrip_bytes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("track.mp3");

    let mut orig = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
    orig.extend_from_slice(&[0x55u8; 300]);
    let mut tagged = orig.clone();
    tagged.extend_from_slice(&build_ape(b"some-ape-items"));
    fs::write(&path, &tagged).unwrap();

    let plan = plan_strip(&path).unwrap().expect("ape present");
    assert_eq!(plan.stripped, orig, "strip yields the original prefix");
    assert_eq!(plan.orig_size, tagged.len() as u64);

    // Undo: re-splice the excised bytes into the stripped content.
    let restored = restore_bytes(&plan.stripped, &plan.ape_bytes, plan.tag_start).unwrap();
    assert_eq!(
        restored, tagged,
        "restore reproduces the tagged file exactly"
    );
}

#[test]
fn no_plan_when_no_ape() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("clean.mp3");
    fs::write(&path, vec![0xAAu8; 400]).unwrap();
    assert!(plan_strip(&path).unwrap().is_none());
}

/// A real MP3 from the gitignored testdata, if present.
fn fixture_mp3() -> Option<PathBuf> {
    let albums = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("testdata/albums");
    for album in fs::read_dir(&albums).ok()? {
        let album = album.ok()?.path();
        if !album.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&album).ok()? {
            let p = entry.ok()?.path();
            if p.extension().and_then(|e| e.to_str()) == Some("mp3") {
                return Some(p);
            }
        }
    }
    None
}

#[test]
fn full_strip_then_undo_on_real_mp3() {
    let Some(src) = fixture_mp3() else {
        eprintln!("skipping: no testdata mp3 fixture present");
        return;
    };
    let dir = tempdir().unwrap();
    let path = dir.path().join("real.mp3");

    // A real MP3 with a stray APE appended at the very end.
    let orig = fs::read(&src).unwrap();
    let mut tagged = orig.clone();
    tagged.extend_from_slice(&build_ape(b"stray-ape-tag"));
    fs::write(&path, &tagged).unwrap();

    // Strip: the file decodes afterwards and equals the original bytes.
    let plan = plan_strip(&path).unwrap().expect("ape present");
    commit_strip(&path, &plan.stripped).unwrap();
    let after = fs::read(&path).unwrap();
    assert_eq!(
        after, orig,
        "stripped file is byte-identical to the original"
    );
    assert!(locate_ape(&after).is_none(), "no APE remains");
    assert!(lofty_reads(&path), "stripped file still decodes");

    // Undo: re-splice the excised bytes; the tagged file comes back exactly.
    let restored = restore_bytes(&after, &plan.ape_bytes, plan.tag_start).unwrap();
    conservatory_core::ape::write_atomic_plain(&path, &restored).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        tagged,
        "undo restores the tagged file"
    );
}

fn lofty_reads(path: &std::path::Path) -> bool {
    lofty::read_from_path(path).is_ok()
}

// Keep `strip_bytes` referenced for the doc-link / API surface check.
#[test]
fn strip_bytes_matches_plan() {
    let mut data = b"ID3prefix".to_vec();
    data.extend_from_slice(&build_ape(b"x"));
    let span = locate_ape(&data).unwrap();
    assert_eq!(strip_bytes(&data, &span), b"ID3prefix");
}
