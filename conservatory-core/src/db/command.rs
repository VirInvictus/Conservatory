//! Worker command enum (spec §2.1).
//!
//! Each variant carries a `oneshot::Sender` reply channel so callers await
//! their own result. Consumers never construct `Command` directly: they call
//! typed methods on [`crate::db::WorkerHandle`]. The enum is internal to `db`.
//!
//! Phase 1a carried the debug round-trip; Phase 1b adds the music inserts the
//! import pipeline and fixture builder need. Update/delete and queue/podcast
//! commands land in later sub-phases.

use tokio::sync::oneshot;

use crate::db::models::{Album, Artist, Chapter, Episode, Playback, Show, ShowSettings, Track};
use crate::edit::{AlbumEdit, TrackEdit};
use crate::errors::Result;
use crate::mover::journal::JobState;
use crate::mover::{MoveKind, MoveMode, MoveOp};

pub(crate) enum Command {
    /// Write a key/value through the debug probe table (Phase 1a artifact).
    ProbeWrite {
        key: String,
        value: String,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Insert an artist, returning its new id.
    InsertArtist {
        artist: Artist,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Resolve an artist by sort_name, creating it on first sight (import).
    GetOrCreateArtist {
        name: String,
        sort_name: String,
        musicbrainz_id: Option<String>,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Resolve an album by (album_artist_id, title), creating it on first sight.
    GetOrCreateAlbum {
        album: Album,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Set an album's shelf genre (a path-affecting edit; re-organize to move).
    SetAlbumShelfGenre {
        album_id: i64,
        shelf_genre: String,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Insert an album, returning its new id.
    InsertAlbum {
        album: Album,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Apply a track-level field edit (Phase 5a, spec §3.5).
    UpdateTrack {
        track_id: i64,
        edit: TrackEdit,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Apply an album-level field edit (Phase 5a). Album-level fields are
    /// path-affecting; the caller re-renders and moves.
    UpdateAlbum {
        album_id: i64,
        edit: AlbumEdit,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Replace a track's raw genre set (Phase 5a, the §5.2 multi-value side).
    SetTrackGenres {
        track_id: i64,
        genres: Vec<String>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Update a track's ReplayGain values after a scan (Phase 5c).
    SetTrackReplayGain {
        track_id: i64,
        track_gain: Option<f64>,
        album_gain: Option<f64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Set an album's cover path, optionally refreshing the accent (Phase 5d).
    SetAlbumCoverPath {
        album_id: i64,
        cover_path: Option<String>,
        accent_rgb: Option<u32>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Insert a track, returning its new id.
    InsertTrack {
        track: Track,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Resolve a raw genre name to its id, creating it on first sight.
    GetOrCreateGenre {
        name: String,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Link a track to a genre (idempotent).
    LinkTrackGenre {
        track_id: i64,
        genre_id: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Journal a move job and all its operations (`pending`) atomically, before
    /// any file is touched (spec §5.4). Returns the new job id.
    CreateMoveJob {
        kind: MoveKind,
        mode: MoveMode,
        library_root: String,
        created_at: i64,
        ops: Vec<MoveOp>,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Mark an operation done and apply the DB path it implies (track file_path
    /// + album folder_path), in one transaction.
    CompleteOperation {
        op_id: i64,
        track_id: Option<i64>,
        album_id: Option<i64>,
        db_new_path: Option<String>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Restore the pre-move DB path and reset the operation to pending (undo).
    RevertOperation {
        op_id: i64,
        track_id: Option<i64>,
        album_id: Option<i64>,
        db_old_path: Option<String>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Set a job's lifecycle state.
    SetJobState {
        job_id: i64,
        state: JobState,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Save (insert or overwrite by name) a Perspective, returning its id.
    SavePerspective {
        name: String,
        expression: String,
        scope: String,
        created_at: i64,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Delete a Perspective by id.
    DeletePerspective {
        id: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Upsert the singleton playback cursor (what is playing and where), so a
    /// restart resumes (spec §6.4, Phase 4a).
    SavePlaybackState {
        track_id: Option<i64>,
        position: f64,
        paused: bool,
        volume: i64,
        updated_at: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Bump a track's `play_count` and stamp `last_played` on a completed play
    /// (spec §6.4).
    IncrementPlayCount {
        track_id: i64,
        played_at: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Append tracks to the unified queue tail (spec §4.3, Phase 4b).
    EnqueueTracks {
        track_ids: Vec<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Replace the whole queue with these tracks in order ("play these now").
    ReplaceQueueWithTracks {
        track_ids: Vec<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Remove the queue entry at `position`, closing the gap.
    RemoveQueueItem {
        position: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Move the queue entry at `from` to `to`, keeping positions contiguous.
    ReorderQueue {
        from: i64,
        to: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Empty the queue.
    ClearQueue { reply: oneshot::Sender<Result<()>> },

    // --- Podcasts (Phase 6a-i, spec §4.2). The schema is core-owned, so these
    // commands live here; the `conservatory-podcasts` plugin calls the typed
    // `WorkerHandle` methods that build them.
    /// Resolve a show by `feed_url`, creating it on first sight (`podcast add`).
    GetOrCreateShow {
        show: Show,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Update a subscription in full (incl. the conditional-GET bookkeeping).
    UpdateShow {
        show: Show,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Delete a subscription (cascades episodes/playback/settings/…).
    DeleteShow {
        id: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Insert or update an episode by `(show_id, guid)`, returning its id.
    UpsertEpisode {
        episode: Episode,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Upsert an episode's triage/playback row.
    UpsertPlayback {
        playback: Playback,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Upsert a show's per-show overrides.
    UpsertShowSettings {
        settings: ShowSettings,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Replace an episode's chapter set.
    ReplaceChapters {
        episode_id: i64,
        chapters: Vec<Chapter>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Resolve a tag name to its id, creating it on first sight.
    GetOrCreateTag {
        name: String,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Replace a show's tag set.
    SetShowTags {
        show_id: i64,
        tags: Vec<String>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Ack a shutdown request. The loop exits naturally once every
    /// `WorkerHandle` clone has dropped and the channel closes.
    Shutdown { reply: oneshot::Sender<()> },

    /// Test-only: panic inside the handler to exercise the
    /// panic-catch-and-restart loop. The reply is dropped by the panic, which
    /// the caller observes as `WorkerChannelClosed`.
    #[cfg(test)]
    Panic,
}

impl Command {
    /// Stable string name for tracing instrumentation.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::ProbeWrite { .. } => "probe_write",
            Self::InsertArtist { .. } => "insert_artist",
            Self::GetOrCreateArtist { .. } => "get_or_create_artist",
            Self::GetOrCreateAlbum { .. } => "get_or_create_album",
            Self::SetAlbumShelfGenre { .. } => "set_album_shelf_genre",
            Self::InsertAlbum { .. } => "insert_album",
            Self::UpdateTrack { .. } => "update_track",
            Self::UpdateAlbum { .. } => "update_album",
            Self::SetTrackGenres { .. } => "set_track_genres",
            Self::SetTrackReplayGain { .. } => "set_track_replaygain",
            Self::SetAlbumCoverPath { .. } => "set_album_cover_path",
            Self::InsertTrack { .. } => "insert_track",
            Self::GetOrCreateGenre { .. } => "get_or_create_genre",
            Self::LinkTrackGenre { .. } => "link_track_genre",
            Self::CreateMoveJob { .. } => "create_move_job",
            Self::CompleteOperation { .. } => "complete_operation",
            Self::RevertOperation { .. } => "revert_operation",
            Self::SetJobState { .. } => "set_job_state",
            Self::SavePerspective { .. } => "save_perspective",
            Self::DeletePerspective { .. } => "delete_perspective",
            Self::SavePlaybackState { .. } => "save_playback_state",
            Self::IncrementPlayCount { .. } => "increment_play_count",
            Self::EnqueueTracks { .. } => "enqueue_tracks",
            Self::ReplaceQueueWithTracks { .. } => "replace_queue_with_tracks",
            Self::RemoveQueueItem { .. } => "remove_queue_item",
            Self::ReorderQueue { .. } => "reorder_queue",
            Self::ClearQueue { .. } => "clear_queue",
            Self::GetOrCreateShow { .. } => "get_or_create_show",
            Self::UpdateShow { .. } => "update_show",
            Self::DeleteShow { .. } => "delete_show",
            Self::UpsertEpisode { .. } => "upsert_episode",
            Self::UpsertPlayback { .. } => "upsert_playback",
            Self::UpsertShowSettings { .. } => "upsert_show_settings",
            Self::ReplaceChapters { .. } => "replace_chapters",
            Self::GetOrCreateTag { .. } => "get_or_create_tag",
            Self::SetShowTags { .. } => "set_show_tags",
            Self::Shutdown { .. } => "shutdown",
            #[cfg(test)]
            Self::Panic => "panic",
        }
    }
}
