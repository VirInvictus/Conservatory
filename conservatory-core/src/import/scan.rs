//! Recursively find audio files under a folder (the first step of import, §5.4).

use std::path::{Path, PathBuf};

use crate::errors::Result;

/// Extensions Conservatory treats as importable audio. Matched case-insensitively.
const AUDIO_EXTS: &[&str] = &[
    "flac", "mp3", "opus", "ogg", "m4a", "aac", "wav", "wv", "ape", "mpc",
];

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| AUDIO_EXTS.contains(&e.as_str()))
}

/// Collect audio files under `dir`, recursively, sorted for deterministic order.
/// A plain file path is accepted too (a single-file import).
pub fn scan(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if dir.is_file() {
        if is_audio(dir) {
            out.push(dir.to_path_buf());
        }
        return Ok(out);
    }
    walk(dir, &mut out)?;
    out.sort();
    tracing::debug!(target: "conservatory::io", dir = %dir.display(), files = out.len(), "import: scanned");
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if is_audio(&path) {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn finds_audio_recursively_and_skips_non_audio() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.flac"), b"").unwrap();
        fs::write(root.join("sub/b.MP3"), b"").unwrap(); // uppercase ext
        fs::write(root.join("cover.jpg"), b"").unwrap();
        fs::write(root.join("notes.txt"), b"").unwrap();

        let found = scan(root).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|p| p.ends_with("a.flac")));
        assert!(found.iter().any(|p| p.ends_with("sub/b.MP3")));
    }

    #[test]
    fn single_file_import() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("x.opus");
        fs::write(&f, b"").unwrap();
        assert_eq!(scan(&f).unwrap(), vec![f]);
    }
}
