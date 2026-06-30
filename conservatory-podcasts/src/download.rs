//! Episode download into the managed tree (Phase 6a-iii-b).
//!
//! Streams an episode's audio to `<root>/<episode.folder_path>/<filename>`
//! (the managed download layout, spec §5.3) and records the relative
//! `audio_path` on success. The write is crash-safe in the shape of
//! `core::mover::fsops`: stream to a sibling `.part` file, fsync, then rename
//! into place. `audio_path` stays `None` if anything fails, so a partial
//! download is never mistaken for a complete one.

use std::path::{Path, PathBuf};

use conservatory_core::db::{Episode, WorkerHandle};
use reqwest::Client;
use tokio::io::AsyncWriteExt;

use crate::credentials::BasicAuth;
use crate::error::{FetchError, Result};

/// Download an episode's audio, recording its `audio_path`. Reuses the caller's
/// `reqwest::Client` (so it shares the [`Fetcher`](crate::Fetcher) pool).
/// `auth` carries HTTP Basic credentials for a private feed; a 401/404 surfaces
/// as a transport error (the file is not created).
pub async fn download_episode(
    client: &Client,
    worker: &WorkerHandle,
    root: &Path,
    episode: &Episode,
    auth: Option<&BasicAuth>,
) -> Result<PathBuf> {
    let url = episode
        .audio_url
        .as_deref()
        .ok_or_else(|| FetchError::Download("episode has no audio URL".into()))?;

    let filename = filename_for(url, episode.mime_type.as_deref());
    let rel = format!("{}/{}", episode.folder_path, filename);
    let dst = root.join(&rel);
    let dir = dst
        .parent()
        .ok_or_else(|| FetchError::Download("episode path has no parent".into()))?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| FetchError::Download(format!("creating {}: {e}", dir.display())))?;

    let mut req = client.get(url);
    if let Some(a) = auth {
        req = req.basic_auth(&a.user, Some(&a.password));
    }
    // error_for_status turns a 401 (auth required) / 404 into an error before we
    // touch the filesystem.
    tracing::debug!(target: "conservatory::net", url, episode = episode.id, "download: GET");
    let mut response = req.send().await?.error_for_status()?;

    // Stream to a sibling temp file, fsync, then rename atomically (same dir).
    let tmp = part_path(&dst);
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .map_err(|e| FetchError::Download(format!("creating {}: {e}", tmp.display())))?;
    let mut written: u64 = 0;
    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk)
            .await
            .map_err(|e| FetchError::Download(format!("writing {}: {e}", tmp.display())))?;
        written += chunk.len() as u64;
    }
    file.sync_all()
        .await
        .map_err(|e| FetchError::Download(format!("fsync {}: {e}", tmp.display())))?;
    drop(file);
    tokio::fs::rename(&tmp, &dst)
        .await
        .map_err(|e| FetchError::Download(format!("renaming into {}: {e}", dst.display())))?;
    tracing::debug!(target: "conservatory::io", dst = %dst.display(), bytes = written, "download: wrote episode");

    if let Some(expected) = episode.file_size
        && expected != written
    {
        // Feeds routinely carry a stale enclosure length; warn but keep the file.
        tracing::warn!(
            expected,
            written,
            episode = episode.id,
            "download: enclosure size mismatch"
        );
    }

    worker.set_episode_audio_path(episode.id, rel).await?;
    Ok(dst)
}

/// The URL's last path segment when it looks like a filename, else a name
/// synthesized from the MIME type.
fn filename_for(url: &str, mime: Option<&str>) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut segs| segs.next_back().map(str::to_string))
        })
        .filter(|s| !s.is_empty() && s.contains('.'))
        .unwrap_or_else(|| format!("episode.{}", ext_for_mime(mime)))
}

fn ext_for_mime(mime: Option<&str>) -> &'static str {
    match mime.map(|m| m.split(';').next().unwrap_or(m).trim()) {
        Some("audio/mpeg") => "mp3",
        Some("audio/mp4" | "audio/x-m4a" | "audio/aac") => "m4a",
        Some("audio/ogg" | "audio/opus") => "ogg",
        Some("audio/flac" | "audio/x-flac") => "flac",
        Some("audio/wav" | "audio/x-wav") => "wav",
        _ => "bin",
    }
}

/// `foo.mp3` -> `foo.mp3.part` (a sibling, so the rename is a same-dir move).
fn part_path(dst: &Path) -> PathBuf {
    let mut s = dst.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_from_url_basename() {
        assert_eq!(
            filename_for("https://cdn.example/ep-12.mp3", None),
            "ep-12.mp3"
        );
        // Query strings are ignored (path only).
        assert_eq!(
            filename_for("https://cdn.example/audio/ep-12.m4a?token=abc", None),
            "ep-12.m4a"
        );
    }

    #[test]
    fn filename_falls_back_to_mime() {
        assert_eq!(
            filename_for("https://cdn.example/stream", Some("audio/mpeg")),
            "episode.mp3"
        );
        assert_eq!(
            filename_for("https://cdn.example/", Some("audio/mp4")),
            "episode.m4a"
        );
        assert_eq!(filename_for("https://cdn.example/x", None), "episode.bin");
    }

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(
            part_path(Path::new("/lib/Podcasts/s/e/a.mp3")),
            PathBuf::from("/lib/Podcasts/s/e/a.mp3.part")
        );
    }
}
