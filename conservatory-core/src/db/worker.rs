//! Single-writer SQLite worker (spec §2.1).
//!
//! A dedicated tokio blocking task owns the one writable `rusqlite::Connection`.
//! Every consumer (GTK / CLI / future fetch + playback loops) holds an
//! `mpsc::Sender<Command>` and never touches the connection directly; reads go
//! through the separate [`crate::db::ReadPool`]. The dispatch shape (typed
//! `WorkerHandle` methods, per-op `oneshot` replies) is ported from
//! `belfry-core`; the **panic-catch-and-restart loop** is ported from Viaduct,
//! so a single bad command logs and restarts the loop instead of silently
//! killing the writer.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

use crate::db::command::Command;
use crate::db::models::{Album, Artist, Chapter, Episode, Playback, Show, ShowSettings, Track};
use crate::db::{connection, migrations, probe, writes};
use crate::edit::{AlbumEdit, TrackEdit};
use crate::errors::{Error, Result};
use crate::mover::journal::{self, JobState};
use crate::mover::{MoveKind, MoveMode, MoveOp};

const CHANNEL_CAPACITY: usize = 64;

/// How long the loop waits after a panic before reopening and resuming, so a
/// tight panic-restart cycle can't spin the CPU. Ported from Viaduct.
const RESTART_BACKOFF: Duration = Duration::from_secs(1);

/// Handle to the running worker. Cloneable; backed by a tokio mpsc sender.
#[derive(Clone)]
pub struct WorkerHandle {
    tx: mpsc::Sender<Command>,
}

impl WorkerHandle {
    /// Build a command with a fresh reply channel, send it, and await the
    /// worker's result. A closed channel (worker gone) surfaces as
    /// `WorkerChannelClosed` through the `?` conversions in `errors`.
    async fn dispatch<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T>>) -> Command,
    ) -> Result<T> {
        let (reply, recv) = oneshot::channel();
        self.tx.send(build(reply)).await?;
        recv.await?
    }

    /// Write a key/value through the debug probe table (Phase 1a artifact).
    pub async fn probe_write(
        &self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<()> {
        let key = key.into();
        let value = value.into();
        self.dispatch(|reply| Command::ProbeWrite { key, value, reply })
            .await
    }

    /// Insert an artist, returning its new id.
    pub async fn insert_artist(&self, artist: Artist) -> Result<i64> {
        self.dispatch(|reply| Command::InsertArtist { artist, reply })
            .await
    }

    /// Insert an album, returning its new id.
    pub async fn insert_album(&self, album: Album) -> Result<i64> {
        self.dispatch(|reply| Command::InsertAlbum { album, reply })
            .await
    }

    /// Apply a track-level field edit (Phase 5a, spec §3.5).
    pub async fn update_track(&self, track_id: i64, edit: TrackEdit) -> Result<()> {
        self.dispatch(|reply| Command::UpdateTrack {
            track_id,
            edit,
            reply,
        })
        .await
    }

    /// Apply an album-level field edit (Phase 5a). Album-level fields are
    /// path-affecting; re-render and move after.
    pub async fn update_album(&self, album_id: i64, edit: AlbumEdit) -> Result<()> {
        self.dispatch(|reply| Command::UpdateAlbum {
            album_id,
            edit,
            reply,
        })
        .await
    }

    /// Replace a track's raw genre set (Phase 5a, §5.2 multi-value side).
    pub async fn set_track_genres(&self, track_id: i64, genres: Vec<String>) -> Result<()> {
        self.dispatch(|reply| Command::SetTrackGenres {
            track_id,
            genres,
            reply,
        })
        .await
    }

    /// Update a track's ReplayGain values after a scan (Phase 5c).
    pub async fn set_track_replaygain(
        &self,
        track_id: i64,
        track_gain: Option<f64>,
        album_gain: Option<f64>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::SetTrackReplayGain {
            track_id,
            track_gain,
            album_gain,
            reply,
        })
        .await
    }

    /// Set an album's cover path, optionally refreshing the accent (Phase 5d).
    pub async fn set_album_cover_path(
        &self,
        album_id: i64,
        cover_path: Option<String>,
        accent_rgb: Option<u32>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::SetAlbumCoverPath {
            album_id,
            cover_path,
            accent_rgb,
            reply,
        })
        .await
    }

    /// Resolve an artist by sort_name, creating it on first sight (import).
    pub async fn get_or_create_artist(
        &self,
        name: String,
        sort_name: String,
        musicbrainz_id: Option<String>,
    ) -> Result<i64> {
        self.dispatch(|reply| Command::GetOrCreateArtist {
            name,
            sort_name,
            musicbrainz_id,
            reply,
        })
        .await
    }

    /// Resolve an album by (album_artist_id, title), creating it on first sight.
    pub async fn get_or_create_album(&self, album: Album) -> Result<i64> {
        self.dispatch(|reply| Command::GetOrCreateAlbum { album, reply })
            .await
    }

    /// Set an album's shelf genre.
    pub async fn set_album_shelf_genre(&self, album_id: i64, shelf_genre: String) -> Result<()> {
        self.dispatch(|reply| Command::SetAlbumShelfGenre {
            album_id,
            shelf_genre,
            reply,
        })
        .await
    }

    /// Insert a track, returning its new id.
    pub async fn insert_track(&self, track: Track) -> Result<i64> {
        self.dispatch(|reply| Command::InsertTrack { track, reply })
            .await
    }

    /// Resolve a raw genre name to its id, creating it on first sight.
    pub async fn get_or_create_genre(&self, name: impl Into<String>) -> Result<i64> {
        let name = name.into();
        self.dispatch(|reply| Command::GetOrCreateGenre { name, reply })
            .await
    }

    /// Link a track to a genre (idempotent).
    pub async fn link_track_genre(&self, track_id: i64, genre_id: i64) -> Result<()> {
        self.dispatch(|reply| Command::LinkTrackGenre {
            track_id,
            genre_id,
            reply,
        })
        .await
    }

    /// Journal a move job and its operations atomically, returning the job id.
    /// Called before any file is touched (spec §5.4).
    pub async fn create_move_job(
        &self,
        kind: MoveKind,
        mode: MoveMode,
        library_root: String,
        created_at: i64,
        ops: Vec<MoveOp>,
    ) -> Result<i64> {
        self.dispatch(|reply| Command::CreateMoveJob {
            kind,
            mode,
            library_root,
            created_at,
            ops,
            reply,
        })
        .await
    }

    /// Mark an operation done and apply its DB path update.
    pub async fn complete_operation(
        &self,
        op_id: i64,
        track_id: Option<i64>,
        album_id: Option<i64>,
        db_new_path: Option<String>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::CompleteOperation {
            op_id,
            track_id,
            album_id,
            db_new_path,
            reply,
        })
        .await
    }

    /// Revert an operation's DB path and reset it to pending (undo).
    pub async fn revert_operation(
        &self,
        op_id: i64,
        track_id: Option<i64>,
        album_id: Option<i64>,
        db_old_path: Option<String>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::RevertOperation {
            op_id,
            track_id,
            album_id,
            db_old_path,
            reply,
        })
        .await
    }

    /// Set a move job's lifecycle state.
    pub async fn set_job_state(&self, job_id: i64, state: JobState) -> Result<()> {
        self.dispatch(|reply| Command::SetJobState {
            job_id,
            state,
            reply,
        })
        .await
    }

    /// Save a Perspective (insert, or overwrite by name), returning its id.
    pub async fn save_perspective(
        &self,
        name: String,
        expression: String,
        scope: String,
        created_at: i64,
    ) -> Result<i64> {
        self.dispatch(|reply| Command::SavePerspective {
            name,
            expression,
            scope,
            created_at,
            reply,
        })
        .await
    }

    /// Delete a Perspective by id.
    pub async fn delete_perspective(&self, id: i64) -> Result<()> {
        self.dispatch(|reply| Command::DeletePerspective { id, reply })
            .await
    }

    /// Save the singleton playback cursor (spec §6.4, Phase 4a).
    pub async fn save_playback_state(
        &self,
        track_id: Option<i64>,
        position: f64,
        paused: bool,
        volume: i64,
        updated_at: i64,
    ) -> Result<()> {
        self.dispatch(|reply| Command::SavePlaybackState {
            track_id,
            position,
            paused,
            volume,
            updated_at,
            reply,
        })
        .await
    }

    /// Record a completed play: bump `play_count` and stamp `last_played`.
    pub async fn increment_play_count(&self, track_id: i64, played_at: i64) -> Result<()> {
        self.dispatch(|reply| Command::IncrementPlayCount {
            track_id,
            played_at,
            reply,
        })
        .await
    }

    /// Append tracks to the unified queue tail (spec §4.3, Phase 4b).
    pub async fn enqueue_tracks(&self, track_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::EnqueueTracks { track_ids, reply })
            .await
    }

    /// Replace the whole queue with these tracks in order.
    pub async fn replace_queue_with_tracks(&self, track_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::ReplaceQueueWithTracks { track_ids, reply })
            .await
    }

    /// Remove the queue entry at `position`.
    pub async fn remove_queue_item(&self, position: i64) -> Result<()> {
        self.dispatch(|reply| Command::RemoveQueueItem { position, reply })
            .await
    }

    /// Move the queue entry at `from` to `to`.
    pub async fn reorder_queue(&self, from: i64, to: i64) -> Result<()> {
        self.dispatch(|reply| Command::ReorderQueue { from, to, reply })
            .await
    }

    /// Empty the queue.
    pub async fn clear_queue(&self) -> Result<()> {
        self.dispatch(|reply| Command::ClearQueue { reply }).await
    }

    /// Resolve a show by `feed_url`, creating it on first sight (`podcast add`,
    /// Phase 6a). Returns the show id; adding the same feed twice is idempotent.
    pub async fn get_or_create_show(&self, show: Show) -> Result<i64> {
        self.dispatch(|reply| Command::GetOrCreateShow { show, reply })
            .await
    }

    /// Update a subscription in full (incl. the conditional-GET state the fetch
    /// loop refreshes, Phase 6a-ii).
    pub async fn update_show(&self, show: Show) -> Result<()> {
        self.dispatch(|reply| Command::UpdateShow { show, reply })
            .await
    }

    /// Delete a subscription (`podcast remove`); cascades its episodes and state.
    pub async fn delete_show(&self, id: i64) -> Result<()> {
        self.dispatch(|reply| Command::DeleteShow { id, reply })
            .await
    }

    /// Insert or update an episode by `(show_id, guid)`, returning its id.
    pub async fn upsert_episode(&self, episode: Episode) -> Result<i64> {
        self.dispatch(|reply| Command::UpsertEpisode { episode, reply })
            .await
    }

    /// Upsert an episode's triage/playback row.
    pub async fn upsert_playback(&self, playback: Playback) -> Result<()> {
        self.dispatch(|reply| Command::UpsertPlayback { playback, reply })
            .await
    }

    /// Upsert a show's per-show overrides.
    pub async fn upsert_show_settings(&self, settings: ShowSettings) -> Result<()> {
        self.dispatch(|reply| Command::UpsertShowSettings { settings, reply })
            .await
    }

    /// Replace an episode's chapter set.
    pub async fn replace_chapters(&self, episode_id: i64, chapters: Vec<Chapter>) -> Result<()> {
        self.dispatch(|reply| Command::ReplaceChapters {
            episode_id,
            chapters,
            reply,
        })
        .await
    }

    /// Resolve a tag name to its id, creating it on first sight.
    pub async fn get_or_create_tag(&self, name: impl Into<String>) -> Result<i64> {
        let name = name.into();
        self.dispatch(|reply| Command::GetOrCreateTag { name, reply })
            .await
    }

    /// Replace a show's tag set (the OPML round-trip side).
    pub async fn set_show_tags(&self, show_id: i64, tags: Vec<String>) -> Result<()> {
        self.dispatch(|reply| Command::SetShowTags {
            show_id,
            tags,
            reply,
        })
        .await
    }

    /// Send a shutdown ack. The loop exits once every `WorkerHandle` clone has
    /// dropped and the channel closes; this just confirms the worker is alive.
    pub async fn shutdown_ack(&self) -> Result<()> {
        let (reply, recv) = oneshot::channel();
        self.tx.send(Command::Shutdown { reply }).await?;
        recv.await.map_err(|_| Error::WorkerChannelClosed)
    }

    /// Test-only: crash the worker to exercise the restart loop. The send
    /// succeeds (the command is buffered); the worker panics handling it.
    #[cfg(test)]
    async fn trigger_panic(&self) -> Result<()> {
        self.tx.send(Command::Panic).await?;
        Ok(())
    }
}

/// Spawn the worker. Opens the writer and runs migrations **synchronously**
/// first, so a bad path or failed migration surfaces to the caller as an
/// `Err` (the CLI relies on this); only then is the long-lived serve task
/// spawned. The pre-flight connection is dropped and the serve task reopens
/// its own; the second open is a negligible startup cost for clean error
/// reporting plus reopen-on-panic.
pub fn spawn_worker(db_path: PathBuf) -> Result<WorkerHandle> {
    let mut conn = connection::open_writer(&db_path)?;
    migrations::run(&mut conn)?;
    drop(conn);

    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    tokio::task::spawn_blocking(move || serve(db_path, rx));

    Ok(WorkerHandle { tx })
}

/// The serve loop: reopen the writer, drain commands until the channel closes,
/// and on a handler panic log, back off, and restart (reopening the writer).
/// `&mut rx` is held across restarts so the channel — and any buffered
/// commands — survive the panic (Viaduct's pattern).
fn serve(db_path: PathBuf, mut rx: mpsc::Receiver<Command>) {
    loop {
        let mut rx_ref = AssertUnwindSafe(&mut rx);
        let db_path = db_path.clone();

        let outcome = std::panic::catch_unwind(move || {
            let mut conn = match connection::open_writer(&db_path) {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::error!(error = %e, "worker failed to reopen writer; stopping");
                    return;
                }
            };
            while let Some(command) = rx_ref.blocking_recv() {
                let kind = command.kind();
                let start = Instant::now();
                handle(&mut conn, command);
                tracing::trace!(
                    kind,
                    elapsed_us = start.elapsed().as_micros() as u64,
                    "worker cmd"
                );
            }
        });

        match outcome {
            Ok(()) => break, // channel closed (or reopen failed): clean exit.
            Err(err) => {
                tracing::error!(?err, "worker panicked; restarting loop");
                std::thread::sleep(RESTART_BACKOFF);
            }
        }
    }
    tracing::info!("worker shutdown");
}

fn handle(conn: &mut Connection, command: Command) {
    match command {
        Command::ProbeWrite { key, value, reply } => {
            let _ = reply.send(probe::write(conn, &key, &value));
        }
        Command::InsertArtist { artist, reply } => {
            let _ = reply.send(writes::insert_artist(conn, &artist));
        }
        Command::InsertAlbum { album, reply } => {
            let _ = reply.send(writes::insert_album(conn, &album));
        }
        Command::UpdateTrack {
            track_id,
            edit,
            reply,
        } => {
            let _ = reply.send(writes::update_track(conn, track_id, &edit));
        }
        Command::UpdateAlbum {
            album_id,
            edit,
            reply,
        } => {
            let _ = reply.send(writes::update_album(conn, album_id, &edit));
        }
        Command::SetTrackGenres {
            track_id,
            genres,
            reply,
        } => {
            let _ = reply.send(writes::set_track_genres(conn, track_id, &genres));
        }
        Command::SetTrackReplayGain {
            track_id,
            track_gain,
            album_gain,
            reply,
        } => {
            let _ = reply.send(writes::set_track_replaygain(
                conn, track_id, track_gain, album_gain,
            ));
        }
        Command::SetAlbumCoverPath {
            album_id,
            cover_path,
            accent_rgb,
            reply,
        } => {
            let _ = reply.send(writes::set_album_cover_path(
                conn,
                album_id,
                cover_path.as_deref(),
                accent_rgb,
            ));
        }
        Command::GetOrCreateArtist {
            name,
            sort_name,
            musicbrainz_id,
            reply,
        } => {
            let _ = reply.send(writes::get_or_create_artist(
                conn,
                &name,
                &sort_name,
                musicbrainz_id.as_deref(),
            ));
        }
        Command::GetOrCreateAlbum { album, reply } => {
            let _ = reply.send(writes::get_or_create_album(conn, &album));
        }
        Command::SetAlbumShelfGenre {
            album_id,
            shelf_genre,
            reply,
        } => {
            let _ = reply.send(writes::set_album_shelf_genre(conn, album_id, &shelf_genre));
        }
        Command::InsertTrack { track, reply } => {
            let _ = reply.send(writes::insert_track(conn, &track));
        }
        Command::GetOrCreateGenre { name, reply } => {
            let _ = reply.send(writes::get_or_create_genre(conn, &name));
        }
        Command::LinkTrackGenre {
            track_id,
            genre_id,
            reply,
        } => {
            let _ = reply.send(writes::link_track_genre(conn, track_id, genre_id));
        }
        Command::CreateMoveJob {
            kind,
            mode,
            library_root,
            created_at,
            ops,
            reply,
        } => {
            let _ = reply.send(journal::create_job(
                conn,
                kind,
                mode,
                &library_root,
                created_at,
                &ops,
            ));
        }
        Command::CompleteOperation {
            op_id,
            track_id,
            album_id,
            db_new_path,
            reply,
        } => {
            let _ = reply.send(journal::complete_operation(
                conn,
                op_id,
                track_id,
                album_id,
                db_new_path.as_deref(),
            ));
        }
        Command::RevertOperation {
            op_id,
            track_id,
            album_id,
            db_old_path,
            reply,
        } => {
            let _ = reply.send(journal::revert_operation(
                conn,
                op_id,
                track_id,
                album_id,
                db_old_path.as_deref(),
            ));
        }
        Command::SetJobState {
            job_id,
            state,
            reply,
        } => {
            let _ = reply.send(journal::set_job_state(conn, job_id, state));
        }
        Command::SavePerspective {
            name,
            expression,
            scope,
            created_at,
            reply,
        } => {
            let _ = reply.send(writes::save_perspective(
                conn,
                &name,
                &expression,
                &scope,
                created_at,
            ));
        }
        Command::DeletePerspective { id, reply } => {
            let _ = reply.send(writes::delete_perspective(conn, id));
        }
        Command::SavePlaybackState {
            track_id,
            position,
            paused,
            volume,
            updated_at,
            reply,
        } => {
            let _ = reply.send(writes::save_playback_state(
                conn, track_id, position, paused, volume, updated_at,
            ));
        }
        Command::IncrementPlayCount {
            track_id,
            played_at,
            reply,
        } => {
            let _ = reply.send(writes::increment_play_count(conn, track_id, played_at));
        }
        Command::EnqueueTracks { track_ids, reply } => {
            let _ = reply.send(writes::enqueue_tracks(conn, &track_ids));
        }
        Command::ReplaceQueueWithTracks { track_ids, reply } => {
            let _ = reply.send(writes::replace_queue_with_tracks(conn, &track_ids));
        }
        Command::RemoveQueueItem { position, reply } => {
            let _ = reply.send(writes::remove_queue_item(conn, position));
        }
        Command::ReorderQueue { from, to, reply } => {
            let _ = reply.send(writes::reorder_queue(conn, from, to));
        }
        Command::ClearQueue { reply } => {
            let _ = reply.send(writes::clear_queue(conn));
        }
        Command::GetOrCreateShow { show, reply } => {
            let _ = reply.send(writes::get_or_create_show(conn, &show));
        }
        Command::UpdateShow { show, reply } => {
            let _ = reply.send(writes::update_show(conn, &show));
        }
        Command::DeleteShow { id, reply } => {
            let _ = reply.send(writes::delete_show(conn, id));
        }
        Command::UpsertEpisode { episode, reply } => {
            let _ = reply.send(writes::upsert_episode(conn, &episode));
        }
        Command::UpsertPlayback { playback, reply } => {
            let _ = reply.send(writes::upsert_playback(conn, &playback));
        }
        Command::UpsertShowSettings { settings, reply } => {
            let _ = reply.send(writes::upsert_show_settings(conn, &settings));
        }
        Command::ReplaceChapters {
            episode_id,
            chapters,
            reply,
        } => {
            let _ = reply.send(writes::replace_chapters(conn, episode_id, &chapters));
        }
        Command::GetOrCreateTag { name, reply } => {
            let _ = reply.send(writes::get_or_create_tag(conn, &name));
        }
        Command::SetShowTags {
            show_id,
            tags,
            reply,
        } => {
            let _ = reply.send(writes::set_show_tags(conn, show_id, &tags));
        }
        Command::Shutdown { reply } => {
            let _ = reply.send(());
            // Loop exits naturally when the last sender drops.
        }
        #[cfg(test)]
        Command::Panic => panic!("worker panic (test hook)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ReadPool, probe_read};
    use tempfile::tempdir;

    #[tokio::test]
    async fn writer_restarts_after_panic() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let handle = spawn_worker(path.clone()).unwrap();
        let pool = ReadPool::new(path.clone(), 1).unwrap();

        // Crash the worker, then prove a subsequent write still lands once the
        // restart loop has reopened the connection.
        handle.trigger_panic().await.unwrap();
        handle.probe_write("k", "v").await.unwrap();

        assert_eq!(probe_read(&pool, "k").unwrap().as_deref(), Some("v"));
    }
}
