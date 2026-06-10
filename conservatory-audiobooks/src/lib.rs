//! Conservatory audiobook plugin.
//!
//! A compile-time plugin: a feature-gated workspace crate, compiled into the
//! binaries when their `audiobooks` feature is on (the default; spec §2.2).
//! The audiobook subsystem lands here at Phase 7 (spec §3.8, §17): the tag +
//! sidecar reader (Audiobookshelf conventions: `.opf`, `desc.txt`,
//! `reader.txt`), the chapter resolver (embedded M4B markers or
//! one-file-per-chapter folders), book-state derivation (New / In progress /
//! Finished), and the audiobook CLI verbs and the Cozy-shaped shelf tab.
//!
//! The plugin boundary is code and dependencies, not the database: the book
//! schema lives in `conservatory-core`'s single migration ledger, and the
//! unified queue, libmpv host, file mover, path-template engine, and the
//! spoken-word profile shared with podcasts are core. A book is one queue
//! entry; chapters are intra-item navigation (spec §6.1).
//!
//! Phase 0.5 stub: no implementation until Phase 7a.
