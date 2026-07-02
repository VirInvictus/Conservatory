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
use crate::db::models::{
    Album, ApeStripRow, Artist, AudioState, Book, BookChapter, BookPlayback, Chapter,
    EQ_BAND_COUNT, Episode, EqState, Playback, PlaybackCursor, PlayedState, PlaylistKind,
    PlaylistOrder, Show, ShowSettings, Track, VerifyResultRow,
};
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
    #[allow(clippy::too_many_arguments)]
    pub async fn complete_operation(
        &self,
        op_id: i64,
        track_id: Option<i64>,
        album_id: Option<i64>,
        book_id: Option<i64>,
        db_old_path: Option<String>,
        db_new_path: Option<String>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::CompleteOperation {
            op_id,
            track_id,
            album_id,
            book_id,
            db_old_path,
            db_new_path,
            reply,
        })
        .await
    }

    /// Revert an operation's DB path and reset it to pending (undo).
    #[allow(clippy::too_many_arguments)]
    pub async fn revert_operation(
        &self,
        op_id: i64,
        track_id: Option<i64>,
        album_id: Option<i64>,
        book_id: Option<i64>,
        db_old_path: Option<String>,
        db_new_path: Option<String>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::RevertOperation {
            op_id,
            track_id,
            album_id,
            book_id,
            db_old_path,
            db_new_path,
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

    /// Overwrite the singleton active EQ state (Phase 5.5b).
    pub async fn set_eq_state(&self, state: EqState) -> Result<()> {
        self.dispatch(|reply| Command::SetEqState { state, reply })
            .await
    }

    /// Save (insert or overwrite by name) a named EQ preset.
    pub async fn save_eq_preset(&self, name: String, bands: [f64; EQ_BAND_COUNT]) -> Result<()> {
        self.dispatch(|reply| Command::SaveEqPreset { name, bands, reply })
            .await
    }

    /// Delete a named EQ preset.
    pub async fn delete_eq_preset(&self, name: String) -> Result<()> {
        self.dispatch(|reply| Command::DeleteEqPreset { name, reply })
            .await
    }

    /// Overwrite the singleton active audio configuration (Phase 5.5c).
    pub async fn set_audio_state(&self, state: AudioState) -> Result<()> {
        self.dispatch(|reply| Command::SetAudioState { state, reply })
            .await
    }

    /// Save the singleton playback cursor (spec §6.4, Phase 4a). The cursor's
    /// `kind` (Phase 6b-ii-c-2) records what was last playing; `track_id` is set
    /// for a track, `episode_id` for an episode.
    pub async fn save_playback_state(&self, cursor: PlaybackCursor) -> Result<()> {
        self.dispatch(|reply| Command::SavePlaybackState { cursor, reply })
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

    /// Remove a track from the library (Phase 16a). DB-only unlink; file stays.
    pub async fn delete_track(&self, track_id: i64) -> Result<()> {
        self.dispatch(|reply| Command::DeleteTrack { track_id, reply })
            .await
    }

    /// Insert tracks into the queue at `at` (the Play Next path, Phase 16a).
    pub async fn insert_queue_tracks_at(&self, at: i64, track_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::InsertQueueTracksAt {
            at,
            track_ids,
            reply,
        })
        .await
    }

    /// Insert audiobooks into the queue at `at` (16.5h: the book Play Next).
    pub async fn insert_queue_books_at(&self, at: i64, book_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::InsertQueueBooksAt {
            at,
            book_ids,
            reply,
        })
        .await
    }

    /// Remove a book from the library (16.5h). DB-only unlink; files stay.
    pub async fn delete_book(&self, book_id: i64) -> Result<()> {
        self.dispatch(|reply| Command::DeleteBook { book_id, reply })
            .await
    }

    /// Replace the whole queue with these tracks in order.
    pub async fn replace_queue_with_tracks(&self, track_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::ReplaceQueueWithTracks { track_ids, reply })
            .await
    }

    /// Append episodes to the unified queue tail (Phase 6b-ii-c).
    pub async fn enqueue_episodes(&self, episode_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::EnqueueEpisodes { episode_ids, reply })
            .await
    }

    /// Replace the whole queue with these episodes in order ("play these now").
    pub async fn replace_queue_with_episodes(&self, episode_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::ReplaceQueueWithEpisodes { episode_ids, reply })
            .await
    }

    /// Append books to the queue tail (Phase 7c-iii).
    pub async fn enqueue_books(&self, book_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::EnqueueBooks { book_ids, reply })
            .await
    }

    /// Replace the whole queue with these books in order ("play these now").
    pub async fn replace_queue_with_books(&self, book_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::ReplaceQueueWithBooks { book_ids, reply })
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

    // --- Playlists (Phase 16d) ---

    pub async fn create_playlist(
        &self,
        name: String,
        kind: PlaylistKind,
        query: Option<String>,
        limit_n: Option<i64>,
        order: Option<PlaylistOrder>,
        created_at: i64,
    ) -> Result<i64> {
        self.dispatch(|reply| Command::CreatePlaylist {
            name,
            kind,
            query,
            limit_n,
            order,
            created_at,
            reply,
        })
        .await
    }

    pub async fn delete_playlist(&self, id: i64) -> Result<()> {
        self.dispatch(|reply| Command::DeletePlaylist { id, reply })
            .await
    }

    pub async fn rename_playlist(&self, id: i64, name: String) -> Result<()> {
        self.dispatch(|reply| Command::RenamePlaylist { id, name, reply })
            .await
    }

    pub async fn append_playlist_tracks(
        &self,
        playlist_id: i64,
        track_ids: Vec<i64>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::AppendPlaylistTracks {
            playlist_id,
            track_ids,
            reply,
        })
        .await
    }

    pub async fn remove_playlist_entry(&self, playlist_id: i64, position: i64) -> Result<()> {
        self.dispatch(|reply| Command::RemovePlaylistEntry {
            playlist_id,
            position,
            reply,
        })
        .await
    }

    pub async fn reorder_playlist_entry(&self, playlist_id: i64, from: i64, to: i64) -> Result<()> {
        self.dispatch(|reply| Command::ReorderPlaylistEntry {
            playlist_id,
            from,
            to,
            reply,
        })
        .await
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

    /// Record an episode's downloaded `audio_path` (Phase 6a-iii-b download).
    pub async fn set_episode_audio_path(
        &self,
        episode_id: i64,
        audio_path: impl Into<String>,
    ) -> Result<()> {
        let audio_path = audio_path.into();
        self.dispatch(|reply| Command::SetEpisodeAudioPath {
            episode_id,
            audio_path,
            reply,
        })
        .await
    }

    /// Clear an episode's downloaded `audio_path` (retention prune, Phase
    /// 6b-ii-c-3-b): the file was deleted from disk, so the row reverts to
    /// stream-only.
    pub async fn clear_episode_audio_path(&self, episode_id: i64) -> Result<()> {
        self.dispatch(|reply| Command::ClearEpisodeAudioPath { episode_id, reply })
            .await
    }

    /// Upsert an episode's triage/playback row.
    pub async fn upsert_playback(&self, playback: Playback) -> Result<()> {
        self.dispatch(|reply| Command::UpsertPlayback { playback, reply })
            .await
    }

    /// Set an episode's played state (triage, Phase 6b-ii-b); preserves starred.
    pub async fn set_episode_played(
        &self,
        episode_id: i64,
        state: PlayedState,
        when: Option<i64>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::SetEpisodePlayed {
            episode_id,
            state,
            when,
            reply,
        })
        .await
    }

    /// Toggle an episode's starred flag (triage, Phase 6b-ii-b).
    pub async fn set_episode_starred(&self, episode_id: i64, starred: bool) -> Result<()> {
        self.dispatch(|reply| Command::SetEpisodeStarred {
            episode_id,
            starred,
            reply,
        })
        .await
    }

    /// Persist an episode's resume position during playback (Phase 6b-ii-c-2):
    /// the engine's tick / pause / seek write. Marks `InProgress`, preserves
    /// starred / play_count.
    pub async fn set_episode_position(
        &self,
        episode_id: i64,
        position: f64,
        when: Option<i64>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::SetEpisodePosition {
            episode_id,
            position,
            when,
            reply,
        })
        .await
    }

    /// Record an episode played through to the end (Phase 6b-ii-c-2): marks
    /// `PlayedFully`, bumps play_count, rewinds position. The podcast analogue of
    /// `increment_play_count`.
    pub async fn complete_episode(&self, episode_id: i64, when: Option<i64>) -> Result<()> {
        self.dispatch(|reply| Command::CompleteEpisode {
            episode_id,
            when,
            reply,
        })
        .await
    }

    /// Append one listening session (Phase 6c-ii): the engine's per-episode
    /// time-saved record. Append-only.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_listening_session(
        &self,
        episode_id: Option<i64>,
        book_id: Option<i64>,
        started_at: i64,
        ended_at: i64,
        real_seconds: f64,
        audio_seconds: f64,
        smart_speed_saved: f64,
    ) -> Result<()> {
        self.dispatch(|reply| Command::InsertListeningSession {
            episode_id,
            book_id,
            started_at,
            ended_at,
            real_seconds,
            audio_seconds,
            smart_speed_saved,
            reply,
        })
        .await
    }

    /// Upsert a batch of integrity-verification results (Phase 8a), one tx.
    pub async fn upsert_verify_results(&self, rows: Vec<VerifyResultRow>) -> Result<()> {
        self.dispatch(|reply| Command::UpsertVerifyResults { rows, reply })
            .await
    }

    /// Record an APE-strip undo row before a file is stripped (Phase 8c-iii).
    pub async fn record_ape_strip(&self, row: ApeStripRow) -> Result<()> {
        self.dispatch(|reply| Command::RecordApeStrip { row, reply })
            .await
    }

    /// Delete an APE-strip undo row after a strip is undone (Phase 8c-iii).
    pub async fn delete_ape_strip(&self, file_path: String) -> Result<()> {
        self.dispatch(|reply| Command::DeleteApeStrip { file_path, reply })
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

    // --- Audiobooks (spec §4.5, Phase 7a-i) ---

    /// Resolve an audiobook person by `sort_name`, creating on first sight.
    pub async fn get_or_create_book_person(
        &self,
        name: impl Into<String>,
        sort_name: impl Into<String>,
    ) -> Result<i64> {
        let name = name.into();
        let sort_name = sort_name.into();
        self.dispatch(|reply| Command::GetOrCreateBookPerson {
            name,
            sort_name,
            reply,
        })
        .await
    }

    /// Resolve a series by `name`, creating on first sight.
    pub async fn get_or_create_series(&self, name: impl Into<String>) -> Result<i64> {
        let name = name.into();
        self.dispatch(|reply| Command::GetOrCreateSeries { name, reply })
            .await
    }

    /// Insert a book, returning its new id.
    pub async fn insert_book(&self, book: Book) -> Result<i64> {
        self.dispatch(|reply| Command::InsertBook { book, reply })
            .await
    }

    /// Link an author to a book (role-tagged many-to-many).
    pub async fn link_book_author(&self, book_id: i64, person_id: i64) -> Result<()> {
        self.dispatch(|reply| Command::LinkBookAuthor {
            book_id,
            person_id,
            reply,
        })
        .await
    }

    /// Link a narrator to a book (role-tagged many-to-many).
    pub async fn link_book_narrator(&self, book_id: i64, person_id: i64) -> Result<()> {
        self.dispatch(|reply| Command::LinkBookNarrator {
            book_id,
            person_id,
            reply,
        })
        .await
    }

    /// Replace a book's ordered chapter set.
    pub async fn replace_book_chapters(
        &self,
        book_id: i64,
        chapters: Vec<BookChapter>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::ReplaceBookChapters {
            book_id,
            chapters,
            reply,
        })
        .await
    }

    /// Upsert a book's resume row.
    pub async fn upsert_book_playback(&self, playback: BookPlayback) -> Result<()> {
        self.dispatch(|reply| Command::UpsertBookPlayback { playback, reply })
            .await
    }

    /// Persist a book's absolute resume position during playback.
    pub async fn set_book_position(
        &self,
        book_id: i64,
        position: f64,
        when: Option<i64>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::SetBookPosition {
            book_id,
            position,
            when,
            reply,
        })
        .await
    }

    /// Record a book played through to the end.
    pub async fn complete_book(&self, book_id: i64, when: Option<i64>) -> Result<()> {
        self.dispatch(|reply| Command::CompleteBook {
            book_id,
            when,
            reply,
        })
        .await
    }

    /// Set a book's cover path, optionally refreshing the accent (Phase 7a-iii).
    pub async fn set_book_cover_path(
        &self,
        book_id: i64,
        cover_path: Option<String>,
        accent_rgb: Option<u32>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::SetBookCoverPath {
            book_id,
            cover_path,
            accent_rgb,
            reply,
        })
        .await
    }

    /// Edit a book's scalar metadata (title / year / series sequence / shelf genre
    /// / rating / starred); each `None` leaves that column unchanged.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_book(
        &self,
        book_id: i64,
        title: Option<String>,
        year: Option<i32>,
        series_sequence: Option<f64>,
        shelf_genre: Option<String>,
        rating: Option<u8>,
        starred: Option<bool>,
    ) -> Result<()> {
        self.dispatch(|reply| Command::UpdateBook {
            book_id,
            title,
            year,
            series_sequence,
            shelf_genre,
            rating,
            starred,
            reply,
        })
        .await
    }

    /// Set (`Some`) or clear to standalone (`None`) a book's series (Phase 7b-iii).
    pub async fn set_book_series(&self, book_id: i64, series_id: Option<i64>) -> Result<()> {
        self.dispatch(|reply| Command::SetBookSeries {
            book_id,
            series_id,
            reply,
        })
        .await
    }

    /// Replace a book's credited author set (Phase 7b-iii).
    pub async fn set_book_authors(&self, book_id: i64, person_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::SetBookAuthors {
            book_id,
            person_ids,
            reply,
        })
        .await
    }

    /// Replace a book's credited narrator set (Phase 7b-iii).
    pub async fn set_book_narrators(&self, book_id: i64, person_ids: Vec<i64>) -> Result<()> {
        self.dispatch(|reply| Command::SetBookNarrators {
            book_id,
            person_ids,
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
            book_id,
            db_old_path,
            db_new_path,
            reply,
        } => {
            let _ = reply.send(journal::complete_operation(
                conn,
                op_id,
                track_id,
                album_id,
                book_id,
                db_old_path.as_deref(),
                db_new_path.as_deref(),
            ));
        }
        Command::RevertOperation {
            op_id,
            track_id,
            album_id,
            book_id,
            db_old_path,
            db_new_path,
            reply,
        } => {
            let _ = reply.send(journal::revert_operation(
                conn,
                op_id,
                track_id,
                album_id,
                book_id,
                db_old_path.as_deref(),
                db_new_path.as_deref(),
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
        Command::SetEqState { state, reply } => {
            let _ = reply.send(writes::set_eq_state(conn, &state));
        }
        Command::SaveEqPreset { name, bands, reply } => {
            let _ = reply.send(writes::save_eq_preset(conn, &name, &bands));
        }
        Command::DeleteEqPreset { name, reply } => {
            let _ = reply.send(writes::delete_eq_preset(conn, &name));
        }
        Command::SetAudioState { state, reply } => {
            let _ = reply.send(writes::set_audio_state(conn, &state));
        }
        Command::SavePlaybackState { cursor, reply } => {
            let _ = reply.send(writes::save_playback_state(conn, &cursor));
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
        Command::DeleteTrack { track_id, reply } => {
            let _ = reply.send(writes::delete_track(conn, track_id));
        }
        Command::InsertQueueTracksAt {
            at,
            track_ids,
            reply,
        } => {
            let _ = reply.send(writes::insert_queue_tracks_at(conn, at, &track_ids));
        }
        Command::InsertQueueBooksAt {
            at,
            book_ids,
            reply,
        } => {
            let _ = reply.send(writes::insert_queue_books_at(conn, at, &book_ids));
        }
        Command::DeleteBook { book_id, reply } => {
            let _ = reply.send(writes::delete_book(conn, book_id));
        }
        Command::ReplaceQueueWithTracks { track_ids, reply } => {
            let _ = reply.send(writes::replace_queue_with_tracks(conn, &track_ids));
        }
        Command::EnqueueEpisodes { episode_ids, reply } => {
            let _ = reply.send(writes::enqueue_episodes(conn, &episode_ids));
        }
        Command::ReplaceQueueWithEpisodes { episode_ids, reply } => {
            let _ = reply.send(writes::replace_queue_with_episodes(conn, &episode_ids));
        }
        Command::EnqueueBooks { book_ids, reply } => {
            let _ = reply.send(writes::enqueue_books(conn, &book_ids));
        }
        Command::ReplaceQueueWithBooks { book_ids, reply } => {
            let _ = reply.send(writes::replace_queue_with_books(conn, &book_ids));
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
        Command::CreatePlaylist {
            name,
            kind,
            query,
            limit_n,
            order,
            created_at,
            reply,
        } => {
            let _ = reply.send(writes::create_playlist(
                conn,
                &name,
                kind,
                query.as_deref(),
                limit_n,
                order,
                created_at,
            ));
        }
        Command::DeletePlaylist { id, reply } => {
            let _ = reply.send(writes::delete_playlist(conn, id));
        }
        Command::RenamePlaylist { id, name, reply } => {
            let _ = reply.send(writes::rename_playlist(conn, id, &name));
        }
        Command::AppendPlaylistTracks {
            playlist_id,
            track_ids,
            reply,
        } => {
            let _ = reply.send(writes::append_playlist_tracks(
                conn,
                playlist_id,
                &track_ids,
            ));
        }
        Command::RemovePlaylistEntry {
            playlist_id,
            position,
            reply,
        } => {
            let _ = reply.send(writes::remove_playlist_entry(conn, playlist_id, position));
        }
        Command::ReorderPlaylistEntry {
            playlist_id,
            from,
            to,
            reply,
        } => {
            let _ = reply.send(writes::reorder_playlist_entry(conn, playlist_id, from, to));
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
        Command::SetEpisodeAudioPath {
            episode_id,
            audio_path,
            reply,
        } => {
            let _ = reply.send(writes::set_episode_audio_path(
                conn,
                episode_id,
                &audio_path,
            ));
        }
        Command::ClearEpisodeAudioPath { episode_id, reply } => {
            let _ = reply.send(writes::clear_episode_audio_path(conn, episode_id));
        }
        Command::UpsertPlayback { playback, reply } => {
            let _ = reply.send(writes::upsert_playback(conn, &playback));
        }
        Command::SetEpisodePlayed {
            episode_id,
            state,
            when,
            reply,
        } => {
            let _ = reply.send(writes::set_episode_played(conn, episode_id, state, when));
        }
        Command::SetEpisodeStarred {
            episode_id,
            starred,
            reply,
        } => {
            let _ = reply.send(writes::set_episode_starred(conn, episode_id, starred));
        }
        Command::SetEpisodePosition {
            episode_id,
            position,
            when,
            reply,
        } => {
            let _ = reply.send(writes::set_episode_position(
                conn, episode_id, position, when,
            ));
        }
        Command::CompleteEpisode {
            episode_id,
            when,
            reply,
        } => {
            let _ = reply.send(writes::complete_episode(conn, episode_id, when));
        }
        Command::InsertListeningSession {
            episode_id,
            book_id,
            started_at,
            ended_at,
            real_seconds,
            audio_seconds,
            smart_speed_saved,
            reply,
        } => {
            let _ = reply.send(writes::insert_listening_session(
                conn,
                episode_id,
                book_id,
                started_at,
                ended_at,
                real_seconds,
                audio_seconds,
                smart_speed_saved,
            ));
        }
        Command::UpsertVerifyResults { rows, reply } => {
            let _ = reply.send(writes::upsert_verify_results(conn, &rows));
        }
        Command::RecordApeStrip { row, reply } => {
            let _ = reply.send(writes::record_ape_strip(conn, &row));
        }
        Command::DeleteApeStrip { file_path, reply } => {
            let _ = reply.send(writes::delete_ape_strip(conn, &file_path));
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
        Command::GetOrCreateBookPerson {
            name,
            sort_name,
            reply,
        } => {
            let _ = reply.send(writes::get_or_create_book_person(conn, &name, &sort_name));
        }
        Command::GetOrCreateSeries { name, reply } => {
            let _ = reply.send(writes::get_or_create_series(conn, &name));
        }
        Command::InsertBook { book, reply } => {
            let _ = reply.send(writes::insert_book(conn, &book));
        }
        Command::LinkBookAuthor {
            book_id,
            person_id,
            reply,
        } => {
            let _ = reply.send(writes::link_book_author(conn, book_id, person_id));
        }
        Command::LinkBookNarrator {
            book_id,
            person_id,
            reply,
        } => {
            let _ = reply.send(writes::link_book_narrator(conn, book_id, person_id));
        }
        Command::ReplaceBookChapters {
            book_id,
            chapters,
            reply,
        } => {
            let _ = reply.send(writes::replace_book_chapters(conn, book_id, &chapters));
        }
        Command::UpsertBookPlayback { playback, reply } => {
            let _ = reply.send(writes::upsert_book_playback(conn, &playback));
        }
        Command::SetBookPosition {
            book_id,
            position,
            when,
            reply,
        } => {
            let _ = reply.send(writes::set_book_position(conn, book_id, position, when));
        }
        Command::CompleteBook {
            book_id,
            when,
            reply,
        } => {
            let _ = reply.send(writes::complete_book(conn, book_id, when));
        }
        Command::SetBookCoverPath {
            book_id,
            cover_path,
            accent_rgb,
            reply,
        } => {
            let _ = reply.send(writes::set_book_cover_path(
                conn,
                book_id,
                cover_path.as_deref(),
                accent_rgb,
            ));
        }
        Command::UpdateBook {
            book_id,
            title,
            year,
            series_sequence,
            shelf_genre,
            rating,
            starred,
            reply,
        } => {
            let _ = reply.send(writes::update_book(
                conn,
                book_id,
                title.as_deref(),
                year,
                series_sequence,
                shelf_genre.as_deref(),
                rating,
                starred,
            ));
        }
        Command::SetBookSeries {
            book_id,
            series_id,
            reply,
        } => {
            let _ = reply.send(writes::set_book_series(conn, book_id, series_id));
        }
        Command::SetBookAuthors {
            book_id,
            person_ids,
            reply,
        } => {
            let _ = reply.send(writes::set_book_authors(conn, book_id, &person_ids));
        }
        Command::SetBookNarrators {
            book_id,
            person_ids,
            reply,
        } => {
            let _ = reply.send(writes::set_book_narrators(conn, book_id, &person_ids));
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
