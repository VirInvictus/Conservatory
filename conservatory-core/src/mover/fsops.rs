//! The per-file move primitive: idempotent, crash-safe, cross-filesystem aware.
//!
//! `relocate` tries a same-filesystem `rename` (atomic) first, and on a
//! cross-device error falls back to **copy → fsync → verify → delete source**
//! (the copy step models Atrium's `write_atomic`: a same-dir temp file, fsynced,
//! then renamed into place). Every operation is **idempotent**: re-running a
//! completed move is a no-op. That is what makes roll-forward replay safe across
//! a crash between the file move and the journal-complete write (docs/mover.md).

use std::fs::{self, File};
use std::io;
use std::path::Path;

use super::MoveMode;

/// Temp suffix for the cross-filesystem copy, so a crash mid-copy leaves a
/// recognisable partial file next to the destination rather than a half-written
/// destination.
const PART_SUFFIX: &str = ".conservatory-part";

/// Move or copy `src` to `dst` per `mode`. Idempotent: if `src` is gone and a
/// valid `dst` already exists, the operation already completed and this is a
/// no-op success.
pub fn relocate(src: &Path, dst: &Path, mode: MoveMode) -> io::Result<()> {
    if src == dst {
        return Ok(());
    }

    match fs::symlink_metadata(src) {
        Ok(meta) => {
            ensure_parent(dst)?;
            match mode {
                MoveMode::Move => move_one(src, dst, meta.len()),
                MoveMode::Copy => copy_one(src, dst, meta.len()),
            }
        }
        // Source gone: only a success if the destination is already in place
        // (the op completed before a crash); otherwise the source is genuinely
        // missing and the caller must surface a conflict.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if dst.try_exists()? {
                Ok(())
            } else {
                Err(e)
            }
        }
        Err(e) => Err(e),
    }
}

/// Undo a previously-applied operation. For `Move`, move the file back to its
/// source; for `Copy`, delete the copied destination and leave the source. Also
/// idempotent.
pub fn revert(src: &Path, dst: &Path, mode: MoveMode) -> io::Result<()> {
    match mode {
        MoveMode::Move => relocate(dst, src, MoveMode::Move),
        MoveMode::Copy => {
            if dst.try_exists()? {
                tracing::debug!(target: "conservatory::io", dst = %dst.display(), "mover: revert (delete copy)");
                fs::remove_file(dst)?;
            }
            Ok(())
        }
    }
}

fn move_one(src: &Path, dst: &Path, src_len: u64) -> io::Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => {
            tracing::debug!(target: "conservatory::io", src = %src.display(), dst = %dst.display(), bytes = src_len, "mover: rename");
            Ok(())
        }
        Err(e) if is_cross_device(&e) => {
            copy_across(src, dst, src_len)?;
            tracing::debug!(target: "conservatory::io", src = %src.display(), "mover: remove source after cross-device copy");
            fs::remove_file(src)
        }
        Err(e) => Err(e),
    }
}

fn copy_one(src: &Path, dst: &Path, src_len: u64) -> io::Result<()> {
    // Idempotent copy replay: a destination that already matches by size is
    // treated as done (the source is intact in copy mode, so we can re-check).
    if dst.try_exists()? && fs::metadata(dst)?.len() == src_len {
        return Ok(());
    }
    copy_across(src, dst, src_len)
}

/// Copy `src` to `dst` via a same-dir temp file, fsync, rename, then verify the
/// size. Public to the crate so the cross-filesystem path is directly testable
/// without a second filesystem. Leaves the source untouched.
pub(crate) fn copy_across(src: &Path, dst: &Path, src_len: u64) -> io::Result<()> {
    ensure_parent(dst)?;
    let temp = temp_path(dst)?;

    let result = (|| -> io::Result<()> {
        fs::copy(src, &temp)?;
        let f = File::open(&temp)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&temp, dst)?;
        // Verify: the destination must match the source size, or the copy is
        // suspect and we refuse to leave it in place.
        let dst_len = fs::metadata(dst)?.len();
        if dst_len != src_len {
            let _ = fs::remove_file(dst);
            return Err(io::Error::other(format!(
                "copy verification failed: {} is {dst_len} bytes, expected {src_len}",
                dst.display()
            )));
        }
        tracing::debug!(target: "conservatory::io", src = %src.display(), dst = %dst.display(), bytes = src_len, "mover: copy + fsync + rename");
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn ensure_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn temp_path(dst: &Path) -> io::Result<std::path::PathBuf> {
    let name = dst.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "relocate: destination has no file name",
        )
    })?;
    let mut temp_name = name.to_os_string();
    temp_name.push(PART_SUFFIX);
    Ok(dst.with_file_name(temp_name))
}

/// EXDEV (cross-device link) is how `rename` reports a cross-filesystem move.
/// `io::ErrorKind::CrossesDevices` is still unstable, so match the raw code.
fn is_cross_device(e: &io::Error) -> bool {
    e.raw_os_error() == Some(18)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        let mut f = File::create(path).unwrap();
        f.write_all(bytes).unwrap();
    }

    #[test]
    fn rename_fast_path_moves_and_creates_parents() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.flac");
        let dst = dir.path().join("a/b/c/dst.flac");
        write(&src, b"audio");

        relocate(&src, &dst, MoveMode::Move).unwrap();

        assert!(!src.exists());
        assert_eq!(fs::read(&dst).unwrap(), b"audio");
    }

    #[test]
    fn move_is_idempotent_after_completion() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.flac");
        let dst = dir.path().join("dst.flac");
        write(&src, b"x");

        relocate(&src, &dst, MoveMode::Move).unwrap();
        // Replay: source already gone, destination present => clean no-op.
        relocate(&src, &dst, MoveMode::Move).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"x");
    }

    #[test]
    fn missing_source_with_no_destination_errors() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("nope.flac");
        let dst = dir.path().join("dst.flac");
        let err = relocate(&src, &dst, MoveMode::Move).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn copy_mode_leaves_source_and_is_idempotent() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.flac");
        let dst = dir.path().join("out/dst.flac");
        write(&src, b"hello");

        relocate(&src, &dst, MoveMode::Copy).unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"hello"); // source kept
        assert_eq!(fs::read(&dst).unwrap(), b"hello");

        // Replay: destination already matches by size => no-op.
        relocate(&src, &dst, MoveMode::Copy).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"hello");
    }

    #[test]
    fn copy_across_copies_verifies_and_is_idempotent() {
        // Exercises the cross-filesystem branch directly (no second FS needed).
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.flac");
        let dst = dir.path().join("nested/dst.flac");
        write(&src, b"some bytes");

        copy_across(&src, &dst, fs::metadata(&src).unwrap().len()).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"some bytes");
        assert!(src.exists());
        // No stray temp file left behind.
        assert!(
            !dst.with_file_name(format!("dst.flac{PART_SUFFIX}"))
                .exists()
        );

        // Re-copy over an existing destination is fine.
        copy_across(&src, &dst, fs::metadata(&src).unwrap().len()).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"some bytes");
    }

    #[test]
    fn revert_move_returns_the_file() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.flac");
        let dst = dir.path().join("dst.flac");
        write(&src, b"v");

        relocate(&src, &dst, MoveMode::Move).unwrap();
        revert(&src, &dst, MoveMode::Move).unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"v");
        assert!(!dst.exists());
    }

    #[test]
    fn revert_copy_deletes_only_the_destination() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.flac");
        let dst = dir.path().join("dst.flac");
        write(&src, b"v");

        relocate(&src, &dst, MoveMode::Copy).unwrap();
        revert(&src, &dst, MoveMode::Copy).unwrap();
        assert!(src.exists()); // source untouched
        assert!(!dst.exists());
        // Idempotent revert.
        revert(&src, &dst, MoveMode::Copy).unwrap();
    }

    #[test]
    fn src_equals_dst_is_noop() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("x.flac");
        write(&p, b"x");
        relocate(&p, &p, MoveMode::Move).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"x");
    }
}
