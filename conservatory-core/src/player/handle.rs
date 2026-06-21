//! The cross-thread player handle and its message types (spec §6.1).
//!
//! The engine runs on a dedicated thread (it owns the `!Send` libmpv host). The
//! rest of the app holds a [`PlayerHandle`] — `Send + Clone` — and talks to the
//! engine two ways: **commands** flow out over an `mpsc` channel, and **state**
//! flows back through a shared [`PlayerSnapshot`] the consumer polls (the GTK
//! Now-bar on a `glib` timeout, the CLI on a sleep loop). No new dependency; a
//! transport readout is a sampled display, so polling a `Mutex` is enough.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::player::item::PlayableItem;

/// A command sent from a consumer to the engine thread.
pub enum PlayerCommand {
    /// Replace the queue and cue `start`, then begin playing.
    SetQueue {
        items: Vec<PlayableItem>,
        start: usize,
    },
    Play,
    Pause,
    TogglePause,
    Next,
    Previous,
    /// Seek to an absolute offset in seconds.
    Seek(f64),
    /// Set the output volume (0–100).
    SetVolume(i64),
    /// Halt playback and persist, but keep the engine thread alive.
    Stop,
    /// Stop and exit the engine thread (joined by [`PlayerHandle::shutdown`]).
    Shutdown,
}

/// A consistent snapshot of the engine's state, refreshed once per loop turn and
/// read by the Now-bar / CLI. Always written whole under one lock, so a reader
/// never sees fields from two different turns.
#[derive(Debug, Clone)]
pub struct PlayerSnapshot {
    pub current_index: Option<usize>,
    pub track_id: Option<i64>,
    pub position: f64,
    pub duration: Option<f64>,
    pub paused: bool,
    pub volume: i64,
    pub queue_len: usize,
    /// The queue has been played to its end (or is empty): a poller can stop.
    pub ended: bool,
}

impl Default for PlayerSnapshot {
    fn default() -> Self {
        Self {
            current_index: None,
            track_id: None,
            position: 0.0,
            duration: None,
            paused: false,
            volume: 100,
            queue_len: 0,
            ended: false,
        }
    }
}

/// Handle to the running player engine. Cloneable; every clone shares the one
/// command channel and snapshot. Dropping a clone does nothing; call
/// [`PlayerHandle::shutdown`] to stop and join the engine thread.
#[derive(Clone)]
pub struct PlayerHandle {
    tx: Sender<PlayerCommand>,
    snapshot: Arc<Mutex<PlayerSnapshot>>,
    join: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl PlayerHandle {
    pub(crate) fn new(
        tx: Sender<PlayerCommand>,
        snapshot: Arc<Mutex<PlayerSnapshot>>,
        join: JoinHandle<()>,
    ) -> Self {
        Self {
            tx,
            snapshot,
            join: Arc::new(Mutex::new(Some(join))),
        }
    }

    /// Replace the queue with `items` and start playing from `start`.
    pub fn play_queue(&self, items: Vec<PlayableItem>, start: usize) {
        let _ = self.tx.send(PlayerCommand::SetQueue { items, start });
    }

    pub fn play(&self) {
        let _ = self.tx.send(PlayerCommand::Play);
    }

    pub fn pause(&self) {
        let _ = self.tx.send(PlayerCommand::Pause);
    }

    pub fn toggle_pause(&self) {
        let _ = self.tx.send(PlayerCommand::TogglePause);
    }

    pub fn next(&self) {
        let _ = self.tx.send(PlayerCommand::Next);
    }

    pub fn previous(&self) {
        let _ = self.tx.send(PlayerCommand::Previous);
    }

    pub fn seek(&self, secs: f64) {
        let _ = self.tx.send(PlayerCommand::Seek(secs));
    }

    pub fn set_volume(&self, volume: i64) {
        let _ = self.tx.send(PlayerCommand::SetVolume(volume));
    }

    pub fn stop(&self) {
        let _ = self.tx.send(PlayerCommand::Stop);
    }

    /// The current engine state (a cheap clone of the shared snapshot).
    pub fn snapshot(&self) -> PlayerSnapshot {
        self.snapshot.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Stop the engine and join its thread. Idempotent: the first caller takes
    /// and joins the handle; later calls (from other clones) are no-ops.
    pub fn shutdown(&self) {
        let _ = self.tx.send(PlayerCommand::Shutdown);
        if let Ok(mut guard) = self.join.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}
