//! Cover art on disk (Phase 5d, spec §7.4).
//!
//! The database owns organization, but the cover is written into each managed
//! album folder as `cover.jpg` (or `cover.png`) so the library is portable and
//! the Now-bar / MPRIS art have a file to point at. Covers are *derived* (always
//! re-extractable from embedded art), so they are synced idempotently rather
//! than journaled by the mover: the trust-critical move/undo machinery (§5.4) is
//! left untouched, and a crash mid-sync just re-derives.

use std::collections::HashMap;
use std::path::Path;

use crate::db::{ReadPool, WorkerHandle, list_albums, track_render_rows};
use crate::errors::Result;
use crate::tags::read_track;

/// The canonical cover filename for some image bytes: PNG keeps its extension,
/// everything else is treated as JPEG (the embedded-art norm).
fn cover_filename(bytes: &[u8]) -> &'static str {
    const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.starts_with(PNG_MAGIC) {
        "cover.png"
    } else {
        "cover.jpg"
    }
}

/// Write cover `bytes` into `album_dir`, returning the filename written. Removes
/// the other-extension cover so an album keeps a single canonical cover.
pub fn write_cover(album_dir: &Path, bytes: &[u8]) -> Result<&'static str> {
    let name = cover_filename(bytes);
    let other = if name == "cover.png" {
        "cover.jpg"
    } else {
        "cover.png"
    };
    let _ = std::fs::remove_file(album_dir.join(other));
    std::fs::write(album_dir.join(name), bytes)?;
    tracing::debug!(target: "conservatory::io", dir = %album_dir.display(), name, bytes = bytes.len(), "covers: write");
    Ok(name)
}

/// Ensure the album at `album_folder_rel` (relative to `root`) has its cover on
/// disk, returning the new `cover_path` (also root-relative). A stale cover at
/// `old_cover_path` (a different location, e.g. before a move) is removed.
/// Idempotent.
pub fn sync_album_cover(
    root: &Path,
    album_folder_rel: &str,
    bytes: &[u8],
    old_cover_path: Option<&str>,
) -> Result<String> {
    let dir = root.join(album_folder_rel);
    std::fs::create_dir_all(&dir)?;
    let name = write_cover(&dir, bytes)?;
    // Build the root-relative path with `Path::join` (handles a trailing slash
    // and never produces a leading-slash "absolute" string from an odd folder).
    let new_rel = Path::new(album_folder_rel)
        .join(name)
        .to_string_lossy()
        .into_owned();
    if let Some(old) = old_cover_path
        && old != new_rel
    {
        let _ = std::fs::remove_file(root.join(old));
    }
    Ok(new_rel)
}

/// Ensure every album's cover is in its current folder and `cover_path` matches
/// (Phase 5d). Idempotent; run after an organize/move so covers follow their
/// albums. Cover bytes come from the existing cover file (even at its old
/// location, since the mover does not move it), else a track's embedded art.
/// Best-effort per album; returns how many `cover_path`s changed.
pub async fn resync_album_covers(
    worker: &WorkerHandle,
    pool: &ReadPool,
    root: &Path,
) -> Result<usize> {
    let (albums, rows) = {
        let conn = pool.open()?;
        (list_albums(&conn)?, track_render_rows(&conn)?)
    };
    // One track file per album, for the embedded-art fallback.
    let mut a_track: HashMap<i64, String> = HashMap::new();
    for r in &rows {
        if let Some(aid) = r.album_id {
            a_track.entry(aid).or_insert_with(|| r.file_path.clone());
        }
    }

    let mut updated = 0;
    for album in &albums {
        let folder = &album.folder_path;
        if folder.is_empty() {
            continue;
        }
        // Already in place: skip (no rewrite churn on a no-op organize).
        if let Some(cp) = &album.cover_path
            && cp.starts_with(&format!("{folder}/"))
            && root.join(cp).exists()
        {
            continue;
        }
        // Bytes from the existing cover file (possibly at its old path), else
        // embedded art from a track.
        let bytes = match &album.cover_path {
            Some(cp) if root.join(cp).exists() => std::fs::read(root.join(cp)).ok(),
            _ => None,
        }
        .or_else(|| {
            a_track
                .get(&album.id)
                .and_then(|fp| read_track(&root.join(fp)).ok())
                .and_then(|d| d.cover.map(|c| c.data))
        });
        let Some(bytes) = bytes else { continue };

        let new_cp = sync_album_cover(root, folder, &bytes, album.cover_path.as_deref())?;
        if album.cover_path.as_deref() != Some(new_cp.as_str()) {
            worker
                .set_album_cover_path(album.id, Some(new_cp), None)
                .await?;
            updated += 1;
        }
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_by_magic() {
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0];
        let jpg = [0xff, 0xd8, 0xff, 0xe0, 0, 0];
        assert_eq!(cover_filename(&png), "cover.png");
        assert_eq!(cover_filename(&jpg), "cover.jpg");
    }

    #[test]
    fn sync_writes_and_clears_stale() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Old/Album")).unwrap();
        let jpg = [0xff, 0xd8, 0xff, 0xe0, 1, 2, 3];
        let old = sync_album_cover(root, "Old/Album", &jpg, None).unwrap();
        assert_eq!(old, "Old/Album/cover.jpg");
        assert!(root.join(&old).exists());

        // Moving the album removes the stale cover and writes the new one.
        let new = sync_album_cover(root, "New/Album", &jpg, Some(&old)).unwrap();
        assert_eq!(new, "New/Album/cover.jpg");
        assert!(root.join(&new).exists());
        assert!(!root.join(&old).exists(), "stale cover removed");
    }
}
