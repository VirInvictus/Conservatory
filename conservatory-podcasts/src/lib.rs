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
//! Phase 6a-ii-a lands the RSS-catching layer: the HTTP client ([`http`]) and
//! the conditional-GET [`Fetcher`], both ported from Viaduct (ATTRIBUTIONS.md).
//! Parsing (feed-rs + the `podcast:` namespace handler) and the refresh
//! orchestration arrive at 6a-ii-b.

pub mod error;
pub mod fetcher;
pub mod http;

pub use error::{FetchError, Result};
pub use fetcher::{FetchResult, Fetcher};
