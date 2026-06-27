//! Error type for the audiobook plugin (Phase 7a-ii reader, 7a-iii import).
//!
//! The reader is deliberately tolerant: a missing field is `None`, not an error.
//! These variants cover the genuine failures (an unreadable file, a broken
//! probe), never an absent tag or sidecar. The import pipeline (7a-iii) also
//! surfaces core worker / mover failures through [`ReadError::Core`].

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReadError {
    /// A filesystem failure (reading the folder, a file, or a sidecar).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// lofty could not read the file's tags or audio properties.
    #[error("tag read error: {0}")]
    Lofty(#[from] lofty::error::LoftyError),

    /// The source path held no audio files (nothing to read into a book).
    #[error("no audio files found under {0}")]
    NoAudio(String),

    /// The `ffprobe` binary is not on PATH (embedded-chapter read needs it). The
    /// chapter resolver treats this as "no embedded chapters" and falls back, so
    /// it never aborts a read; the variant exists for callers that want to know.
    #[error("ffprobe not found on PATH")]
    FfprobeMissing,

    /// `ffprobe` ran but failed, or its JSON did not parse.
    #[error("ffprobe error: {0}")]
    Ffprobe(String),

    /// A core failure during import (the single-writer worker, the read pool, or
    /// the file mover). Carries the underlying `conservatory-core` error.
    #[error("core error: {0}")]
    Core(#[from] conservatory_core::errors::Error),
}

pub type Result<T> = std::result::Result<T, ReadError>;
