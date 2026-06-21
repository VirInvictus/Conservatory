//! The player engine thread (spec §6.1, §6.4, docs/libmpv-profiles.md).
//!
//! A dedicated `std::thread` owns the `!Send` [`MpvHost`] and the in-memory
//! queue. It pumps libmpv events, advances on end-of-file applying each item's
//! profile (the spec §16.9 boundary switch, prototyped here with the music
//! profile), drains commands from the [`PlayerHandle`], and persists the cursor
//! through the single-writer worker. The host is constructed *inside* the thread
//! (via the `make_host` factory) so it never crosses a thread boundary.
//!
//! Persistence is split (spec §6.4): steady-state ticks are debounced and fired
//! and forgotten through the tokio runtime; the terminal writes (pause, seek,
//! stop, shutdown, and the play-count bump + final cursor on end-of-file) block
//! on the worker so the user-visible resume position and play counts are
//! guaranteed to land before the thread moves on.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tokio::runtime::Handle;

use crate::db::WorkerHandle;
use crate::errors::{Error, Result};
use crate::player::handle::{PlayerCommand, PlayerHandle, PlayerSnapshot};
use crate::player::host::{HostEvent, MpvHost};
use crate::player::item::PlayableItem;
use crate::player::state::{EndReason, StateDebounce, StateEvent};

/// How long each `pump` blocks waiting for a libmpv event. Short, so a queued
/// command is acted on within this bound (≤100 ms latency for the transport).
const PUMP_TIMEOUT: f64 = 0.1;

/// `Previous` restarts the current item if more than this many seconds in,
/// else it steps to the prior item (the conventional transport behaviour).
const PREVIOUS_RESTART_THRESHOLD: f64 = 3.0;

/// Spawn the engine with a real audio output.
pub fn spawn(worker: WorkerHandle, rt: Handle) -> Result<PlayerHandle> {
    spawn_with(MpvHost::new, worker, rt)
}

/// Spawn the engine with a null audio output (headless tests / CI).
pub fn spawn_null(worker: WorkerHandle, rt: Handle) -> Result<PlayerHandle> {
    spawn_with(MpvHost::new_null, worker, rt)
}

/// Spawn the engine, building its host with `make_host` **on the engine thread**
/// (so the `!Send` host never crosses a boundary). Returns only once the host
/// has constructed; a libmpv init failure surfaces as `Err`, like `spawn_worker`.
pub fn spawn_with(
    make_host: impl FnOnce() -> Result<MpvHost> + Send + 'static,
    worker: WorkerHandle,
    rt: Handle,
) -> Result<PlayerHandle> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<PlayerCommand>();
    let snapshot = Arc::new(Mutex::new(PlayerSnapshot::default()));
    let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

    let snap_for_thread = snapshot.clone();
    let join = std::thread::Builder::new()
        .name("conservatory-player".into())
        .spawn(move || {
            let host = match make_host() {
                Ok(host) => {
                    let _ = ready_tx.send(Ok(()));
                    host
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            Engine::new(host, worker, rt, snap_for_thread).run(cmd_rx);
        })
        .map_err(|e| Error::Player(format!("spawning player thread: {e}")))?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(PlayerHandle::new(cmd_tx, snapshot, join)),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(_) => Err(Error::Player("player thread exited before init".into())),
    }
}

struct Engine {
    host: MpvHost,
    worker: WorkerHandle,
    rt: Handle,
    snapshot: Arc<Mutex<PlayerSnapshot>>,
    queue: Vec<PlayableItem>,
    current: Option<usize>,
    paused: bool,
    volume: i64,
    ended: bool,
    debounce: StateDebounce,
    started: Instant,
    audio_devices: std::sync::Arc<[crate::player::host::AudioDevice]>,
    audio_device: Option<String>,
}

impl Engine {
    fn new(
        host: MpvHost,
        worker: WorkerHandle,
        rt: Handle,
        snapshot: Arc<Mutex<PlayerSnapshot>>,
    ) -> Self {
        // The output-device list is static-at-startup (spec §6.5); a failed query
        // (e.g. a null AO) just yields an empty list.
        let audio_devices = host.audio_devices().unwrap_or_default().into();
        Self {
            host,
            worker,
            rt,
            snapshot,
            queue: Vec::new(),
            current: None,
            paused: false,
            volume: 100,
            ended: false,
            debounce: StateDebounce::default(),
            started: Instant::now(),
            audio_devices,
            audio_device: None,
        }
    }

    fn run(mut self, rx: Receiver<PlayerCommand>) {
        loop {
            match self.host.pump(PUMP_TIMEOUT) {
                HostEvent::Ended(reason) => self.on_item_ended(reason),
                HostEvent::Shutdown => break,
                HostEvent::Idle => {}
            }

            let mut shutdown = false;
            loop {
                match rx.try_recv() {
                    Ok(cmd) => {
                        if self.handle_command(cmd) {
                            shutdown = true;
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        shutdown = true;
                        break;
                    }
                }
            }
            if shutdown {
                break;
            }

            // Steady-state insurance write (debounced, fire-and-forget).
            self.flush(StateEvent::Tick, false);
            self.refresh_snapshot();
        }

        // Terminal flush: guarantee the resume cursor lands before we exit.
        self.flush(StateEvent::Quit, true);
    }

    /// Returns `true` if the command was `Shutdown`.
    fn handle_command(&mut self, cmd: PlayerCommand) -> bool {
        match cmd {
            PlayerCommand::SetQueue {
                items,
                start,
                paused,
            } => {
                self.queue = items;
                if self.queue.is_empty() {
                    self.current = None;
                    self.ended = true;
                    self.paused = false;
                } else {
                    self.current = Some(start.min(self.queue.len() - 1));
                    self.ended = false;
                    self.load_current();
                    // Launch-resume loads paused so opening the app is silent.
                    if paused {
                        self.set_paused(true);
                    }
                    self.flush(StateEvent::Seek, false);
                }
            }
            PlayerCommand::AppendItems(mut items) => {
                let was_idle = self.current.is_none();
                let first_new = self.queue.len();
                self.queue.append(&mut items);
                // Appending to an idle/empty queue starts playing the first new
                // item; appending while playing just extends the tail.
                if was_idle && first_new < self.queue.len() {
                    self.current = Some(first_new);
                    self.ended = false;
                    self.load_current();
                    self.flush(StateEvent::Seek, false);
                }
            }
            PlayerCommand::Play => self.set_paused(false),
            PlayerCommand::Pause => {
                self.set_paused(true);
                self.flush(StateEvent::Pause, true);
            }
            PlayerCommand::TogglePause => {
                let paused = !self.paused;
                self.set_paused(paused);
                if paused {
                    self.flush(StateEvent::Pause, true);
                }
            }
            PlayerCommand::Next => self.skip_next(),
            PlayerCommand::Previous => self.skip_previous(),
            PlayerCommand::Seek(secs) => {
                let _ = self.host.seek_absolute(secs.max(0.0));
                self.flush(StateEvent::Seek, true);
            }
            PlayerCommand::SetVolume(v) => {
                self.volume = v.clamp(0, 100);
                let _ = self.host.set_volume(self.volume);
            }
            PlayerCommand::MoveItem { from, to } => {
                let len = self.queue.len();
                if from < len {
                    let to = to.min(len - 1);
                    let item = self.queue.remove(from);
                    self.queue.insert(to, item);
                    // The playing item keeps playing; only its index moves.
                    self.current = move_current_index(self.current, from, to);
                    self.flush(StateEvent::Seek, false);
                }
            }
            PlayerCommand::RemoveItem { index } => {
                if index < self.queue.len() {
                    self.queue.remove(index);
                    let outcome = remove_current_index(self.current, index, self.queue.len());
                    self.current = outcome.current;
                    if outcome.ended {
                        self.ended = true;
                        self.set_paused(false);
                        let _ = self.host.stop();
                    } else if outcome.reload {
                        // The playing item was removed; play what fell into its slot.
                        self.ended = false;
                        self.load_current();
                    }
                    self.flush(StateEvent::Seek, false);
                }
            }
            PlayerCommand::ClearQueue => {
                self.queue.clear();
                self.current = None;
                self.ended = true;
                self.paused = false;
                let _ = self.host.stop();
            }
            PlayerCommand::SetAudioDevice(name) => {
                if self.host.set_audio_device(&name).is_ok() {
                    self.audio_device = Some(name);
                }
            }
            PlayerCommand::Stop => {
                self.set_paused(true);
                self.flush(StateEvent::Quit, true);
            }
            PlayerCommand::Shutdown => return true,
        }
        self.refresh_snapshot();
        false
    }

    fn on_item_ended(&mut self, reason: EndReason) {
        match reason {
            // Natural completion: count the play, then advance.
            EndReason::Eof => {
                if let Some(item) = self.current_item() {
                    let track_id = item.track_id;
                    self.block_increment_play_count(track_id);
                }
                self.advance_after_end();
            }
            // The current item is unplayable: skip it (no play count), don't stall.
            EndReason::Errored => self.advance_after_end(),
            // Self-initiated (our own `load`/stop) or the host shutting down:
            // these are not item completions, so do nothing here.
            EndReason::Stopped | EndReason::Redirect | EndReason::Quit => {}
        }
        self.refresh_snapshot();
    }

    /// Step to the next item after an end-of-file / error. At the end of the
    /// queue, stop and leave `current` on the last item as the resume cursor.
    fn advance_after_end(&mut self) {
        let next = self.current.map_or(0, |i| i + 1);
        if next < self.queue.len() {
            self.current = Some(next);
            self.ended = false;
            self.load_current();
            self.flush(StateEvent::Seek, false);
        } else {
            self.ended = true;
            // Persist the finished item at offset 0 (the cursor for a resume).
            if let Some(item) = self.current_item() {
                let track_id = item.track_id;
                self.save_cursor(track_id, 0.0, true);
            }
        }
    }

    /// Manual skip forward: load the next item (the abandoned one emits an
    /// `Ended(Stopped)` we ignore). At the end, stop.
    fn skip_next(&mut self) {
        let next = self.current.map_or(0, |i| i + 1);
        if next < self.queue.len() {
            self.current = Some(next);
            self.ended = false;
            self.load_current();
            self.flush(StateEvent::Seek, false);
        } else {
            self.ended = true;
            self.set_paused(true);
            self.flush(StateEvent::Quit, true);
        }
    }

    /// Manual skip back: restart the current item if we're past the threshold,
    /// else step to the prior item (clamped at the start).
    fn skip_previous(&mut self) {
        let pos = self.host.time_pos().unwrap_or(0.0);
        if pos > PREVIOUS_RESTART_THRESHOLD || self.current.is_none_or(|i| i == 0) {
            let _ = self.host.seek_absolute(0.0);
        } else if let Some(i) = self.current {
            self.current = Some(i - 1);
            self.ended = false;
            self.load_current();
        }
        self.flush(StateEvent::Seek, true);
    }

    fn load_current(&mut self) {
        let Some(item) = self.current.and_then(|i| self.queue.get(i)) else {
            return;
        };
        let path = item.source.to_string_lossy().into_owned();
        let profile = item.profile;
        if let Err(e) = self.host.load(&path, &profile) {
            tracing::warn!(error = %e, path, "player: load failed");
        }
        let _ = self.host.set_volume(self.volume);
        self.paused = false;
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        let _ = self.host.set_paused(paused);
    }

    fn current_item(&self) -> Option<&PlayableItem> {
        self.current.and_then(|i| self.queue.get(i))
    }

    /// Persist the cursor for the relevant `event`, debounced. `Tick` writes at
    /// most once per insurance interval; the forced events always write. A
    /// `blocking` write waits for the worker's ack (terminal writes); otherwise
    /// it is fired and forgotten through the runtime.
    fn flush(&mut self, event: StateEvent, blocking: bool) {
        let now_ms = self.started.elapsed().as_millis() as u64;
        if !self.debounce.should_flush(now_ms, event) {
            return;
        }
        let Some(item) = self.current_item() else {
            return;
        };
        let track_id = item.track_id;
        let position = self.host.time_pos().unwrap_or(0.0);
        self.save_cursor(track_id, position, blocking);
    }

    fn save_cursor(&self, track_id: i64, position: f64, blocking: bool) {
        let worker = self.worker.clone();
        let paused = self.paused;
        let volume = self.volume;
        let updated = now_secs();
        let fut = async move {
            let _ = worker
                .save_playback_state(Some(track_id), position, paused, volume, updated)
                .await;
        };
        if blocking {
            self.rt.block_on(fut);
        } else {
            self.rt.spawn(fut);
        }
    }

    fn block_increment_play_count(&self, track_id: i64) {
        let worker = self.worker.clone();
        let when = now_secs();
        self.rt.block_on(async move {
            let _ = worker.increment_play_count(track_id, when).await;
        });
    }

    fn refresh_snapshot(&self) {
        let snap = PlayerSnapshot {
            current_index: self.current,
            track_id: self.current_item().map(|i| i.track_id),
            position: self.host.time_pos().unwrap_or(0.0),
            duration: self.host.duration(),
            paused: self.paused,
            volume: self.volume,
            queue_len: self.queue.len(),
            ended: self.ended,
            audio_devices: self.audio_devices.clone(),
            audio_device: self.audio_device.clone(),
        };
        if let Ok(mut guard) = self.snapshot.lock() {
            *guard = snap;
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Where `current` lands after the item at `from` moves to `to` (the same
/// `remove(from)` + `insert(to)` transform the queue uses, so the engine index
/// stays aligned with the DB positions). Pure.
pub(crate) fn move_current_index(current: Option<usize>, from: usize, to: usize) -> Option<usize> {
    let c = current?;
    Some(if c == from {
        to // the playing item is the one moved
    } else if from < c && c <= to {
        c - 1 // it shifted up to fill the gap left behind
    } else if to <= c && c < from {
        c + 1 // it shifted down to make room
    } else {
        c
    })
}

/// The result of removing a queue entry: where `current` lands, whether the
/// engine must reload (the playing item was the one removed and another fell
/// into its slot), and whether the queue has now ended.
pub(crate) struct RemoveOutcome {
    pub current: Option<usize>,
    pub reload: bool,
    pub ended: bool,
}

/// Where `current` lands after the item at `index` is removed; `new_len` is the
/// queue length *after* the removal. Pure.
pub(crate) fn remove_current_index(
    current: Option<usize>,
    index: usize,
    new_len: usize,
) -> RemoveOutcome {
    let Some(c) = current else {
        return RemoveOutcome {
            current: None,
            reload: false,
            ended: false,
        };
    };
    if index < c {
        RemoveOutcome {
            current: Some(c - 1),
            reload: false,
            ended: false,
        }
    } else if index > c {
        RemoveOutcome {
            current: Some(c),
            reload: false,
            ended: false,
        }
    } else if new_len == 0 {
        // Removed the only/last item that was playing: queue is empty.
        RemoveOutcome {
            current: None,
            reload: false,
            ended: true,
        }
    } else if index < new_len {
        // Another item fell into the playing slot: play it.
        RemoveOutcome {
            current: Some(index),
            reload: true,
            ended: false,
        }
    } else {
        // Removed the playing *last* item; nothing fell into the slot: queue ends.
        RemoveOutcome {
            current: Some(new_len - 1),
            reload: false,
            ended: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_index_when_current_is_the_moved_item() {
        assert_eq!(move_current_index(Some(2), 2, 5), Some(5));
        assert_eq!(move_current_index(Some(0), 0, 3), Some(3));
    }

    #[test]
    fn move_index_when_current_is_crossed() {
        // Moving an earlier item to after current shifts current up by one.
        assert_eq!(move_current_index(Some(3), 1, 5), Some(2));
        // Moving a later item to before current shifts current down by one.
        assert_eq!(move_current_index(Some(2), 5, 0), Some(3));
    }

    #[test]
    fn move_index_when_current_is_untouched() {
        assert_eq!(move_current_index(Some(1), 3, 5), Some(1));
        assert_eq!(move_current_index(Some(6), 1, 3), Some(6));
        assert_eq!(move_current_index(None, 1, 3), None);
    }

    #[test]
    fn remove_before_current_shifts_down() {
        let o = remove_current_index(Some(3), 1, 5);
        assert_eq!(o.current, Some(2));
        assert!(!o.reload && !o.ended);
    }

    #[test]
    fn remove_after_current_is_a_noop() {
        let o = remove_current_index(Some(2), 4, 5);
        assert_eq!(o.current, Some(2));
        assert!(!o.reload && !o.ended);
    }

    #[test]
    fn remove_current_reloads_the_next_in_slot() {
        // Queue had 5, remove the current (index 2); 4 remain, slot 2 reloads.
        let o = remove_current_index(Some(2), 2, 4);
        assert_eq!(o.current, Some(2));
        assert!(o.reload && !o.ended);
    }

    #[test]
    fn remove_current_last_ends_the_queue() {
        // Current was the last (index 4 of 5); after removal 4 remain, none in slot.
        let o = remove_current_index(Some(4), 4, 4);
        assert_eq!(o.current, Some(3));
        assert!(!o.reload && o.ended);
    }

    #[test]
    fn remove_current_only_item_empties() {
        let o = remove_current_index(Some(0), 0, 0);
        assert_eq!(o.current, None);
        assert!(!o.reload && o.ended);
    }
}
