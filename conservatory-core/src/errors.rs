//! Error types for `conservatory-core`.
//!
//! Mirrors the `belfry-core` shape: a single `thiserror` enum, with the
//! channel-failure conversions that let `WorkerHandle` methods use `?` on the
//! `mpsc` send and `oneshot` receive that bracket every worker dispatch.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("tag error: {0}")]
    Tag(#[from] lofty::error::LoftyError),

    #[error("image error: {0}")]
    Image(#[from] image::ImageError),

    #[error("worker channel closed")]
    WorkerChannelClosed,

    #[error("invalid enum value: {field} = {value:?}")]
    InvalidEnum { field: &'static str, value: String },

    #[error("path template: {0}")]
    Template(String),

    #[error("move: {0}")]
    Move(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl<T> From<tokio::sync::mpsc::error::SendError<T>> for Error {
    fn from(_: tokio::sync::mpsc::error::SendError<T>) -> Self {
        Self::WorkerChannelClosed
    }
}

impl From<tokio::sync::oneshot::error::RecvError> for Error {
    fn from(_: tokio::sync::oneshot::error::RecvError) -> Self {
        Self::WorkerChannelClosed
    }
}
