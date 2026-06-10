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
use crate::db::models::{Album, Artist, Track};
use crate::db::{connection, migrations, probe, writes};
use crate::errors::{Error, Result};

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
