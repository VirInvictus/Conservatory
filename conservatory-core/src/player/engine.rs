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

use crate::db::{MediaKind, PlaybackCursor, WorkerHandle};
use crate::errors::{Error, Result};
use crate::player::book::locate;
use crate::player::chapters::{current_chapter_at, neighbour_chapter};
use crate::player::handle::{PlayerCommand, PlayerHandle, PlayerSnapshot};
use crate::player::host::{HostEvent, MpvHost};
use crate::player::item::PlayableItem;
use crate::player::mode::Repeat;
use crate::player::scrobble_progress::ScrobbleProgress;
use crate::player::session::{SessionAccumulator, SessionOwner};
use crate::player::shuffle::{apply_permutation, shuffle_order};
use crate::player::sleep::{SleepClock, SleepMode};
use crate::player::state::{EndReason, StateDebounce, StateEvent};
use crate::scrobble::ScrobbleService;

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
    /// The active episode's Smart Speed time-saved accumulator (Phase 6c-ii):
    /// `Some` only while an episode plays. Closed (one `listening_sessions` row)
    /// at each episode boundary.
    session: Option<SessionAccumulator>,
    /// When the session was last sampled, for the per-tick wall-clock delta.
    session_tick: Instant,
    /// The current scrobbleable play's submission-rule accounting (Phase 9d):
    /// `Some` while a music track or podcast episode is the active item (never a
    /// book). Ticked with playtime each loop turn and finalized (a listen queued
    /// if `eligible()` and scrobbling is on) whenever the item leaves the slot.
    /// Independent of `session`, which is spoken-word-only.
    scrobble_progress: Option<ScrobbleProgress>,
    /// The armed sleep timer (Phase 6c-iii-d), or `None` when unset. Ticked each
    /// loop turn; the boundary modes are enforced at the EOF / advance points.
    sleep: Option<SleepClock>,
    /// When the sleep clock was last ticked, for its wall-clock delta.
    sleep_tick: Instant,
    /// Which file of the current audiobook is loaded (Phase 7c): an index into
    /// the current item's `segments`. `0` for non-books and at the head of every
    /// freshly-loaded book; the engine bumps it as it advances file to file
    /// within the one queue item. Used with the segment's cumulative `start` to
    /// report a book-absolute position.
    book_segment: usize,
    /// Stop-after-current is armed (Phase 11d): the engine pauses at the end of
    /// the current item instead of playing on, then disarms. Consulted at the
    /// EOF boundary alongside the `EndOfItem` sleep mode.
    stop_after_current: bool,
    /// The next queue item is appended to mpv's internal playlist (the gapless
    /// prefetch, v0.1.22): at the current track's EOF mpv decodes straight into
    /// it and the engine advances bookkeeping instead of reloading
    /// ([`Self::advance_into_prefetched`]). Only ever true for a track→track
    /// boundary with gapless profiles and no stop-at-boundary armed
    /// ([`should_prefetch`]); every queue mutation resyncs it.
    prefetched: bool,
    /// The active repeat mode (Phase 17a): consulted at the EOF boundary. `One`
    /// replays the current item; `All` wraps the queue at its end; `Off` stops.
    repeat: Repeat,
    /// Shuffle is on (Phase 17b): a repeat-all lap reshuffles the queue in place,
    /// and the flag is the ReplayGain context (Phase 17c). The one-shot shuffle of
    /// the live queue is driven by the GUI (a `ReorderQueue` command), so the DB
    /// queue is persisted in the same step.
    shuffle: bool,
    /// The scrobble target, or `None` when scrobbling is off (Phase 9b). Set from
    /// `[scrobble]` at startup and on a Preferences change. When `Some`, a natural
    /// track / episode EOF enqueues a listen into the outbox; audiobooks never do.
    scrobble: Option<ScrobbleService>,
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
            session: None,
            session_tick: Instant::now(),
            scrobble_progress: None,
            sleep: None,
            sleep_tick: Instant::now(),
            book_segment: 0,
            stop_after_current: false,
            prefetched: false,
            repeat: Repeat::Off,
            shuffle: false,
            scrobble: None,
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

            // Smart Speed time-saved sampling (Phase 6c-ii).
            self.sample_session();
            // Sleep-timer countdown (Phase 6c-iii-d): pause when a duration timer
            // elapses.
            self.tick_sleep();
            // Steady-state insurance write (debounced, fire-and-forget).
            self.flush(StateEvent::Tick, false);
            self.refresh_snapshot();
        }

        // Terminal flush: guarantee the resume cursor lands before we exit.
        self.flush(StateEvent::Quit, true);
        // Close an episode session still open at shutdown (a mid-episode quit
        // still records the listening so far).
        self.close_session();
    }

    /// Sample the active episode session once per loop: accrue real vs audio time
    /// while playing, or just resync the playhead while paused / ended so idle
    /// time never inflates the saved figure. `time-pos` may be unknown right after
    /// a load; we skip the sample then (Phase 6c-ii).
    fn sample_session(&mut self) {
        if self.session.is_none() && self.scrobble_progress.is_none() {
            return;
        }
        let dt = self.session_tick.elapsed().as_secs_f64();
        self.session_tick = Instant::now();
        // Book-absolute for a book, so the accounting spans its files (7c-ii).
        let pos = self.playhead_opt();
        let idle = self.paused || self.ended;
        if let (Some(acc), Some(pos)) = (self.session.as_mut(), pos) {
            if idle {
                acc.resync(pos);
            } else {
                acc.tick(dt, pos);
            }
        }
        // Scrobble playtime (Phase 9d): accrue only while playing; learn the
        // duration once the host decodes it (0 right after load). Read the
        // duration before the mutable borrow to keep the field borrows disjoint.
        let duration = self.host.duration().unwrap_or(0.0);
        if let Some(prog) = self.scrobble_progress.as_mut() {
            if !idle {
                prog.tick(dt);
            }
            prog.observe_duration(duration);
        }
    }

    /// Advance the sleep clock once per loop turn (Phase 6c-iii-d). A duration
    /// timer counts down only while actually playing; when it elapses, pause
    /// playback and persist (the `Pause` command's behaviour), leaving the
    /// tap-to-extend window open for a re-arm on the next `Play`.
    fn tick_sleep(&mut self) {
        let dt = self.sleep_tick.elapsed().as_secs_f64();
        self.sleep_tick = Instant::now();
        let playing = !self.paused && !self.ended;
        let fired = self.sleep.as_mut().is_some_and(|c| c.tick(dt, playing));
        if fired {
            self.set_paused(true);
            self.flush(StateEvent::Pause, true);
            self.refresh_snapshot();
        } else if self.sleep.as_ref().is_some_and(|c| c.spent()) {
            // The tap-to-extend window lapsed without a resume: the timer is done,
            // so disarm it (the UI returns to "no timer" rather than lingering).
            self.sleep = None;
            self.refresh_snapshot();
        }
    }

    /// Returns `true` if the command was `Shutdown`.
    fn handle_command(&mut self, cmd: PlayerCommand) -> bool {
        match cmd {
            PlayerCommand::SetQueue {
                items,
                start,
                paused,
            } => {
                // Replacing the queue ends the current episode's session and
                // finalizes the leaving item's scrobble (Phase 9d) before the old
                // queue is gone.
                self.close_session();
                self.finalize_scrobble();
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
                } else {
                    // The tail grew: a playing last item may now have a next
                    // worth prefetching.
                    self.resync_prefetch();
                }
            }
            PlayerCommand::InsertItems { at, mut items } => {
                if !items.is_empty() {
                    let was_idle = self.current.is_none();
                    let at = at.min(self.queue.len());
                    let k = items.len();
                    // Splice `items` in at `at`: [0..at) + items + [at..].
                    let tail = self.queue.split_off(at);
                    self.queue.append(&mut items);
                    self.queue.extend(tail);
                    if was_idle {
                        // Inserting into an idle queue starts the first new item.
                        self.current = Some(at);
                        self.ended = false;
                        self.load_current();
                    } else {
                        // The playing item keeps playing; its index shifts past
                        // the inserted block (Play Next inserts after it, so this
                        // is a no-op there, but a general insert can precede it).
                        self.current = insert_current_index(self.current, at, k);
                        // Play Next changes what follows the playing item.
                        self.resync_prefetch();
                    }
                    self.flush(StateEvent::Seek, false);
                }
            }
            PlayerCommand::Play => {
                self.set_paused(false);
                self.sleep_on_play();
            }
            PlayerCommand::Pause => {
                self.set_paused(true);
                self.flush(StateEvent::Pause, true);
            }
            PlayerCommand::TogglePause => {
                let paused = !self.paused;
                self.set_paused(paused);
                if paused {
                    self.flush(StateEvent::Pause, true);
                } else {
                    self.sleep_on_play();
                }
            }
            PlayerCommand::Next => self.skip_next(),
            PlayerCommand::Previous => self.skip_previous(),
            PlayerCommand::SkipChapter(dir) => self.skip_chapter(dir),
            PlayerCommand::Seek(secs) => {
                let target = secs.max(0.0);
                // Book-absolute for a book: map to the right file + in-file offset
                // (Phase 7c-ii). This is also the launch-resume path.
                self.seek_book_absolute(target);
                // A user seek is a jump, not audio played: exclude its interval
                // from the time-saved accounting (Phase 6c-ii). This is also the
                // launch-resume path (the resume offset Seek after SetQueue), so
                // the jump to the saved position is not counted as covered audio.
                if let Some(acc) = self.session.as_mut() {
                    acc.seek(target);
                }
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
                    // A reorder can change which item follows the playing one.
                    self.resync_prefetch();
                    self.flush(StateEvent::Seek, false);
                }
            }
            PlayerCommand::RemoveItem { index } => {
                if index < self.queue.len() {
                    self.queue.remove(index);
                    let outcome = remove_current_index(self.current, index, self.queue.len());
                    self.current = outcome.current;
                    if outcome.ended {
                        // Removing the playing item ends its session (the reload
                        // branch closes via `load_current`).
                        self.close_session();
                        self.ended = true;
                        self.set_paused(false);
                        // mpv `stop` clears its internal playlist, prefetch
                        // entry included.
                        self.prefetched = false;
                        let _ = self.host.stop();
                    } else if outcome.reload {
                        // The playing item was removed; play what fell into its slot.
                        self.ended = false;
                        self.load_current();
                    } else {
                        // A neighbour was removed: what follows the playing
                        // item may have changed.
                        self.resync_prefetch();
                    }
                    self.flush(StateEvent::Seek, false);
                }
            }
            PlayerCommand::ClearQueue => {
                self.close_session();
                self.queue.clear();
                self.current = None;
                self.ended = true;
                self.paused = false;
                // mpv `stop` clears its internal playlist, prefetch entry
                // included.
                self.prefetched = false;
                let _ = self.host.stop();
            }
            PlayerCommand::SetAudioDevice(name) => {
                if self.host.set_audio_device(&name).is_ok() {
                    self.audio_device = Some(name);
                }
            }
            PlayerCommand::SetOutputBackend(backend) => {
                // mpv's `ao` driver: applied live via `ao-reload` (gap-acceptable).
                let _ = self.host.set_output_backend(&backend);
            }
            PlayerCommand::SetResamplerQuality(quality) => {
                // The `audio-resample-*` knobs: applied now and from the next load.
                let _ = self.host.set_resampler(quality);
            }
            PlayerCommand::SetEq(eq) => {
                // A preset switch / launch state: applied live when playing
                // (structural rebuild), else from the next load.
                self.host.set_eq(eq);
            }
            PlayerCommand::SetEqBand { index, gain } => {
                // The slider-drag path: live, gap-free, via `af-command`.
                let _ = self.host.set_eq_band(index, gain);
            }
            PlayerCommand::SetDsp(dsp) => {
                // The DSP modules: applied live when playing (structural rebuild),
                // else from the next load.
                self.host.set_dsp(dsp);
            }
            PlayerCommand::SetSmartSpeedLevel(level) => {
                // The global Smart Speed aggressiveness: live when a spoken-word
                // item with Smart Speed on is playing, else from the next load.
                self.host.set_smart_speed_level(level);
            }
            PlayerCommand::SetSpoken {
                speed,
                smart_speed,
                voice_boost,
            } => {
                // Live spoken-word change for the current episode / book (structural
                // rebuild); a no-op when nothing is loaded.
                self.host.set_spoken(speed, smart_speed, voice_boost);
            }
            PlayerCommand::SetSleepTimer(mode) => {
                // Arm a fresh clock (or cancel). Reset the tick baseline so the
                // first countdown sample is a small delta, not the gap since the
                // last timer.
                self.sleep = mode.map(SleepClock::new);
                self.sleep_tick = Instant::now();
                // An EndOfItem boundary must not hand off gaplessly (the next
                // track would start sounding before the pause); cancelling
                // re-arms the prefetch.
                self.resync_prefetch();
            }
            PlayerCommand::SetStopAfterCurrent(on) => {
                self.stop_after_current = on;
                // Same boundary rule as the EndOfItem sleep mode above.
                self.resync_prefetch();
            }
            PlayerCommand::SetRepeat(mode) => {
                self.repeat = mode;
                // `One` replays the current item, so the coming boundary must not
                // hand off gaplessly to the next track; re-derive the prefetch
                // (also re-arms it when leaving `One`). The wrap boundary of `All`
                // is handled at end-of-queue, not here.
                self.resync_prefetch();
            }
            PlayerCommand::SetShuffle(on) => {
                self.shuffle = on;
                // Re-apply context-aware ReplayGain to the current track live
                // (Phase 17c): album gain ↔ track gain follows the shuffle state, a
                // gap-free chain rebuild (the set_eq path). Spoken word carries no
                // RG so this is a no-op there. Extract the profile before touching
                // the host to release the queue borrow. No reorder here: the GUI
                // drives that via `ReorderQueue` so the DB persists the same perm.
                let live = self
                    .current_item()
                    .filter(|i| i.kind == MediaKind::Track)
                    .map(|i| i.profile.contextual(on));
                if let Some(profile) = live {
                    let _ = self.host.apply_profile(&profile);
                }
            }
            PlayerCommand::ReorderQueue(perm) => self.reorder_queue(perm),
            PlayerCommand::SetScrobble(service) => self.scrobble = service,
            PlayerCommand::Stop => {
                self.set_paused(true);
                self.flush(StateEvent::Quit, true);
                self.close_session();
                // A stop ends the current play: submit it if it earned a scrobble
                // (Phase 9d). The playtime so far decides, not reaching the end.
                self.finalize_scrobble();
            }
            PlayerCommand::Shutdown => {
                // A clean shutdown ends the current play: finalize its scrobble
                // before the loop exits (Phase 9d), so quitting mid-track does not
                // silently drop a listen that met the threshold.
                self.finalize_scrobble();
                return true;
            }
        }
        self.refresh_snapshot();
        false
    }

    fn on_item_ended(&mut self, reason: EndReason) {
        match reason {
            // Natural completion: record the completed play per kind, then
            // advance. A track bumps `tracks.play_count`; an episode marks its
            // podcast `playback` row PlayedFully + bumps its count (6b-ii-c-2).
            // `track_id` carries the episode id for an episode item.
            EndReason::Eof => {
                // A book plays through its files as ONE queue item: a non-final
                // file's EOF advances to the next file *internally* (no queue
                // advance, the session stays open), and only the last file's EOF
                // completes the book (spec §6.1). Handle that first.
                if self.advance_book_segment() {
                    self.refresh_snapshot();
                    return;
                }
                if let Some((kind, id)) = self.current_item().map(|i| (i.kind, i.track_id)) {
                    match kind {
                        MediaKind::Track => self.block_increment_play_count(id),
                        MediaKind::Episode => self.block_complete_episode(id),
                        MediaKind::Audiobook => self.block_complete_book(id),
                    }
                    // The scrobble is not enqueued here (Phase 9d): a natural EOF
                    // is one of several ways a play ends, so `finalize_scrobble`
                    // fires from the advance paths below (and skip / stop / queue
                    // replace), gated on the submission rule rather than on
                    // reaching EOF. The play-count bump above stays EOF-only.
                    //
                    // But a natural EOF *is* a full listen, so mark the progress
                    // complete: the advance's finalize then scrobbles it on
                    // completion (subject to the 30s floor), not on wall-clock.
                    if let Some(prog) = self.scrobble_progress.as_mut() {
                        prog.mark_complete();
                    }
                }
                // Append the listening session at the natural boundary, before
                // advancing (Phase 6c-ii). `advance_after_end`'s `load_current`
                // would also close it, so this is the explicit close for the
                // last-item case, where no next load happens.
                self.close_session();
                // Sleep timer "end of episode/track" (Phase 6c-iii-d): cue the
                // next item but pause there, so playback stops at this boundary
                // (the user can resume into the next item); then disarm.
                let stop_after_item = matches!(
                    self.sleep.as_ref().map(|c| c.mode()),
                    Some(SleepMode::EndOfItem)
                );
                // Stop-after-current (Phase 11d) shares the EndOfItem boundary
                // behaviour: cue the next item but pause there, then disarm.
                // (Arming either clears the prefetch, so the branches below
                // never race a hand-off that has already started sounding.)
                let stop_after_current = self.stop_after_current;
                // Repeat::One replays the current item (Phase 17a): reload it from
                // the head instead of advancing. A one-shot stop (stop-after-current
                // or the end-of-item sleep) is an explicit override that wins, so it
                // still pauses at the boundary; those fall through to the advance
                // path below. `One` never prefetches (`should_prefetch`), so there
                // is no gapless hand-off to reconcile here.
                if self.repeat == Repeat::One && !stop_after_item && !stop_after_current {
                    self.ended = false;
                    self.load_current();
                    self.flush(StateEvent::Seek, false);
                } else {
                    if self.prefetched {
                        // mpv already crossed into the appended next track
                        // gaplessly; sync bookkeeping instead of reloading.
                        self.advance_into_prefetched();
                    } else {
                        self.advance_after_end();
                    }
                    if stop_after_item || stop_after_current {
                        if stop_after_item {
                            self.sleep = None;
                        }
                        self.stop_after_current = false;
                        if !self.ended {
                            self.set_paused(true);
                            self.flush(StateEvent::Pause, true);
                        }
                    }
                }
            }
            // The current item is unplayable: skip it (no play count), don't stall.
            EndReason::Errored => {
                self.close_session();
                if self.prefetched {
                    // An errored end still advances mpv into the appended next
                    // entry; same bookkeeping hand-off as the natural EOF.
                    self.advance_into_prefetched();
                } else {
                    self.advance_after_end();
                }
            }
            // Self-initiated (our own `load`/stop) or the host shutting down:
            // these are not item completions, so do nothing here.
            EndReason::Stopped | EndReason::Redirect | EndReason::Quit => {}
        }
        self.refresh_snapshot();
    }

    /// Append the next queue item to mpv's internal playlist when the coming
    /// boundary qualifies for a gapless hand-off ([`should_prefetch`]). A
    /// failed append just leaves the reactive `load_current` path in charge.
    fn maybe_prefetch_next(&mut self) {
        if self.prefetched {
            return;
        }
        let stop_boundary = self.stop_after_current
            || self.repeat == Repeat::One
            || matches!(
                self.sleep.as_ref().map(|c| c.mode()),
                Some(SleepMode::EndOfItem)
            );
        let (current, next) = match self.current {
            Some(i) => (self.queue.get(i), self.queue.get(i + 1)),
            None => (None, None),
        };
        if !should_prefetch(current, next, stop_boundary) {
            return;
        }
        let path = next
            .expect("should_prefetch requires a next item")
            .source
            .to_string_lossy()
            .into_owned();
        match self.host.append_next(&path) {
            Ok(()) => {
                tracing::debug!(source = %path, "player: prefetched next track (gapless)");
                self.prefetched = true;
            }
            Err(e) => tracing::warn!(error = %e, path, "player: prefetch append failed"),
        }
    }

    /// Re-derive the prefetch after anything that may have changed what "next"
    /// means (a queue mutation, a stop-at-boundary toggle): drop the appended
    /// entry, then re-append if the new boundary still qualifies. Idempotent
    /// and cheap, so callers use it unconditionally.
    fn resync_prefetch(&mut self) {
        if self.prefetched {
            let _ = self.host.clear_prefetch();
            self.prefetched = false;
        }
        self.maybe_prefetch_next();
    }

    /// The prefetched next track is already sounding (mpv crossed the file
    /// boundary gaplessly on its own): advance the engine's bookkeeping to
    /// match and apply the new track's profile — notably its `@rg` ReplayGain
    /// head — to the live stream instead of reloading it (the reload is what
    /// used to make every album transition audibly gap). The counterpart of
    /// [`Self::advance_after_end`] for the prefetched case.
    fn advance_into_prefetched(&mut self) {
        // The outgoing track completed (a gapless EOF hand-off): finalize its
        // scrobble before the bookkeeping moves on (Phase 9d).
        self.finalize_scrobble();
        self.prefetched = false;
        let next = self.current.map_or(0, |i| i + 1);
        if next >= self.queue.len() {
            // Defensive: a prefetch should imply a next item; fall back to the
            // reactive path rather than strand the engine.
            self.advance_after_end();
            return;
        }
        self.current = Some(next);
        self.ended = false;
        self.book_segment = 0;
        if let Some(profile) = self
            .queue
            .get(next)
            .map(|i| i.profile.contextual(self.shuffle))
            && let Err(e) = self.host.apply_profile(&profile)
        {
            tracing::warn!(error = %e, "player: profile hand-off failed at gapless boundary");
        }
        // Only track→track boundaries prefetch, so there is no spoken-word
        // session to open; the ended item's was closed by the caller.
        self.session = None;
        self.session_tick = Instant::now();
        // Start scrobble accounting for the track that just took over gaplessly
        // (Phase 9d). Only tracks prefetch, so this is always a Track.
        if let Some((kind, id)) = self.queue.get(next).map(|i| (i.kind, i.track_id)) {
            self.begin_scrobble(kind, id);
        }
        self.paused = false;
        self.flush(StateEvent::Seek, false);
        self.maybe_prefetch_next();
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
            // The last item ended and nothing reloads here (a real stop, unless a
            // Repeat::All wrap follows): finalize its scrobble now (Phase 9d). A
            // wrap's load_current would find the progress already taken.
            self.finalize_scrobble();
            // Repeat::All wraps back to the top instead of stopping (Phase 17a). A
            // sleep timer set to "end of queue" is an explicit stop that wins, so
            // the wrap is suppressed when it is armed (the timer then fires below).
            // With shuffle on the lap is reshuffled first (Phase 17b, wired here).
            let sleep_end_of_queue = matches!(
                self.sleep.as_ref().map(|c| c.mode()),
                Some(SleepMode::EndOfQueue)
            );
            if self.repeat == Repeat::All && !self.queue.is_empty() && !sleep_end_of_queue {
                self.wrap_to_top();
                return;
            }
            self.ended = true;
            // A sleep timer set to "end of queue" is satisfied here (Phase
            // 6c-iii-d); disarm it.
            if sleep_end_of_queue {
                self.sleep = None;
            }
            // Persist the finished item at offset 0 as the resume cursor. Per
            // kind (6b-ii-c-2): the cursor records `kind` + the right id, so a
            // restart reopens an episode, not just the last track. This writes
            // only the singleton cursor; the episode's `playback` row was already
            // finalized by `block_complete_episode` in `on_item_ended`, so we do
            // not re-touch it here (that would undo PlayedFully).
            if let Some((kind, id)) = self.current_item().map(|i| (i.kind, i.track_id)) {
                self.save_cursor(kind, id, 0.0, true);
            }
        }
    }

    /// Wrap the queue back to the top for a Repeat::All lap (Phase 17a). Loads the
    /// first item and keeps playing. With shuffle on (Phase 17b) each lap is a
    /// fresh order: the queue is reshuffled in place and the *same* permutation is
    /// persisted to the DB queue, so the engine and the DB stay lock-step.
    fn wrap_to_top(&mut self) {
        if self.shuffle && self.queue.len() > 1 {
            let perm = shuffle_order(self.queue.len(), 0, seed_now());
            self.queue = apply_permutation(&self.queue, &perm);
            let worker = self.worker.clone();
            self.rt.block_on(async move {
                let _ = worker.reorder_queue_by_positions(perm).await;
            });
        }
        self.current = Some(0);
        self.ended = false;
        self.load_current();
        self.flush(StateEvent::Seek, false);
    }

    /// Apply a queue permutation (`perm[new] = old`), Phase 17b: rebuild the queue
    /// in the new order and move `current` to wherever the playing item landed. A
    /// perm whose length does not match the queue (a stale GUI snapshot) is ignored
    /// so the engine and the DB — which guards identically — stay lock-step. The
    /// playing item keeps playing; only its index changes.
    fn reorder_queue(&mut self, perm: Vec<usize>) {
        if perm.len() != self.queue.len() {
            return;
        }
        self.queue = apply_permutation(&self.queue, &perm);
        if let Some(c) = self.current {
            self.current = perm.iter().position(|&old| old == c);
        }
        // A reorder can change which item follows the playing one.
        self.resync_prefetch();
        self.flush(StateEvent::Seek, false);
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
            self.close_session();
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
            // Restarting is a jump, not rewound audio: exclude it (Phase 6c-ii).
            if let Some(acc) = self.session.as_mut() {
                acc.seek(0.0);
            }
        } else if let Some(i) = self.current {
            self.current = Some(i - 1);
            self.ended = false;
            self.load_current();
        }
        self.flush(StateEvent::Seek, true);
    }

    /// Skip to the neighbouring chapter of the current item (Phase 6c-iii-b): an
    /// absolute seek to the next / previous `ChapterMark`, clamped at the ends. A
    /// no-op when the item has no chapters or the skip would run off the end
    /// (`neighbour_chapter` returns `None`). The shared mechanism 7c reuses.
    fn skip_chapter(&mut self, dir: i32) {
        let chapters = match self.current_item() {
            Some(item) if !item.chapters.is_empty() => item.chapters.clone(),
            _ => return,
        };
        // Book-absolute for a book (its marks are absolute too), so a skip can
        // cross a file boundary; `seek_book_absolute` loads the right file.
        let pos = self.playhead();
        let Some(target) = neighbour_chapter(&chapters, pos, dir) else {
            return;
        };
        self.seek_book_absolute(target);
        // A chapter skip is a jump, not audio played: exclude it from the
        // time-saved accounting, like a user seek (Phase 6c-ii).
        if let Some(acc) = self.session.as_mut() {
            acc.seek(target);
        }
        self.flush(StateEvent::Seek, true);
    }

    fn load_current(&mut self) {
        // Close the session for the item we are leaving before loading the next
        // (Phase 6c-ii). Idempotent, so a boundary that already closed (EOF) is a
        // no-op here.
        self.close_session();
        // Finalize the leaving item's scrobble before the reload replaces it
        // (Phase 9d): this is the common choke point for a skip, an EOF advance,
        // a repeat-one restart, and an explicit load. Idempotent.
        self.finalize_scrobble();
        // The replace-mode loadfile below clears mpv's internal playlist, so any
        // prefetched entry is gone with it; re-derived at the end of this load.
        self.prefetched = false;
        // A freshly-loaded item starts at the head of its file list: `source` is
        // the first segment for a book (Phase 7c), so loading it below loads file
        // zero; reset the segment cursor to match. The resume path (7c-ii) seeks
        // to a later segment after this load.
        self.book_segment = 0;
        let Some(item) = self.current.and_then(|i| self.queue.get(i)) else {
            return;
        };
        let path = item.source.to_string_lossy().into_owned();
        // Context-aware ReplayGain (Phase 17c): album gain in order, track gain
        // when shuffling. A no-op for spoken word (no RG carried).
        let profile = item.profile.contextual(self.shuffle);
        let kind = item.kind;
        // `track_id` carries the episode id for an episode item (the queue's
        // per-kind id field, 6b-ii-c).
        let episode_id = item.track_id;
        let streaming = item.streaming;
        tracing::debug!(?kind, streaming, source = %path, index = ?self.current, "player: loading item");
        if let Err(e) = self.host.load(&path, &profile) {
            tracing::warn!(error = %e, path, "player: load failed");
        }
        let _ = self.host.set_volume(self.volume);
        // Sync mpv's pause state to "playing". load() (loadfile) inherits mpv's
        // prior pause property, so without this an item loaded after a paused one
        // (notably the launch-resume queue, which loads paused) would come up
        // paused while the engine and UI think it is playing — the "had to press
        // pause then play" bug. A caller that wants paused (launch-resume) sets it
        // again right after, in SetQueue.
        let _ = self.host.set_paused(false);
        self.paused = false;
        // Start a Smart Speed accounting session for a spoken-word item (episode
        // or book, Phase 6c-ii / 7c-ii). The launch-resume / explicit Seek that
        // may follow excludes its jump to the resume offset, so the resume is not
        // counted as covered audio. `episode_id` carries the per-kind id; for a
        // book the session spans its files (the engine feeds it book-absolute
        // positions), opened once here and closed at the book's completion.
        let pos = self.host.time_pos().unwrap_or(0.0);
        let now = now_secs();
        self.session = match kind {
            MediaKind::Episode => {
                Some(SessionAccumulator::new(episode_id, now, profile.speed, pos))
            }
            MediaKind::Audiobook => Some(SessionAccumulator::new_book(
                episode_id,
                now,
                profile.speed,
                pos,
            )),
            MediaKind::Track => None,
        };
        self.session_tick = Instant::now();
        // Start scrobble accounting for the new item (Phase 9d): a track or an
        // episode, keyed by the play's start time. A book clears it.
        self.begin_scrobble(kind, episode_id);
        // With the new item playing, line up the gapless hand-off for its end.
        self.maybe_prefetch_next();
    }

    /// On a book file's EOF: if another file follows, load it (an internal file
    /// advance, Phase 7c) and report `true` — the caller then skips the normal
    /// end-of-item completion, so the queue does not advance and the listening
    /// session stays open across the file boundary. `false` for non-books and at
    /// the book's last file (the book is finished; the caller completes it).
    fn advance_book_segment(&mut self) -> bool {
        let Some(item) = self.current_item() else {
            return false;
        };
        if item.kind != MediaKind::Audiobook {
            return false;
        }
        if self.book_segment + 1 >= item.segments.len() {
            return false; // the last (or only) file ended → finished
        }
        self.book_segment += 1;
        self.load_book_segment(self.book_segment);
        self.flush(StateEvent::Seek, false);
        true
    }

    /// Load the current book's `idx`-th file without touching `book_segment` (the
    /// caller owns it) or the running session — the internal-advance counterpart
    /// to [`Self::load_current`] (Phase 7c). Used by the file advance and, at
    /// 7c-ii, by the resume / cross-file seek.
    fn load_book_segment(&mut self, idx: usize) {
        let Some(item) = self.current.and_then(|i| self.queue.get(i)) else {
            return;
        };
        let Some(seg) = item.segments.get(idx) else {
            return;
        };
        let path = seg.file.to_string_lossy().into_owned();
        let profile = item.profile;
        tracing::debug!(segment = idx, source = %path, "player: loading book file");
        if let Err(e) = self.host.load(&path, &profile) {
            tracing::warn!(error = %e, path, "player: book file load failed");
        }
        let _ = self.host.set_volume(self.volume);
        let _ = self.host.set_paused(false);
        self.paused = false;
    }

    /// Close the active episode session, if any, appending one append-only
    /// `listening_sessions` row (Phase 6c-ii). Idempotent (`take`), and blocking
    /// so the row lands before the engine moves on (the ledger discipline of the
    /// terminal cursor write). A no-op when no episode is playing.
    fn close_session(&mut self) {
        let Some(acc) = self.session.take() else {
            return;
        };
        let worker = self.worker.clone();
        // The session row is keyed by the owning item — an episode by `episode_id`,
        // a book by `book_id` (Phase 7c-ii); the time-saved math is identical.
        let (episode_id, book_id) = match acc.owner {
            SessionOwner::Episode => (Some(acc.item_id), None),
            SessionOwner::Book => (None, Some(acc.item_id)),
        };
        let started_at = acc.started_at;
        let ended_at = now_secs();
        let real = acc.real_seconds();
        let audio = acc.audio_seconds();
        let saved = acc.smart_speed_saved();
        self.rt.block_on(async move {
            let _ = worker
                .insert_listening_session(
                    episode_id, book_id, started_at, ended_at, real, audio, saved,
                )
                .await;
        });
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        let _ = self.host.set_paused(paused);
    }

    /// Tap-to-extend (Phase 6c-iii-d): on a (re)start of playback, a sleep timer
    /// that just fired re-arms its interval instead of merely resuming. A no-op
    /// when no timer is armed, it has not fired, or the window has lapsed.
    fn sleep_on_play(&mut self) {
        if let Some(clock) = self.sleep.as_mut() {
            clock.on_play();
        }
    }

    fn current_item(&self) -> Option<&PlayableItem> {
        self.current.and_then(|i| self.queue.get(i))
    }

    /// Persist the resume state for the relevant `event`, debounced. `Tick`
    /// writes at most once per insurance interval; the forced events always
    /// write. A `blocking` write waits for the worker's ack (terminal writes);
    /// otherwise it is fired and forgotten through the runtime.
    ///
    /// Per-kind (6b-ii-c-2): a track writes only the singleton `playback_state`
    /// cursor; an episode writes the cursor (so a restart reopens it) **and** its
    /// own podcast `playback` row (the per-episode resume position + InProgress,
    /// which survives moving on to other items). An audiobook is Phase 7.
    fn flush(&mut self, event: StateEvent, blocking: bool) {
        let now_ms = self.started.elapsed().as_millis() as u64;
        if !self.debounce.should_flush(now_ms, event) {
            return;
        }
        let Some((kind, id)) = self.current_item().map(|i| (i.kind, i.track_id)) else {
            return;
        };
        // Book-absolute for a book (Phase 7c-ii), per-file for a track/episode.
        let position = self.playhead();
        self.save_cursor(kind, id, position, blocking);
        // Persist the per-item resume position only while still playing. Once the
        // queue has `ended` the current item already completed (its terminal write
        // set PlayedFully / finished); a Quit flush re-marking it InProgress would
        // clobber that. The synchronous write keeps it ordered with the completion
        // on the single writer.
        if !self.ended {
            match kind {
                MediaKind::Episode => self.persist_episode_position(id, position),
                MediaKind::Audiobook => self.persist_book_position(id, position),
                MediaKind::Track => {}
            }
        }
    }

    /// Write the singleton transport cursor for the current item's `kind`
    /// (6b-ii-c-2): `track_id` is set for a track, `episode_id` for an episode
    /// (`id` carries whichever). The audiobook cursor lands at Phase 7.
    fn save_cursor(&self, kind: MediaKind, id: i64, position: f64, blocking: bool) {
        let (track_id, episode_id, book_id) = match kind {
            MediaKind::Track => (Some(id), None, None),
            MediaKind::Episode => (None, Some(id), None),
            MediaKind::Audiobook => (None, None, Some(id)),
        };
        let cursor = PlaybackCursor {
            kind,
            track_id,
            episode_id,
            book_id,
            position,
            paused: self.paused,
            volume: self.volume,
            updated_at: now_secs(),
        };
        let worker = self.worker.clone();
        let fut = async move {
            let _ = worker.save_playback_state(cursor).await;
        };
        if blocking {
            self.rt.block_on(fut);
        } else {
            self.rt.spawn(fut);
        }
    }

    /// Persist an episode's per-episode resume position (its podcast `playback`
    /// row, marked InProgress), separate from the singleton cursor so it survives
    /// after the queue moves on to other items (6b-ii-c-2). Synchronous: it
    /// touches `playback.played`, so it must land in order with the terminal
    /// `complete_episode` (see `flush`). Episode position writes fire on the
    /// debounced insurance interval / forced points, so blocking here is cheap.
    fn persist_episode_position(&self, episode_id: i64, position: f64) {
        let worker = self.worker.clone();
        let when = now_secs();
        self.rt.block_on(async move {
            let _ = worker
                .set_episode_position(episode_id, position, Some(when))
                .await;
        });
    }

    /// Persist a book's absolute resume position (`book_playback.position`, spec
    /// §6.4), the audiobook analogue of `persist_episode_position`. `position` is
    /// book-absolute (across the book's files), since the engine speaks that
    /// timeline for a book (Phase 7c-ii). Blocking, on the insurance interval /
    /// forced points; `set_book_position` preserves `finished` + the overrides.
    fn persist_book_position(&self, book_id: i64, position: f64) {
        let worker = self.worker.clone();
        let when = now_secs();
        self.rt.block_on(async move {
            let _ = worker
                .set_book_position(book_id, position, Some(when))
                .await;
        });
    }

    /// The current playhead in the timeline the engine speaks: **book-absolute**
    /// for a book (the current segment's cumulative `start` plus the host's
    /// per-file `time_pos`), the host's `time_pos` for a track / episode (Phase
    /// 7c-ii). `None` when nothing is loaded.
    fn playhead_opt(&self) -> Option<f64> {
        let raw = self.host.time_pos()?;
        Some(match self.current_item() {
            Some(item) if item.kind == MediaKind::Audiobook && !item.segments.is_empty() => {
                let seg = &item.segments[self.book_segment.min(item.segments.len() - 1)];
                seg.start + raw
            }
            _ => raw,
        })
    }

    /// [`Self::playhead_opt`] defaulting to `0.0` when nothing is loaded.
    fn playhead(&self) -> f64 {
        self.playhead_opt().unwrap_or(0.0)
    }

    /// Seek to `abs` in the current item's timeline. For a book `abs` is
    /// book-absolute: find the segment containing it, load that file if it is not
    /// the one playing, and seek to the in-file offset, spanning the file boundary
    /// a multi-file book introduces (Phase 7c-ii). For a track / episode it is a
    /// plain absolute seek.
    fn seek_book_absolute(&mut self, abs: f64) {
        let abs = abs.max(0.0);
        let segments = match self.current_item() {
            Some(item) if item.kind == MediaKind::Audiobook && !item.segments.is_empty() => {
                item.segments.clone()
            }
            _ => {
                let _ = self.host.seek_absolute(abs);
                return;
            }
        };
        let Some((idx, offset)) = locate(&segments, abs) else {
            let _ = self.host.seek_absolute(abs);
            return;
        };
        if idx != self.book_segment {
            self.book_segment = idx;
            self.load_book_segment(idx);
        }
        let _ = self.host.seek_absolute(offset);
    }

    fn block_increment_play_count(&self, track_id: i64) {
        let worker = self.worker.clone();
        let when = now_secs();
        self.rt.block_on(async move {
            let _ = worker.increment_play_count(track_id, when).await;
        });
    }

    /// Episode end-of-file: mark its podcast `playback` row PlayedFully + bump
    /// its play_count (the episode analogue of `block_increment_play_count`).
    fn block_complete_episode(&self, episode_id: i64) {
        let worker = self.worker.clone();
        let when = now_secs();
        self.rt.block_on(async move {
            let _ = worker.complete_episode(episode_id, Some(when)).await;
        });
    }

    /// Start scrobble-progress accounting for a freshly-loaded item (Phase 9d).
    /// Only music tracks and podcast episodes are scrobbled (a book is not a
    /// "listen"), so a book clears any progress. Progress runs regardless of
    /// whether scrobbling is currently on; `finalize_scrobble` makes the on/off
    /// decision at the end, so toggling mid-track behaves predictably. The
    /// duration is 0 here if the host has not decoded it yet; `sample_session`
    /// learns it on the next tick.
    fn begin_scrobble(&mut self, kind: MediaKind, id: i64) {
        self.scrobble_progress = match kind {
            MediaKind::Track | MediaKind::Episode => Some(ScrobbleProgress::new(
                kind,
                id,
                now_secs(),
                self.host.duration().unwrap_or(0.0),
            )),
            MediaKind::Audiobook => None,
        };
    }

    /// Finalize the current play's scrobble (Phase 9d): if scrobbling is on and
    /// the play met the submission rule (`ScrobbleProgress::eligible`), enqueue
    /// the listen stamped with the play's *start* time (the protocol keys a
    /// scrobble by when it began, not when it ended). The listen metadata is
    /// resolved atomically off the writer connection (`enqueue_scrobble_for`).
    /// Idempotent: it takes the progress, so a second call (a transition that
    /// funnels through more than one exit point) is a no-op. Called at every
    /// point the current item leaves the slot: a reload, a gapless hand-off, the
    /// end of the queue, a stop, a queue replace, and shutdown.
    fn finalize_scrobble(&mut self) {
        let Some(prog) = self.scrobble_progress.take() else {
            return;
        };
        let Some(service) = self.scrobble else {
            return; // scrobbling off: the play is over, just discard it.
        };
        if !prog.eligible() {
            return;
        }
        let worker = self.worker.clone();
        let svc = service.as_str().to_string();
        let (kind, id, started_at) = (prog.kind, prog.id, prog.started_at);
        self.rt.block_on(async move {
            let _ = worker.enqueue_scrobble_for(kind, id, svc, started_at).await;
        });
    }

    /// Book end-of-file (the last file): mark `book_playback` finished and clear
    /// its resume position (Phase 7c, spec §6.4), the audiobook analogue of
    /// `block_complete_episode`.
    fn block_complete_book(&self, book_id: i64) {
        let worker = self.worker.clone();
        let when = now_secs();
        self.rt.block_on(async move {
            let _ = worker.complete_book(book_id, Some(when)).await;
        });
    }

    fn refresh_snapshot(&self) {
        let current = self.current_item();
        // For a book, the host's per-file `time_pos` / `duration` are lifted to
        // **book-absolute** time via the current segment's cumulative `start`
        // (Phase 7c), so the seek slider, chapter highlight (the marks are
        // absolute too), and resume all speak one timeline across the book's
        // files. Tracks / episodes are a single file, so they pass through.
        let raw = self.host.time_pos().unwrap_or(0.0);
        let (position, duration) = match current {
            Some(item) if item.kind == MediaKind::Audiobook && !item.segments.is_empty() => {
                let seg = &item.segments[self.book_segment.min(item.segments.len() - 1)];
                let total = item.segments.last().map(|s| s.start + s.duration);
                (seg.start + raw, total)
            }
            _ => (raw, self.host.duration()),
        };
        let snap = PlayerSnapshot {
            current_index: self.current,
            track_id: current.map(|i| i.track_id),
            kind: current.map(|i| i.kind),
            position,
            duration,
            channels: self.host.channels(),
            paused: self.paused,
            streaming: current.is_some_and(|i| i.streaming),
            // Buffering only matters while we mean to be playing: a paused or
            // ended player is idle on purpose, not stalled on the network.
            buffering: !self.paused && !self.ended && self.host.is_buffering(),
            volume: self.volume,
            queue_len: self.queue.len(),
            ended: self.ended,
            chapter_count: current.map_or(0, |i| i.chapters.len()),
            current_chapter: current.and_then(|i| current_chapter_at(&i.chapters, position)),
            smart_speed_active: current.is_some_and(|i| i.profile.smart_speed),
            smart_speed_saved: self.session.as_ref().map_or(0.0, |s| s.smart_speed_saved()),
            audio_devices: self.audio_devices.clone(),
            audio_device: self.audio_device.clone(),
            sleep: self.sleep.as_ref().map(|c| c.status()),
            stop_after_current: self.stop_after_current,
            repeat: self.repeat,
            shuffle: self.shuffle,
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

/// A wall-clock-derived seed for the shuffle reshuffle (Phase 17b). Mixes the
/// seconds and nanoseconds so two reshuffles in the same second differ. Not
/// cryptographic; a play-queue shuffle needs only unpredictability, not security.
fn seed_now() -> u64 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (d.as_secs() << 20) ^ u64::from(d.subsec_nanos())
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

/// Where `current` lands after `k` items are inserted at `at` (indices at or
/// after `at` shift up by `k`; earlier indices are untouched). Pure. The idle
/// case (no `current`) is handled by the caller, which starts the first new item.
pub(crate) fn insert_current_index(current: Option<usize>, at: usize, k: usize) -> Option<usize> {
    let c = current?;
    Some(if c >= at { c + k } else { c })
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

/// Whether the coming item boundary qualifies for the gapless prefetch
/// (v0.1.22): both sides must be gapless-profile **tracks** (spoken word never
/// hands off gaplessly: episodes and books carry sessions, resume seeks, and
/// their own chains, and gapless between speech items is meaningless), and no
/// stop-at-boundary (stop-after-current / end-of-item sleep) may be armed,
/// because a prefetched hand-off would start the next track sounding before
/// the engine pauses it. Pure.
pub(crate) fn should_prefetch(
    current: Option<&PlayableItem>,
    next: Option<&PlayableItem>,
    stop_boundary: bool,
) -> bool {
    if stop_boundary {
        return false;
    }
    match (current, next) {
        (Some(c), Some(n)) => {
            c.kind == MediaKind::Track
                && n.kind == MediaKind::Track
                && c.profile.gapless
                && n.profile.gapless
        }
        _ => false,
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
    fn insert_at_or_before_current_shifts_it_up() {
        // Play Next inserts after current: current is untouched.
        assert_eq!(insert_current_index(Some(2), 3, 2), Some(2));
        // Inserting exactly at current pushes it past the block.
        assert_eq!(insert_current_index(Some(2), 2, 3), Some(5));
        // Inserting before current shifts it up by the block size.
        assert_eq!(insert_current_index(Some(4), 1, 2), Some(6));
        // No current (idle) stays None; the caller starts the first new item.
        assert_eq!(insert_current_index(None, 0, 3), None);
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

    fn item(kind: MediaKind, gapless: bool) -> PlayableItem {
        use crate::player::profile::MusicProfile;
        PlayableItem {
            track_id: 1,
            source: std::path::PathBuf::from("/m/x.flac"),
            profile: MusicProfile {
                gapless,
                replaygain_db: None,
                rg_album: None,
                rg_track: None,
                speed: 1.0,
                pitch_correction: false,
                smart_speed: false,
                voice_boost: false,
            },
            album_id: None,
            kind,
            streaming: false,
            chapters: Vec::new().into(),
            segments: Vec::new().into(),
        }
    }

    #[test]
    fn prefetch_wants_a_gapless_track_to_track_boundary() {
        let t = item(MediaKind::Track, true);
        assert!(should_prefetch(Some(&t), Some(&t), false));
    }

    #[test]
    fn prefetch_refuses_spoken_word_and_gapless_off() {
        let t = item(MediaKind::Track, true);
        let e = item(MediaKind::Episode, false);
        let b = item(MediaKind::Audiobook, false);
        let plain = item(MediaKind::Track, false);
        // Any spoken-word side disqualifies the boundary.
        assert!(!should_prefetch(Some(&t), Some(&e), false));
        assert!(!should_prefetch(Some(&e), Some(&t), false));
        assert!(!should_prefetch(Some(&t), Some(&b), false));
        // Gapless off (either side) disqualifies it too.
        assert!(!should_prefetch(Some(&plain), Some(&t), false));
        assert!(!should_prefetch(Some(&t), Some(&plain), false));
    }

    #[test]
    fn prefetch_refuses_boundaries_that_must_stop() {
        let t = item(MediaKind::Track, true);
        // Stop-after-current / EndOfItem sleep: the hand-off would start the
        // next track sounding before the engine pauses it.
        assert!(!should_prefetch(Some(&t), Some(&t), true));
        // No next (or nothing playing): nothing to prefetch.
        assert!(!should_prefetch(Some(&t), None, false));
        assert!(!should_prefetch(None, Some(&t), false));
    }
}
