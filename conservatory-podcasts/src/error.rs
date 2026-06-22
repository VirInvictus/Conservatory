//! Error types for the podcast fetch layer (Phase 6a-ii-a).

use thiserror::Error;

/// A feed-fetch failure. Parse failures (feed-rs / namespace) land at 6a-ii-b
/// and get their own variants then.
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
}

pub type Result<T> = std::result::Result<T, FetchError>;
