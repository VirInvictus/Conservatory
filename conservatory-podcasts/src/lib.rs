//! Conservatory podcast plugin.
//!
//! A compile-time plugin: a feature-gated workspace crate, compiled into the
//! binaries when their `podcasts` feature is on (the default; spec §2.2). The
//! Belfry absorption lands here at Phase 6 (spec §8, §17): the per-show fetch
//! loop with conditional GET, `feed-rs` plus the hand-rolled `podcast:`
//! namespace handler, the Inbox → Queue → Played triage model, OPML
//! round-trip, and the podcast CLI verbs and GTK tab.
//!
//! The plugin boundary is code and dependencies, not the database: the podcast
//! schema lives in `conservatory-core`'s single migration ledger, and the
//! unified queue, libmpv host, and spoken-word profile (Smart Speed / Voice
//! Boost) are core. A music-only build simply has empty podcast tables.
//!
//! Phase 6a-ii-a landed the RSS-catching layer: the HTTP client ([`http`]) and
//! the conditional-GET [`Fetcher`], both ported from Viaduct (ATTRIBUTIONS.md).
//! Phase 6a-ii-b adds parsing ([`parse`] via feed-rs + the hand-rolled
//! [`namespace`] handler) and the [`refresh`] orchestration (fetch → parse →
//! upsert through the core worker). Phase 6a-iii-a adds [`opml`] import/export
//! round-trip. Phase 6a-iii-b adds the [`credentials`] store (HTTP Basic auth
//! in libsecret) and episode [`download`] into the managed tree. Phase
//! 6b-ii-c-3-b adds inbox-policy routing on the [`refresh`] path and
//! [`retention`] pruning of downloaded episodes beyond a show's `keep_count`.
//! Triage browse / actions are 6b.

pub mod chapters;
pub mod credentials;
pub mod download;
pub mod error;
pub mod fetcher;
pub mod http;
pub mod namespace;
pub mod notes;
pub mod opml;
pub mod parse;
pub mod refresh;
pub mod retention;
pub mod slug;

pub use chapters::{fetch_chapters, parse_chapters_json};
pub use credentials::{BasicAuth, CredentialStore};
pub use download::download_episode;
pub use error::{FetchError, Result};
pub use fetcher::{FetchResult, Fetcher};
pub use notes::sanitize_notes;
pub use opml::{ImportSummary, OpmlSubscription, export_opml, import_opml, parse_opml, write_opml};
pub use parse::{ChannelMeta, ParsedEpisode, ParsedFeed, parse_feed};
pub use refresh::{RefreshOutcome, RefreshStatus, add_show, refresh_all, refresh_show};
pub use retention::{RetentionPrune, prune};
