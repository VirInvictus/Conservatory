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

use crate::db::models::ResamplerQuality;
use crate::db::{DspState, EqState, MediaKind};
use crate::player::host::AudioDevice;
use crate::player::item::PlayableItem;
use crate::player::sleep::{SleepMode, SleepStatus};

/// A command sent from a consumer to the engine thread.
pub enum PlayerCommand {
    /// Replace the queue and cue `start`. Plays immediately unless `paused`
    /// (the launch-resume path loads paused so the app makes no sound on open).
    SetQueue {
        items: Vec<PlayableItem>,
        start: usize,
        paused: bool,
    },
    /// Append items to the queue tail (live; starts playing if the queue was
    /// idle). Mirrors `worker.enqueue_tracks`.
    AppendItems(Vec<PlayableItem>),
    Play,
    Pause,
    TogglePause,
    Next,
    Previous,
    /// Skip to the next (`+1`) or previous (`-1`) chapter of the current item
    /// (Phase 6c-iii-b): an absolute seek to the neighbouring `ChapterMark`. A
    /// no-op when the item has no chapters. The shared mechanism the audiobook
    /// engine reuses at 7c (with `book_chapters`).
    SkipChapter(i32),
    /// Seek to an absolute offset in seconds.
    Seek(f64),
    /// Set the output volume (0–100).
    SetVolume(i64),
    /// Move the queue entry at `from` to `to` (live reorder; the playing item
    /// keeps playing, its index follows). Mirrors `worker.reorder_queue`.
    MoveItem {
        from: usize,
        to: usize,
    },
    /// Remove the queue entry at `index` (live; removing the current item
    /// advances to what fell into its slot). Mirrors `worker.remove_queue_item`.
    RemoveItem {
        index: usize,
    },
    /// Empty the queue and stop playback (keeps the thread alive).
    ClearQueue,
    /// Switch the audio output device (mpv `audio-device`).
    SetAudioDevice(String),
    /// Switch the output backend (Phase 5.5c-ii); mpv's `ao` driver, applied live
    /// via `ao-reload`.
    SetOutputBackend(String),
    /// Set the resampler quality (Phase 5.5c-ii); the `audio-resample-*` knobs,
    /// applied immediately and from the next loaded item.
    SetResamplerQuality(ResamplerQuality),
    /// Set the active equalizer (Phase 5.5b); applied live when playing, else
    /// from the next loaded item.
    SetEq(EqState),
    /// Set one EQ band's gain live (Phase 5.5b-ii); gap-free via `af-command`.
    SetEqBand {
        index: usize,
        gain: f64,
    },
    /// Set the active DSP modules (Phase 5.5c); applied live when playing (a
    /// structural rebuild), else from the next loaded item.
    SetDsp(DspState),
    /// Arm (`Some`) or cancel (`None`) the sleep timer (Phase 6c-iii-d). A duration
    /// timer pauses after N seconds of playback; the boundary modes pause at the
    /// end of the current item / queue.
    SetSleepTimer(Option<SleepMode>),
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
    /// The current item's media kind, so the UI knows whether `track_id` is a
    /// track or an episode id and reads the right metadata (v0.0.38). `None` when
    /// nothing is loaded.
    pub kind: Option<MediaKind>,
    pub position: f64,
    pub duration: Option<f64>,
    pub paused: bool,
    /// The current item streams from a remote URL (an undownloaded episode), not
    /// a local file (v0.0.38).
    pub streaming: bool,
    /// mpv is waiting on the network/cache (not producing audio while meant to be
    /// playing): drives the Now-bar "Buffering…" indicator (v0.0.38).
    pub buffering: bool,
    pub volume: i64,
    pub queue_len: usize,
    /// The queue has been played to its end (or is empty): a poller can stop.
    pub ended: bool,
    /// The current item's chapter count (Phase 6c-iii-b); `0` for a track or a
    /// chapter-less episode. Drives the Now-bar chapter buttons' visibility.
    pub chapter_count: usize,
    /// The chapter the playhead is in (index into the item's marks), or `None`
    /// before the first chapter / when there are none.
    pub current_chapter: Option<usize>,
    /// The current item's profile has Smart Speed on (Phase 6c-iii-c): drives the
    /// Now Playing "Smart Speed" indicator. `false` for music / shows without it.
    pub smart_speed_active: bool,
    /// Seconds saved so far in the current listening session by Smart Speed +
    /// speed-up (`SessionAccumulator::smart_speed_saved`); `0.0` when no session
    /// is open. Shown live in the indicator's label / tooltip.
    pub smart_speed_saved: f64,
    /// The audio output devices (queried once at engine init, spec §6.5).
    pub audio_devices: Arc<[AudioDevice]>,
    /// The selected output device id; `None` is mpv's default (`auto`).
    pub audio_device: Option<String>,
    /// The armed sleep timer (Phase 6c-iii-d), or `None` when no timer is set.
    /// Drives the Now-bar sleep button label and the Now Playing "Sleep · …" line.
    pub sleep: Option<SleepStatus>,
}

impl Default for PlayerSnapshot {
    fn default() -> Self {
        Self {
            current_index: None,
            track_id: None,
            kind: None,
            position: 0.0,
            duration: None,
            paused: false,
            streaming: false,
            buffering: false,
            volume: 100,
            queue_len: 0,
            ended: false,
            chapter_count: 0,
            current_chapter: None,
            smart_speed_active: false,
            smart_speed_saved: 0.0,
            audio_devices: Arc::from([]),
            audio_device: None,
            sleep: None,
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
        let _ = self.tx.send(PlayerCommand::SetQueue {
            items,
            start,
            paused: false,
        });
    }

    /// Replace the queue with `items` cued at `start`, **paused**, and seek to
    /// `position` (the launch-resume path: load where the user left off without
    /// auto-playing).
    pub fn resume(&self, items: Vec<PlayableItem>, start: usize, position: f64) {
        let _ = self.tx.send(PlayerCommand::SetQueue {
            items,
            start,
            paused: true,
        });
        if position > 0.0 {
            let _ = self.tx.send(PlayerCommand::Seek(position));
        }
    }

    /// Append items to the queue tail (starts playing if the queue was idle).
    pub fn append(&self, items: Vec<PlayableItem>) {
        let _ = self.tx.send(PlayerCommand::AppendItems(items));
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

    /// Skip to the next (`dir > 0`) or previous (`dir <= 0`) chapter of the
    /// current item (Phase 6c-iii-b). A no-op when the item has no chapters.
    pub fn skip_chapter(&self, dir: i32) {
        let _ = self.tx.send(PlayerCommand::SkipChapter(dir));
    }

    pub fn seek(&self, secs: f64) {
        let _ = self.tx.send(PlayerCommand::Seek(secs));
    }

    pub fn set_volume(&self, volume: i64) {
        let _ = self.tx.send(PlayerCommand::SetVolume(volume));
    }

    /// Reorder the live queue: move the entry at `from` to `to`.
    pub fn move_item(&self, from: usize, to: usize) {
        let _ = self.tx.send(PlayerCommand::MoveItem { from, to });
    }

    /// Remove the live queue entry at `index`.
    pub fn remove_item(&self, index: usize) {
        let _ = self.tx.send(PlayerCommand::RemoveItem { index });
    }

    /// Empty the live queue and stop playback.
    pub fn clear_queue(&self) {
        let _ = self.tx.send(PlayerCommand::ClearQueue);
    }

    /// Switch the audio output device (spec §6.5).
    pub fn set_audio_device(&self, name: impl Into<String>) {
        let _ = self.tx.send(PlayerCommand::SetAudioDevice(name.into()));
    }

    /// Switch the output backend (Phase 5.5c-ii, spec §6.5): mpv's `ao` driver.
    pub fn set_output_backend(&self, backend: impl Into<String>) {
        let _ = self
            .tx
            .send(PlayerCommand::SetOutputBackend(backend.into()));
    }

    /// Set the resampler quality (Phase 5.5c-ii, spec §6.5).
    pub fn set_resampler_quality(&self, quality: ResamplerQuality) {
        let _ = self.tx.send(PlayerCommand::SetResamplerQuality(quality));
    }

    /// Set the active equalizer (Phase 5.5b): a preset switch / launch state.
    pub fn set_eq(&self, eq: EqState) {
        let _ = self.tx.send(PlayerCommand::SetEq(eq));
    }

    /// Set one EQ band's gain live (Phase 5.5b-ii): the slider-drag path.
    pub fn set_eq_band(&self, index: usize, gain: f64) {
        let _ = self.tx.send(PlayerCommand::SetEqBand { index, gain });
    }

    /// Set the active DSP modules (Phase 5.5c): compressor / limiter / leveler.
    pub fn set_dsp(&self, dsp: DspState) {
        let _ = self.tx.send(PlayerCommand::SetDsp(dsp));
    }

    /// Arm (`Some`) or cancel (`None`) the sleep timer (Phase 6c-iii-d).
    pub fn set_sleep_timer(&self, mode: Option<SleepMode>) {
        let _ = self.tx.send(PlayerCommand::SetSleepTimer(mode));
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
        if let Ok(mut guard) = self.join.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
    }
}
