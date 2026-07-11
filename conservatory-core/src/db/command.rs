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

use crate::db::models::{
    Album, ApeStripRow, Artist, AudioState, Book, BookChapter, BookPlayback, Chapter,
    EQ_BAND_COUNT, Episode, EqState, NewScrobble, Playback, PlaybackCursor, PlayedState,
    PlaylistKind, PlaylistOrder, Show, ShowSettings, Track, VerifyResultRow,
};
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

    /// Mark an operation done and apply the DB path it implies, in one
    /// transaction: a track's file_path and album's folder_path, or a book's
    /// chapter rows and folder_path.
    CompleteOperation {
        op_id: i64,
        track_id: Option<i64>,
        album_id: Option<i64>,
        book_id: Option<i64>,
        db_old_path: Option<String>,
        db_new_path: Option<String>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Restore the pre-move DB path and reset the operation to pending (undo).
    RevertOperation {
        op_id: i64,
        track_id: Option<i64>,
        album_id: Option<i64>,
        book_id: Option<i64>,
        db_old_path: Option<String>,
        db_new_path: Option<String>,
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

    /// Overwrite the singleton active EQ state (Phase 5.5b).
    SetEqState {
        state: EqState,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Save (insert or overwrite by name) a named EQ preset.
    SaveEqPreset {
        name: String,
        bands: [f64; EQ_BAND_COUNT],
        reply: oneshot::Sender<Result<()>>,
    },

    /// Delete a named EQ preset.
    DeleteEqPreset {
        name: String,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Overwrite the singleton active audio configuration (Phase 5.5c).
    SetAudioState {
        state: AudioState,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Upsert the singleton playback cursor (what is playing and where), so a
    /// restart resumes (spec §6.4, Phase 4a).
    SavePlaybackState {
        cursor: PlaybackCursor,
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

    /// Remove a track from the library (Phase 16a). DB-only unlink; the file is
    /// left on disk. Cascades/triggers clean up dependents.
    DeleteTrack {
        track_id: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Insert tracks into the queue at `at`, shifting later entries up (the Play
    /// Next path, Phase 16a; mirrors the engine `InsertItems`).
    InsertQueueTracksAt {
        at: i64,
        track_ids: Vec<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Insert audiobooks into the queue at `at` (16.5h: the book Play Next).
    InsertQueueBooksAt {
        at: i64,
        book_ids: Vec<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Remove a book from the library (16.5h). DB-only unlink; the files are
    /// left on disk. Cascades/triggers clean up dependents.
    DeleteBook {
        book_id: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Replace the whole queue with these tracks in order ("play these now").
    ReplaceQueueWithTracks {
        track_ids: Vec<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Append episodes to the queue tail (Phase 6b-ii-c).
    EnqueueEpisodes {
        episode_ids: Vec<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Replace the whole queue with these episodes in order ("play these now").
    ReplaceQueueWithEpisodes {
        episode_ids: Vec<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Append books to the queue tail (Phase 7c-iii).
    EnqueueBooks {
        book_ids: Vec<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Replace the whole queue with these books in order ("play these now").
    ReplaceQueueWithBooks {
        book_ids: Vec<i64>,
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
    /// Reorder the whole queue by a permutation (`perm[new] = old`), the shuffle
    /// path (Phase 17b). Applied in lock-step with the engine's `ReorderQueue`.
    ReorderQueueByPositions {
        perm: Vec<usize>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Empty the queue.
    ClearQueue { reply: oneshot::Sender<Result<()>> },

    // --- Playlists (Phase 16d). Storage only; smart materialisation is the
    // caller's job (it needs the search grammar, which core is free of).
    CreatePlaylist {
        name: String,
        kind: PlaylistKind,
        query: Option<String>,
        limit_n: Option<i64>,
        order: Option<PlaylistOrder>,
        created_at: i64,
        reply: oneshot::Sender<Result<i64>>,
    },
    DeletePlaylist {
        id: i64,
        reply: oneshot::Sender<Result<()>>,
    },
    RenamePlaylist {
        id: i64,
        name: String,
        reply: oneshot::Sender<Result<()>>,
    },
    AppendPlaylistTracks {
        playlist_id: i64,
        track_ids: Vec<i64>,
        reply: oneshot::Sender<Result<()>>,
    },
    RemovePlaylistEntry {
        playlist_id: i64,
        position: i64,
        reply: oneshot::Sender<Result<()>>,
    },
    ReorderPlaylistEntry {
        playlist_id: i64,
        from: i64,
        to: i64,
        reply: oneshot::Sender<Result<()>>,
    },

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

    /// Record an episode's downloaded `audio_path` (the one field `upsert_episode`
    /// deliberately preserves on re-fetch, so download sets it explicitly).
    SetEpisodeAudioPath {
        episode_id: i64,
        audio_path: String,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Clear an episode's downloaded `audio_path` (retention prune, Phase
    /// 6b-ii-c-3-b): the file has been deleted from disk, so the row reverts to
    /// stream-only.
    ClearEpisodeAudioPath {
        episode_id: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Upsert an episode's triage/playback row.
    UpsertPlayback {
        playback: Playback,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Set an episode's played state (triage), preserving starred/play_count.
    SetEpisodePlayed {
        episode_id: i64,
        state: PlayedState,
        when: Option<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Toggle an episode's starred flag (triage), preserving played/position.
    SetEpisodeStarred {
        episode_id: i64,
        starred: bool,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Persist an episode's resume position during playback (the engine's tick /
    /// pause / seek write, Phase 6b-ii-c-2); marks InProgress, preserves
    /// starred/play_count.
    SetEpisodePosition {
        episode_id: i64,
        position: f64,
        when: Option<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Record an episode played through to the end (the engine's natural-EOF
    /// write, Phase 6b-ii-c-2); marks PlayedFully, bumps play_count.
    CompleteEpisode {
        episode_id: i64,
        when: Option<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Append one listening session (Phase 6c-ii): the engine's per-episode-boundary
    /// time-saved record. Append-only.
    InsertListeningSession {
        episode_id: Option<i64>,
        book_id: Option<i64>,
        started_at: i64,
        ended_at: i64,
        real_seconds: f64,
        audio_seconds: f64,
        smart_speed_saved: f64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Enqueue one listen into `scrobble_outbox` (Phase 9a). Snapshots the
    /// metadata at completion time; the background submitter drains it.
    EnqueueScrobble {
        scrobble: NewScrobble,
        created_at: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Delete an outbox row after a successful submission (Phase 9a).
    DeleteScrobble {
        id: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Record a failed submission attempt (Phase 9a): bump `attempts`, push
    /// `next_attempt_at` to the backoff point.
    BumpScrobbleAttempt {
        id: i64,
        next_attempt_at: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Upsert a batch of integrity-verification results (Phase 8a), one tx.
    UpsertVerifyResults {
        rows: Vec<VerifyResultRow>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Record an APE-strip undo row before a file is stripped (Phase 8c-iii).
    RecordApeStrip {
        row: ApeStripRow,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Delete an APE-strip undo row after a strip is undone (Phase 8c-iii).
    DeleteApeStrip {
        file_path: String,
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

    // --- Audiobooks (spec §4.5, Phase 7a-i) ---
    /// Resolve an audiobook person by `sort_name`, creating on first sight.
    GetOrCreateBookPerson {
        name: String,
        sort_name: String,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Resolve a series by `name`, creating on first sight.
    GetOrCreateSeries {
        name: String,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Insert a book, returning its new id.
    InsertBook {
        book: Book,
        reply: oneshot::Sender<Result<i64>>,
    },

    /// Link an author to a book (role-tagged m2m).
    LinkBookAuthor {
        book_id: i64,
        person_id: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Link a narrator to a book (role-tagged m2m).
    LinkBookNarrator {
        book_id: i64,
        person_id: i64,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Replace a book's ordered chapter set.
    ReplaceBookChapters {
        book_id: i64,
        chapters: Vec<BookChapter>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Upsert a book's resume row.
    UpsertBookPlayback {
        playback: BookPlayback,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Persist a book's absolute resume position during playback.
    SetBookPosition {
        book_id: i64,
        position: f64,
        when: Option<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Record a book played through to the end.
    CompleteBook {
        book_id: i64,
        when: Option<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Set a book's cover path, optionally refreshing the accent (Phase 7a-iii).
    SetBookCoverPath {
        book_id: i64,
        cover_path: Option<String>,
        accent_rgb: Option<u32>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Edit a book's scalar metadata (title / year / series sequence / shelf genre
    /// / rating / starred); each `None` leaves that column unchanged (Phase 7b-iii
    /// broadened this from the 7a-iii rating/starred/shelf set).
    UpdateBook {
        book_id: i64,
        title: Option<String>,
        year: Option<i32>,
        series_sequence: Option<f64>,
        shelf_genre: Option<String>,
        rating: Option<u8>,
        starred: Option<bool>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Set (`Some`) or clear to standalone (`None`) a book's series (Phase 7b-iii).
    SetBookSeries {
        book_id: i64,
        series_id: Option<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Replace a book's credited author set (Phase 7b-iii).
    SetBookAuthors {
        book_id: i64,
        person_ids: Vec<i64>,
        reply: oneshot::Sender<Result<()>>,
    },

    /// Replace a book's credited narrator set (Phase 7b-iii).
    SetBookNarrators {
        book_id: i64,
        person_ids: Vec<i64>,
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
            Self::SetEqState { .. } => "set_eq_state",
            Self::SaveEqPreset { .. } => "save_eq_preset",
            Self::DeleteEqPreset { .. } => "delete_eq_preset",
            Self::SetAudioState { .. } => "set_audio_state",
            Self::SavePlaybackState { .. } => "save_playback_state",
            Self::IncrementPlayCount { .. } => "increment_play_count",
            Self::EnqueueTracks { .. } => "enqueue_tracks",
            Self::DeleteTrack { .. } => "delete_track",
            Self::InsertQueueTracksAt { .. } => "insert_queue_tracks_at",
            Self::InsertQueueBooksAt { .. } => "insert_queue_books_at",
            Self::DeleteBook { .. } => "delete_book",
            Self::ReplaceQueueWithTracks { .. } => "replace_queue_with_tracks",
            Self::EnqueueEpisodes { .. } => "enqueue_episodes",
            Self::ReplaceQueueWithEpisodes { .. } => "replace_queue_with_episodes",
            Self::EnqueueBooks { .. } => "enqueue_books",
            Self::ReplaceQueueWithBooks { .. } => "replace_queue_with_books",
            Self::RemoveQueueItem { .. } => "remove_queue_item",
            Self::ReorderQueue { .. } => "reorder_queue",
            Self::ReorderQueueByPositions { .. } => "reorder_queue_by_positions",
            Self::ClearQueue { .. } => "clear_queue",
            Self::CreatePlaylist { .. } => "create_playlist",
            Self::DeletePlaylist { .. } => "delete_playlist",
            Self::RenamePlaylist { .. } => "rename_playlist",
            Self::AppendPlaylistTracks { .. } => "append_playlist_tracks",
            Self::RemovePlaylistEntry { .. } => "remove_playlist_entry",
            Self::ReorderPlaylistEntry { .. } => "reorder_playlist_entry",
            Self::GetOrCreateShow { .. } => "get_or_create_show",
            Self::UpdateShow { .. } => "update_show",
            Self::DeleteShow { .. } => "delete_show",
            Self::UpsertEpisode { .. } => "upsert_episode",
            Self::SetEpisodeAudioPath { .. } => "set_episode_audio_path",
            Self::ClearEpisodeAudioPath { .. } => "clear_episode_audio_path",
            Self::UpsertPlayback { .. } => "upsert_playback",
            Self::SetEpisodePlayed { .. } => "set_episode_played",
            Self::SetEpisodeStarred { .. } => "set_episode_starred",
            Self::SetEpisodePosition { .. } => "set_episode_position",
            Self::CompleteEpisode { .. } => "complete_episode",
            Self::InsertListeningSession { .. } => "insert_listening_session",
            Self::EnqueueScrobble { .. } => "enqueue_scrobble",
            Self::DeleteScrobble { .. } => "delete_scrobble",
            Self::BumpScrobbleAttempt { .. } => "bump_scrobble_attempt",
            Self::UpsertVerifyResults { .. } => "upsert_verify_results",
            Self::UpsertShowSettings { .. } => "upsert_show_settings",
            Self::ReplaceChapters { .. } => "replace_chapters",
            Self::GetOrCreateTag { .. } => "get_or_create_tag",
            Self::SetShowTags { .. } => "set_show_tags",
            Self::GetOrCreateBookPerson { .. } => "get_or_create_book_person",
            Self::GetOrCreateSeries { .. } => "get_or_create_series",
            Self::InsertBook { .. } => "insert_book",
            Self::LinkBookAuthor { .. } => "link_book_author",
            Self::LinkBookNarrator { .. } => "link_book_narrator",
            Self::ReplaceBookChapters { .. } => "replace_book_chapters",
            Self::UpsertBookPlayback { .. } => "upsert_book_playback",
            Self::SetBookPosition { .. } => "set_book_position",
            Self::CompleteBook { .. } => "complete_book",
            Self::SetBookCoverPath { .. } => "set_book_cover_path",
            Self::UpdateBook { .. } => "update_book",
            Self::SetBookSeries { .. } => "set_book_series",
            Self::SetBookAuthors { .. } => "set_book_authors",
            Self::SetBookNarrators { .. } => "set_book_narrators",
            Self::RecordApeStrip { .. } => "record_ape_strip",
            Self::DeleteApeStrip { .. } => "delete_ape_strip",
            Self::Shutdown { .. } => "shutdown",
            #[cfg(test)]
            Self::Panic => "panic",
        }
    }
}
