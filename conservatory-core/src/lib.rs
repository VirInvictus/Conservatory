//! Conservatory headless data layer.
//!
//! The GUI-free core (spec §2.2): the single-writer SQLite worker and
//! read-only pool, tag read/write, the import pipeline (path-template engine,
//! shelf-genre resolver, file mover with dry-run + undo), the libmpv host and
//! playback profiles, the unified-queue model, and the podcast fetch/parse
//! pipeline (ported from `belfry-core` at Phase 6).
//!
//! Phase 1a (spec §17) lands the spine: the single-writer SQLite worker, the
//! read-only pool, and the numbered-migration runner. The data model, tag
//! read, and the rest follow in later sub-phases.

pub mod db;
pub mod errors;

/// Workspace version, surfaced for the CLI and GUI binaries.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
