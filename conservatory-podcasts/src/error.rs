//! Error types for the podcast fetch + parse + refresh layers (Phase 6a-ii).

use thiserror::Error;

/// A feed fetch/parse/refresh failure. Fetch variants land at 6a-ii-a; the
/// `Parse` and `Core` variants are added at 6a-ii-b for the refresh pipeline.
#[derive(Debug, Error)]
pub enum FetchError {
    /// The feed URL did not parse.
    #[error("invalid feed url: {0}")]
    InvalidUrl(String),

    /// The host is in a 429 cooldown; retry after the given seconds.
    #[error("rate limited; retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    /// A transport-level failure (connect, TLS, timeout, body read).
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// The fetched body was not a parseable feed (feed-rs rejected it).
    #[error("feed parse error: {0}")]
    Parse(String),

    /// A database error from the core worker / read pool during a refresh.
    #[error("database error: {0}")]
    Core(#[from] conservatory_core::errors::Error),
}

pub type Result<T> = std::result::Result<T, FetchError>;
