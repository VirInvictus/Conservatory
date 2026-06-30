//! Retention: prune downloaded episodes beyond a show's `keep_count` (Phase
//! 6b-ii-c-3-b).
//!
//! A management setting, distinct from the playback overrides resolved into the
//! profile (speed, Phase 6b-ii-c-3-a). `shows.keep_count` caps how many of a
//! show's *downloaded* episodes are kept on disk; `0` means keep all. When a
//! show has more downloaded episodes than its cap, the oldest are pruned: the
//! audio file is deleted and the row's `audio_path` cleared, so the episode
//! reverts to stream-only (the row, triage state, and resume position survive;
//! only the bytes go).
//!
//! Retention is **root-aware** (it deletes files under the library root), so it
//! is a separate pass from the network-only [`crate::refresh`], split into a
//! [`plan`] (a pure read of what would be pruned) and an [`apply`] (the
//! deletions), mirroring the mover's dry-run-then-apply safety. Only files the
//! user actually downloaded are ever touched (`auto_download` is off by
//! default, spec §5.3).

use std::path::Path;

use conservatory_core::db::{
    Episode, ReadPool, WorkerHandle, get_show, list_episodes_for_show, list_shows,
};

use crate::error::Result;

/// One downloaded episode slated for (or completed) pruning. Carries enough to
/// report a dry-run without a re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPrune {
    pub episode_id: i64,
    pub show_id: i64,
    pub show_title: String,
    pub episode_title: String,
    /// The relative `audio_path` to delete under the library root.
    pub audio_path: String,
}

/// Compute what retention would prune, for one show or every show (`show_id`
/// `None`). Pure read: touches nothing on disk or in the DB. A show with
/// `keep_count == 0` keeps all its downloads and contributes nothing.
pub fn plan(pool: &ReadPool, show_id: Option<i64>) -> Result<Vec<RetentionPrune>> {
    let conn = pool.open()?;
    let show_ids = match show_id {
        Some(id) => vec![id],
        None => list_shows(&conn)?.into_iter().map(|s| s.id).collect(),
    };

    let mut prunes = Vec::new();
    for id in show_ids {
        let Some(show) = get_show(&conn, id)? else {
            continue;
        };
        if show.keep_count == 0 {
            continue; // keep all
        }
        // Downloaded episodes, newest first (list_episodes_for_show orders by
        // pub_date DESC). Keep the cap's worth; the rest are prune candidates.
        let downloaded: Vec<Episode> = list_episodes_for_show(&conn, id)?
            .into_iter()
            .filter(|e| e.audio_path.is_some())
            .collect();
        for ep in downloaded.into_iter().skip(show.keep_count as usize) {
            prunes.push(RetentionPrune {
                episode_id: ep.id,
                show_id: id,
                show_title: show.title.clone(),
                episode_title: ep.title,
                audio_path: ep.audio_path.expect("filtered to Some above"),
            });
        }
    }
    Ok(prunes)
}

/// Apply a prune plan: delete each episode's audio file under `root`, then
/// clear its `audio_path`. A file already gone is treated as success (the row is
/// still cleared). Returns the count actually pruned; a per-file IO error is
/// logged and skipped (its row is left intact for a retry) rather than aborting
/// the batch. The best-effort empty-parent-dir removal mirrors the managed
/// download layout (one dir per episode, spec §5.3).
pub async fn apply(worker: &WorkerHandle, root: &Path, prunes: &[RetentionPrune]) -> Result<usize> {
    let mut pruned = 0;
    for p in prunes {
        let dst = root.join(&p.audio_path);
        match tokio::fs::remove_file(&dst).await {
            Ok(()) => {
                tracing::debug!(target: "conservatory::io", path = %dst.display(), episode = p.episode_id, "retention: delete");
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // already gone
            Err(e) => {
                tracing::warn!(
                    episode = p.episode_id,
                    path = %dst.display(),
                    error = %e,
                    "retention: could not delete file; leaving audio_path"
                );
                continue;
            }
        }
        // Best-effort: remove the now-empty episode dir.
        if let Some(dir) = dst.parent() {
            let _ = tokio::fs::remove_dir(dir).await;
        }
        worker.clear_episode_audio_path(p.episode_id).await?;
        pruned += 1;
    }
    Ok(pruned)
}

/// Convenience: [`plan`] then [`apply`] for one show or all (`show_id` `None`).
/// Returns `(planned, pruned)`.
pub async fn prune(
    worker: &WorkerHandle,
    pool: &ReadPool,
    root: &Path,
    show_id: Option<i64>,
) -> Result<(usize, usize)> {
    let plan = plan(pool, show_id)?;
    let planned = plan.len();
    let pruned = apply(worker, root, &plan).await?;
    Ok((planned, pruned))
}
