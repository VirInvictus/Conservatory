//! Domain types for the music data model (spec §4.1, docs/schema.md).
//!
//! Rust idiom over the SQL: `Option` for nullable columns, `bool` for INTEGER
//! 0/1, `chrono::DateTime<Utc>` for unix-epoch INTEGER timestamps, packed RGB as
//! `u32`. `id == 0` on a value not yet inserted; the writer returns the real id.
//! The shape mirrors `belfry-core`'s `domain.rs`.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::errors::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub sort_name: String, // "Beatles, The"; drives path + sort (Calibre author_sort)
    pub musicbrainz_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: i64,
    pub title: String,
    pub album_artist_id: Option<i64>, // None => Various Artists bucket
    pub shelf_genre: Option<String>,  // the only genre input to the path (spec §5.2)
    pub year: Option<i32>,
    pub release_date: Option<String>,
    pub musicbrainz_release_id: Option<String>,
    pub cover_path: Option<String>,
    pub accent_rgb: Option<u32>, // packed RGB, median-cut from cover (spec §7.4)
    pub folder_path: String,     // managed; rendered from the template (spec §5.1)
    pub added_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>, // track artist (may differ from album artist)
    pub title: String,
    pub track_no: Option<i32>,
    pub disc_no: Option<i32>,
    pub duration: Option<f64>, // seconds
    pub file_path: String,     // managed; under the album folder
    pub format: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub replaygain_track: Option<f64>,
    pub replaygain_album: Option<f64>,
    pub rating: u8, // 0–5
    pub play_count: u32,
    pub last_played: Option<DateTime<Utc>>,
    pub starred: bool,
    pub musicbrainz_recording_id: Option<String>,
    pub added_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genre {
    pub id: i64,
    pub name: String,
}

/// A named saved search (spec §3.4). `expression` is the raw filter text, stored
/// verbatim and re-parsed on load so it inherits later grammar additions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Perspective {
    pub id: i64,
    pub name: String,
    pub expression: String,
    pub scope: String, // "tracks" today; albums/episodes/books reuse the table later
}

/// The media kind of a unified-queue entry (spec §4.3). Only `Track` is real in
/// Phase 4b; `Episode` and `Audiobook` rows arrive with Phases 6 and 7, but the
/// kind exists from the start because the queue is one core-owned table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Track,
    Episode,
    Audiobook,
}

impl MediaKind {
    /// The `kind` TEXT value stored in the `queue` table.
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Track => "track",
            MediaKind::Episode => "episode",
            MediaKind::Audiobook => "audiobook",
        }
    }
}

impl fmt::Display for MediaKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MediaKind {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "track" => Ok(MediaKind::Track),
            "episode" => Ok(MediaKind::Episode),
            "audiobook" => Ok(MediaKind::Audiobook),
            other => Err(Error::InvalidEnum {
                field: "queue.kind",
                value: other.to_string(),
            }),
        }
    }
}

/// A playlist's kind (Phase 16d). `Static` is a frozen, hand-ordered list whose
/// members live in `playlist_entries`; `Smart` is a live query (with a limit and
/// order) that materialises on demand and holds no entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaylistKind {
    Static,
    Smart,
}

impl PlaylistKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PlaylistKind::Static => "static",
            PlaylistKind::Smart => "smart",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "static" => Some(PlaylistKind::Static),
            "smart" => Some(PlaylistKind::Smart),
            _ => None,
        }
    }
}

/// A smart playlist's prioritisation order (Phase 16d, the research trio plus
/// title/artist). `random` is deferred to the Phase 17 shuffle work; these are
/// the deterministic keys. `LastPlayed` sorts least-recently-played first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaylistOrder {
    Added,
    Rating,
    LastPlayed,
    Title,
    Artist,
}

impl PlaylistOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            PlaylistOrder::Added => "added",
            PlaylistOrder::Rating => "rating",
            PlaylistOrder::LastPlayed => "lastplayed",
            PlaylistOrder::Title => "title",
            PlaylistOrder::Artist => "artist",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "added" => Some(PlaylistOrder::Added),
            "rating" => Some(PlaylistOrder::Rating),
            "lastplayed" | "last_played" => Some(PlaylistOrder::LastPlayed),
            "title" => Some(PlaylistOrder::Title),
            "artist" => Some(PlaylistOrder::Artist),
            _ => None,
        }
    }
}

/// A playlist row (Phase 16d). `query` / `limit_n` / `order_by` are `Some` only
/// for a `Smart` playlist; a `Static` playlist carries `None` for all three and
/// keeps its members in `playlist_entries`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub kind: PlaylistKind,
    pub query: Option<String>,
    pub limit_n: Option<i64>,
    pub order_by: Option<PlaylistOrder>,
    pub created_at: i64,
}

/// One ordered entry in the unified queue (spec §4.3, §6.1). Exactly one of the
/// id columns is populated, matched to `kind`; `position` is contiguous and
/// drag-reorderable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: i64,
    pub position: i64,
    pub kind: MediaKind,
    pub track_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub book_id: Option<i64>,
}

/// The singleton transport cursor as written (spec §6.4, Phase 6b-ii-c-2): what
/// was last playing and where, so a restart resumes. `kind` records the media
/// kind; `track_id` is set for a track, `episode_id` for an episode, `book_id`
/// for an audiobook (the read side is [`crate::db::PlaybackStateRow`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackCursor {
    pub kind: MediaKind,
    pub track_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub book_id: Option<i64>,
    pub position: f64,
    pub paused: bool,
    pub volume: i64,
    pub updated_at: i64,
}

// --- Podcast domain (spec §4.2). Ported in shape from `belfry-core`'s
// `domain.rs`. These are core-owned types (the schema lives in core's migration
// ledger, the §2.2 boundary rule); the `conservatory-podcasts` plugin consumes
// them. A music-only build compiles them but leaves the tables empty.

/// A podcast subscription (spec §4.2). The conditional-GET bookkeeping
/// (`last_fetched` / `last_modified` / `etag`) is written by the fetch loop
/// (Phase 6a-ii); `auth_pass_ref` is a libsecret reference, never an inline
/// secret (oo7, Phase 6a-iii).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Show {
    pub id: i64,
    pub slug: String,
    pub feed_url: String,
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub homepage_url: Option<String>,
    pub cover_path: Option<String>,
    pub accent_rgb: Option<u32>,
    pub apple_podcasts_id: Option<String>,
    pub last_fetched: Option<DateTime<Utc>>,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
    pub fetch_interval: u32,
    pub auth_user: Option<String>,
    pub auth_pass_ref: Option<String>,
    pub auto_download: bool,
    pub keep_count: u32,
    pub priority: i32,
    pub folder_path: String,
}

/// One feed episode. Identity is `(show_id, guid)` (spec §8). `episode_type` is
/// kept as the feed's raw string (commonly full / trailer / bonus) rather than a
/// closed enum, so an unexpected value never fails a read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub id: i64,
    pub show_id: i64,
    pub guid: String,
    pub title: String,
    pub description: Option<String>,
    pub pub_date: Option<DateTime<Utc>>,
    pub duration: Option<u32>, // seconds
    pub file_size: Option<u64>,
    pub audio_url: Option<String>,
    pub audio_path: Option<String>, // None until downloaded (spec §5.3)
    pub folder_path: String,
    pub mime_type: Option<String>,
    pub season: Option<u32>,
    pub episode_number: Option<u32>,
    pub episode_type: Option<String>,
}

/// Per-episode triage + playback state (spec §4.2). Queue membership is *not*
/// here: it lives in the unified `queue` table (the §4.2 change from Belfry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayedState {
    Unplayed = 0,
    InProgress = 1,
    PlayedFully = 2,
    ArchivedUnlistened = 3,
}

impl PlayedState {
    /// The INTEGER value stored in `playback.played`.
    pub fn as_i64(self) -> i64 {
        self as i64
    }

    /// Map a stored INTEGER back to a state.
    pub fn from_i64(value: i64) -> Result<Self, Error> {
        match value {
            0 => Ok(PlayedState::Unplayed),
            1 => Ok(PlayedState::InProgress),
            2 => Ok(PlayedState::PlayedFully),
            3 => Ok(PlayedState::ArchivedUnlistened),
            other => Err(Error::InvalidEnum {
                field: "playback.played",
                value: other.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Playback {
    pub episode_id: i64,
    pub position: f64,
    pub played: PlayedState,
    pub last_played: Option<DateTime<Utc>>,
    pub play_count: u32,
    pub starred: bool,
}

/// What happens to a new episode of this show (Castro inbox policy, spec §3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxPolicy {
    Inbox,
    AlwaysQueue,
    AlwaysArchive,
}

impl InboxPolicy {
    /// The TEXT value stored in `show_settings.inbox_policy`.
    pub fn as_str(self) -> &'static str {
        match self {
            InboxPolicy::Inbox => "inbox",
            InboxPolicy::AlwaysQueue => "always_queue",
            InboxPolicy::AlwaysArchive => "always_archive",
        }
    }
}

impl fmt::Display for InboxPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for InboxPolicy {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "inbox" => Ok(InboxPolicy::Inbox),
            "always_queue" => Ok(InboxPolicy::AlwaysQueue),
            "always_archive" => Ok(InboxPolicy::AlwaysArchive),
            other => Err(Error::InvalidEnum {
                field: "show_settings.inbox_policy",
                value: other.to_string(),
            }),
        }
    }
}

/// Per-show overrides (Overcast pattern, spec §3.7). `skip_forward` / `skip_back`
/// of `None` inherit the global skip amounts.
#[derive(Debug, Clone, PartialEq)]
pub struct ShowSettings {
    pub show_id: i64,
    pub playback_speed: f64,
    pub smart_speed: bool,
    pub voice_boost: bool,
    pub skip_intro: u32,
    pub skip_outro: u32,
    pub skip_forward: Option<u32>,
    pub skip_back: Option<u32>,
    pub inbox_policy: InboxPolicy,
}

/// One playback session (spec §6.3). Append-only; drives Smart Speed time-saved
/// accounting and the history view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListeningSession {
    pub id: i64,
    pub episode_id: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub real_seconds: f64,
    pub audio_seconds: f64,
    pub smart_speed_saved: f64,
}

/// One row of the `verify_results` cache (Phase 8a): a file's integrity verdict
/// plus the size/mtime it had when checked, so a re-verify can skip an unchanged
/// file. Path-keyed (library-relative), media-agnostic. `checked_at`, `file_size`,
/// and `file_mtime` are plain unix-second / byte counts (not `DateTime`, to keep
/// the staleness comparison a cheap integer equality against `fs::metadata`).
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyResultRow {
    pub file_path: String,
    pub file_size: i64,
    pub file_mtime: i64,
    pub verdict: crate::verify::VerifyVerdict,
    pub detail: Option<String>,
    pub checked_at: i64,
}

/// One queued "listen" awaiting submission (Phase 9a, `scrobble_outbox`). The
/// metadata is snapshotted at play-completion time so a later rename cannot
/// corrupt history and the submitter needs no join. [`NewScrobble`] is the
/// enqueue payload; [`PendingScrobble`] is the read-back the drain loop submits,
/// carrying the row id and retry bookkeeping.
#[derive(Debug, Clone, PartialEq)]
pub struct NewScrobble {
    pub service: String,
    /// 'track' | 'episode' (scope + accounting; the services don't distinguish).
    pub kind: String,
    pub listened_at: i64,
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
    pub track_number: Option<i64>,
    pub duration_secs: Option<i64>,
    pub recording_mbid: Option<String>,
}

/// A `scrobble_outbox` row read back for submission (Phase 9a).
#[derive(Debug, Clone, PartialEq)]
pub struct PendingScrobble {
    pub id: i64,
    pub service: String,
    pub kind: String,
    pub listened_at: i64,
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
    pub track_number: Option<i64>,
    pub duration_secs: Option<i64>,
    pub recording_mbid: Option<String>,
    pub attempts: i64,
}

/// The undo record for one `apestrip` (Phase 8c-iii): the excised APEv2 tag
/// bytes and where they were removed, plus the pre-strip size/mtime so undo can
/// refuse a file that changed after the strip. Path-keyed (library-relative).
#[derive(Debug, Clone, PartialEq)]
pub struct ApeStripRow {
    pub file_path: String,
    pub ape_bytes: Vec<u8>,
    pub tag_start: i64,
    pub orig_size: i64,
    pub orig_mtime: i64,
    pub stripped_at: i64,
}

/// An episode chapter (spec §8). Source is `podcast:chapters` JSON or ID3 CHAP;
/// `start_time` / `end_time` are seconds into the episode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub id: i64,
    pub episode_id: i64,
    pub start_time: f64,
    pub end_time: Option<f64>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub image_path: Option<String>,
}

/// A show tag (Calibre loanword; secondary organization, preserved on OPML
/// round-trip, spec §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
}

// --- Audiobooks (spec §4.5, Phase 7) ------------------------------------------
//
// Authors and narrators are distinct roles over a shared `book_people` table;
// `series` carries a decimal sequence; a book is the unit and its ordered
// chapters address either a standalone per-chapter file or a span inside one
// M4B. Resume is a single row per book (the §6.4 first-class case). Schema is
// core-owned (the §2.2 boundary rule); the `conservatory-audiobooks` plugin
// consumes these. A music-only build compiles them but leaves the tables empty.

/// An audiobook author or narrator. The role is carried by which link table
/// (`book_authors` / `book_narrators`) references the person, not by the row.
/// `sort_name` ("Sanderson, Brandon") drives the path and sort (the Calibre
/// author_sort trick, as for [`Artist`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookPerson {
    pub id: i64,
    pub name: String,
    pub sort_name: String,
}

/// A book series. `series_sequence` lives on [`Book`] so one series can hold
/// books at different decimal positions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Series {
    pub id: i64,
    pub name: String,
}

/// One audiobook (spec §4.5). Authors and narrators are linked separately;
/// `series_id` / `series_sequence` are `None` for a standalone book. Format /
/// bitrate / sample-rate are read per chapter file at import and total duration
/// is derived by summing chapter durations, so neither is stored here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Book {
    pub id: i64,
    pub title: String,
    pub subtitle: Option<String>,
    pub series_id: Option<i64>,
    pub series_sequence: Option<f64>, // decimal: "Book 1.5"
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub isbn: Option<String>,
    pub asin: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub shelf_genre: Option<String>, // single-valued path input (spec §5.2)
    pub cover_path: Option<String>,
    pub accent_rgb: Option<u32>, // packed RGB, median-cut from cover (spec §7.4)
    pub folder_path: String,     // managed; rendered from the template (spec §5.7)
    pub rating: u8,              // 0–5
    pub starred: bool,
    pub added_at: Option<DateTime<Utc>>,
}

/// One ordered chapter (spec §4.5). `file_path` + `file_offset` address either a
/// standalone per-chapter file (offset 0) or a span inside a single M4B; the
/// engine treats both identically (spec §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookChapter {
    pub id: i64,
    pub book_id: i64,
    pub idx: i64, // 0-based order within the book
    pub title: Option<String>,
    pub file_path: String,
    pub file_offset: f64, // seconds into file_path where this chapter starts
    pub duration: Option<f64>,
}

/// First-class resume for a book (spec §6.4). One row per book; `position` is an
/// absolute offset across the whole book. The per-book `speed` / `smart_speed` /
/// `voice_boost` overrides are `None` to inherit the global default (spec §6.3).
#[derive(Debug, Clone, PartialEq)]
pub struct BookPlayback {
    pub book_id: i64,
    pub position: f64,
    pub finished: bool,
    pub last_played: Option<DateTime<Utc>>,
    pub speed: Option<f64>,
    pub smart_speed: Option<bool>,
    pub voice_boost: Option<bool>,
}

/// Number of graphic-equalizer bands (Phase 5.5b): a 10-band ISO octave EQ.
pub const EQ_BAND_COUNT: usize = 10;

/// The graphic-EQ band centre frequencies in Hz (ISO octave centres). Indexes
/// align with [`EqState::bands`].
pub const EQ_CENTRES: [u32; EQ_BAND_COUNT] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

/// The active equalizer state (spec §6.2): per-band gains in dB plus the selected
/// preset name (`None` once a band is edited away from a preset). Rendered as the
/// `@eq` stage of the `af` chain; an all-zero state ([`Self::is_flat`]) is a no-op
/// (no `@eq` stage at all).
#[derive(Debug, Clone, PartialEq)]
pub struct EqState {
    pub bands: [f64; EQ_BAND_COUNT],
    pub preset: Option<String>,
}

impl EqState {
    /// The flat state: every band 0 dB, the `Flat` preset selected.
    pub fn flat() -> Self {
        Self {
            bands: [0.0; EQ_BAND_COUNT],
            preset: Some("Flat".to_string()),
        }
    }

    /// Whether every band is effectively 0 dB (renders to a no-op chain).
    pub fn is_flat(&self) -> bool {
        self.bands.iter().all(|g| g.abs() < 0.05)
    }

    /// Parse a CSV of band gains (the storage form). Forgiving: a malformed or
    /// short value reads as 0 dB for that band, so a bad stored row never breaks
    /// playback.
    pub fn parse_bands(csv: &str) -> [f64; EQ_BAND_COUNT] {
        let mut bands = [0.0; EQ_BAND_COUNT];
        for (slot, part) in bands.iter_mut().zip(csv.split(',')) {
            *slot = part.trim().parse().unwrap_or(0.0);
        }
        bands
    }

    /// Format band gains as the CSV storage form.
    pub fn format_bands(bands: &[f64; EQ_BAND_COUNT]) -> String {
        bands
            .iter()
            .map(|g| g.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// A named EQ preset (Phase 5.5b): the `eq_presets` row.
#[derive(Debug, Clone, PartialEq)]
pub struct EqPreset {
    pub name: String,
    pub bands: [f64; EQ_BAND_COUNT],
}

/// One DSP module's state (Phase 5.5c): an `enabled` flag plus its settings. The
/// settings persist even while the module is off, so toggling a tuned module
/// back on restores its parameters rather than resetting them; only `enabled`
/// gates whether the module contributes an `af`-chain stage. `Copy` so the whole
/// [`DspState`] threads through the engine like [`crate::player::MusicProfile`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ModuleState<T> {
    pub enabled: bool,
    pub settings: T,
}

/// Compressor (`acompressor`) settings (Phase 5.5c). `threshold_db` is in dBFS;
/// the stage builder converts it to the filter's linear `threshold`. `ratio` is
/// the N:1 compression ratio; attack/release in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompSettings {
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
}

impl Default for CompSettings {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 3.0,
            attack_ms: 20.0,
            release_ms: 250.0,
        }
    }
}

/// Brick-wall limiter (`alimiter`) settings (Phase 5.5c). `ceiling_db` is the
/// output ceiling in dBFS (converted to the filter's linear `limit`). This is
/// also the ReplayGain clip safety net (chain.rs `@limit`): with the limiter on,
/// a positive net gain can never push a sample over full scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimiterSettings {
    pub ceiling_db: f64,
}

impl Default for LimiterSettings {
    fn default() -> Self {
        Self { ceiling_db: -1.0 }
    }
}

/// Volume leveler (`dynaudnorm`, single-pass/live) settings (Phase 5.5c).
/// `target_peak` is the dynaudnorm `p` target (0..1); `gausssize` is `g` (odd,
/// 3..301; larger windows smooth the gain curve and tame pumping). `dynaudnorm`
/// is chosen over `loudnorm`, whose accurate mode is two-pass/offline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelerSettings {
    pub target_peak: f64,
    pub gausssize: u32,
}

impl Default for LevelerSettings {
    fn default() -> Self {
        Self {
            target_peak: 0.95,
            gausssize: 31,
        }
    }
}

/// The dynamics-processing modules (Phase 5.5c): optional ordered `af`-chain
/// stages after the EQ — compressor, brick-wall limiter, volume leveler — each
/// independently toggleable. `DspState::default()` is everything off (the no-op
/// chain). Rendered by [`crate::player::dsp`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DspState {
    pub comp: ModuleState<CompSettings>,
    pub limiter: ModuleState<LimiterSettings>,
    pub leveler: ModuleState<LevelerSettings>,
}

impl DspState {
    /// Everything off: no DSP stages (the no-op chain).
    pub fn off() -> Self {
        Self::default()
    }

    /// Whether no module is enabled (renders to no DSP stages).
    pub fn is_off(&self) -> bool {
        !self.comp.enabled && !self.limiter.enabled && !self.leveler.enabled
    }
}

/// The high-quality-resampler knob (Phase 5.5c, spec §6.5). `Default` leaves
/// mpv's resampler at its defaults; `High` raises the `audio-resample-*` quality
/// for the unavoidable-resample case. Avoid-resample stays the default either way
/// (`audio-samplerate` / `audio-format` are left unset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResamplerQuality {
    #[default]
    Default,
    High,
}

impl ResamplerQuality {
    /// The TEXT value stored in `audio_state.resampler_quality`.
    pub fn as_str(self) -> &'static str {
        match self {
            ResamplerQuality::Default => "default",
            ResamplerQuality::High => "high",
        }
    }
}

impl fmt::Display for ResamplerQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ResamplerQuality {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "default" => Ok(ResamplerQuality::Default),
            "high" => Ok(ResamplerQuality::High),
            other => Err(Error::InvalidEnum {
                field: "audio_state.resampler_quality",
                value: other.to_string(),
            }),
        }
    }
}

/// The singleton active audio configuration (Phase 5.5c): the playback defaults
/// (ReplayGain mode / preamp / clip, gapless), the DSP modules, and the output
/// backend / resampler. Mirrors `eq_state` — one row, `get_audio_state` reads it
/// and `set_audio_state` overwrites it. The DSP + output halves are consumed at
/// 5.5c-i (the engine host) and 5.5c-ii (output); the playback defaults are
/// consumed at 5.5c-ii, where the queue builders read them instead of
/// `PlaybackConfig::default()`. `replaygain_mode` is stored as TEXT
/// (`off` / `track` / `album`) so this struct stays db-owned (the player layer
/// converts it to its `ReplayGain` enum).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioState {
    pub replaygain_mode: String,
    pub replaygain_preamp: f64,
    pub replaygain_clip: bool,
    pub gapless: bool,
    pub dsp: DspState,
    pub output_backend: String,
    pub resampler: ResamplerQuality,
    /// The global Smart Speed aggressiveness, stored as TEXT (`gentle` / `balanced`
    /// / `aggressive`) so this struct stays db-owned; the player layer converts it
    /// to `player::spoken::SmartSpeedLevel` (the `replaygain_mode` idiom).
    pub smart_speed_level: String,
    /// The repeat mode, stored as TEXT (`off` / `all` / `one`) the `replaygain_mode`
    /// idiom; the player layer converts it to `player::mode::Repeat` (Phase 17a).
    pub repeat: String,
    /// Shuffle mode: whether new play orders (a Play, a repeat-all lap) are
    /// shuffled in place (Phase 17b). Persisted so it survives a restart.
    pub shuffle: bool,
}

impl Default for AudioState {
    fn default() -> Self {
        // Matches `PlaybackConfig::default()` (album / preamp 0 / clip on /
        // gapless on) plus all DSP off and the auto output backend.
        Self {
            replaygain_mode: "album".to_string(),
            replaygain_preamp: 0.0,
            replaygain_clip: true,
            gapless: true,
            dsp: DspState::off(),
            output_backend: "auto".to_string(),
            resampler: ResamplerQuality::Default,
            smart_speed_level: "gentle".to_string(),
            repeat: "off".to_string(),
            shuffle: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_kind_round_trips_through_text() {
        for kind in [MediaKind::Track, MediaKind::Episode, MediaKind::Audiobook] {
            assert_eq!(kind.as_str().parse::<MediaKind>().unwrap(), kind);
        }
        assert!("bogus".parse::<MediaKind>().is_err());
    }

    #[test]
    fn resampler_quality_round_trips_and_rejects_bogus() {
        for q in [ResamplerQuality::Default, ResamplerQuality::High] {
            assert_eq!(q.as_str().parse::<ResamplerQuality>().unwrap(), q);
        }
        // The bogus case is what `get_audio_state` degrades to the default.
        assert!("bogus".parse::<ResamplerQuality>().is_err());
    }

    #[test]
    fn dsp_state_off_is_the_default() {
        let dsp = DspState::off();
        assert!(dsp.is_off());
        assert_eq!(dsp, DspState::default());
        // Enabling any module makes it non-off.
        let mut on = dsp;
        on.limiter.enabled = true;
        assert!(!on.is_off());
    }

    #[test]
    fn played_state_round_trips_through_i64() {
        for state in [
            PlayedState::Unplayed,
            PlayedState::InProgress,
            PlayedState::PlayedFully,
            PlayedState::ArchivedUnlistened,
        ] {
            assert_eq!(PlayedState::from_i64(state.as_i64()).unwrap(), state);
        }
        assert!(PlayedState::from_i64(9).is_err());
    }

    #[test]
    fn inbox_policy_round_trips_through_text() {
        for policy in [
            InboxPolicy::Inbox,
            InboxPolicy::AlwaysQueue,
            InboxPolicy::AlwaysArchive,
        ] {
            assert_eq!(policy.as_str().parse::<InboxPolicy>().unwrap(), policy);
        }
        assert!("bogus".parse::<InboxPolicy>().is_err());
    }

    #[test]
    fn track_serde_round_trip() {
        let track = Track {
            id: 7,
            album_id: Some(1),
            artist_id: Some(2),
            title: "Roygbiv".to_string(),
            track_no: Some(3),
            disc_no: Some(1),
            duration: Some(151.0),
            file_path: "Electronic/Boards of Canada/Music Has the Right (1998)/03 - Roygbiv.flac"
                .to_string(),
            format: Some("flac".to_string()),
            bitrate: Some(1024),
            sample_rate: Some(44100),
            replaygain_track: Some(-7.5),
            replaygain_album: Some(-7.2),
            rating: 5,
            play_count: 42,
            last_played: Some(Utc::now()),
            starred: true,
            musicbrainz_recording_id: None,
            added_at: Some(Utc::now()),
        };
        let json = serde_json::to_string(&track).unwrap();
        let back: Track = serde_json::from_str(&json).unwrap();
        assert_eq!(track, back);
    }
}
