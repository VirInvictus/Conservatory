//! Shared `reqwest::Client` construction for podcast feed fetches.
//!
//! Ported from Viaduct's `network/http.rs` (ATTRIBUTIONS.md), whose own
//! lineage is NetNewsWire. The baseline is the same one a feed reader wants:
//!
//! - **`gzip` + `brotli`** decompression. Servers that negotiate a compressed
//!   encoding otherwise hand back binary garbage and the feed parser sees an
//!   unknown format.
//! - **`rustls-tls`** for TLS, no system OpenSSL dependency.
//! - **Descriptive `User-Agent`** (product + version + contact URL), the
//!   NNW / NewsFlash convention; some hosts 403 short or unrecognized UAs.
//!
//! The `Accept` header is added per request (`ACCEPT_FEED`), not baked into the
//! client, so the download path (Phase 6a-iii) can negotiate audio separately.

use std::time::Duration;

use reqwest::Client;

const CONSERVATORY_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Cap idle connections per origin. reqwest defaults to `usize::MAX`, which on
/// a large subscription list holds a TLS session per host indefinitely (each
/// rustls session retains certs + keys, easily hundreds of KB). Four is enough
/// to pipeline a single host's hot paths without unbounded growth.
const POOL_MAX_IDLE_PER_HOST: usize = 4;

/// How long an idle connection sits in the pool before being closed. reqwest's
/// default is 90 s; 30 s drains the steady-state pool faster after a refresh
/// cycle ends (rustls session resumption covers the next cold start).
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Total per-request budget (connect + headers + body). reqwest defaults to no
/// timeout, so a host that accepts the connection but stalls the body hangs the
/// task until the OS TCP timeout (minutes). The refresh loop (Phase 6a-ii-b)
/// fans feeds out under a semaphore, so a few dead hosts could otherwise wedge
/// a whole cycle. 30 s clears any healthy feed with room to spare.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Connect-phase budget. Fail fast on dead / unroutable hosts rather than
/// holding a concurrency slot for the full request timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Composed at build time so the User-Agent always tracks the crate version.
/// Format mirrors NNW's `NetNewsWire/7.0.5 (Mac; +URL)`.
fn user_agent() -> String {
    format!(
        "Conservatory/{CONSERVATORY_VERSION} (podcast client; +https://github.com/virinvictus/Conservatory)"
    )
}

/// `Accept` header for feed fetches: every format we can parse, in preference
/// order. `*/*;q=0.5` catches misconfigured servers that answer `text/plain`.
pub const ACCEPT_FEED: &str = "application/rss+xml, application/atom+xml, application/feed+json, application/json;q=0.9, application/xml;q=0.8, text/xml;q=0.7, */*;q=0.5";

/// Build the baseline feed-fetch client (UA + rustls + gzip + brotli + the pool
/// caps and timeouts above).
pub fn build_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .user_agent(user_agent())
        .use_rustls_tls()
        .gzip(true)
        .brotli(true)
        .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_names_conservatory() {
        let ua = user_agent();
        assert!(ua.starts_with("Conservatory/"), "ua = {ua}");
        assert!(ua.contains("podcast client"));
    }

    #[test]
    fn client_builds() {
        assert!(build_client().is_ok());
    }
}
