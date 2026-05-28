//! Conservatory headless data layer.
//!
//! The GUI-free core (spec §2.2): the single-writer SQLite worker and
//! read-only pool, tag read/write, the import pipeline (path-template engine,
//! shelf-genre resolver, file mover with dry-run + undo), the libmpv host and
//! playback profiles, the unified-queue model, and the podcast fetch/parse
//! pipeline (ported from `belfry-core` at Phase 6).
//!
//! Phase 0 skeleton: no implementation yet. Phase 1 (spec §17) brings the
//! SQLite worker, read pool, migrations, fixtures, and tag read.

/// Workspace version, surfaced for the CLI and GUI binaries.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
