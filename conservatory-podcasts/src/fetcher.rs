//! Conditional-GET feed fetcher (Phase 6a-ii-a).
//!
//! The RSS-*catching* layer, ported from the network slice of Viaduct's
//! `network/fetcher.rs` (ATTRIBUTIONS.md; lineage NetNewsWire). It keeps the
//! parts a podcast refresh needs: conditional GET (ETag / Last-Modified with a
//! 304 short-circuit), and a per-host 429 cooldown honouring `Retry-After`.
//!
//! Deliberately simpler than Viaduct's: the broadcast request-coalescing is
//! dropped (each show has a distinct feed URL, so same-URL coalescing rarely
//! helps), and the content-hash re-parse skip is deferred to the refresh
//! orchestration (6a-ii-b), where the stored hash lives. Parsing (feed-rs +
//! the `podcast:` namespace handler) is 6a-ii-b; this layer only fetches bytes.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use reqwest::{Client, StatusCode, header};
use tokio::sync::Mutex;

use crate::credentials::BasicAuth;
use crate::error::{FetchError, Result};
use crate::http;

/// Backoff applied to a host that answers 429 without a numeric `Retry-After`
/// (a missing header or the HTTP-date form). Five minutes is conservative; a
/// host that means it usually sends a longer numeric value, which wins.
const DEFAULT_RATE_LIMIT_COOLDOWN_SECS: i64 = 300;

/// The outcome of a single conditional GET. On a 304 the body is empty and the
/// header fields are `None` (the stored values stay authoritative); on a 2xx
/// the caller persists `etag` / `last_modified` for the next request.
#[derive(Clone, Debug)]
pub struct FetchResult {
    pub status: u16,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub cache_control_max_age: Option<i64>,
}

/// A shared feed fetcher. Cheap to clone (the `reqwest::Client` is internally
/// an `Arc`, and the cooldown map is shared), so one `Fetcher` backs a whole
/// refresh cycle.
#[derive(Clone)]
pub struct Fetcher {
    client: Client,
    /// Per-host 429 cooldowns: a host maps to the instant it may be retried.
    cooldowns: Arc<Mutex<HashMap<String, DateTime<Utc>>>>,
}

impl Fetcher {
    /// Build a fetcher with the baseline client ([`http::build_client`]).
    pub fn new() -> Result<Self> {
        Ok(Self::with_client(http::build_client()?))
    }

    /// Build a fetcher over an existing client, so adjacent network work (the
    /// episode download path, 6a-iii) can share the connection pool.
    pub fn with_client(client: Client) -> Self {
        Self {
            client,
            cooldowns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Borrow the underlying client (cheap clone) to share the pool.
    pub fn client(&self) -> Client {
        self.client.clone()
    }

    /// Conditional GET of a feed. Sends `If-None-Match` / `If-Modified-Since`
    /// when the caller has them; returns [`FetchError::RateLimited`] if the
    /// host is cooling down or answers 429, an empty-body result on 304, and
    /// the body plus refreshed validators on any other status.
    pub async fn fetch(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<FetchResult> {
        self.fetch_authed(url, etag, last_modified, None).await
    }

    /// As [`fetch`](Self::fetch), but attaches HTTP Basic auth when `auth` is
    /// present (a private feed, spec §8). The no-auth path is identical.
    pub async fn fetch_authed(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
        auth: Option<&BasicAuth>,
    ) -> Result<FetchResult> {
        let parsed = reqwest::Url::parse(url).map_err(|e| FetchError::InvalidUrl(e.to_string()))?;
        let host = parsed.host_str().unwrap_or("").to_string();

        // Respect an in-effect cooldown before touching the network.
        {
            let cooldowns = self.cooldowns.lock().await;
            if let Some(retry_after) = cooldowns.get(&host)
                && Utc::now() < *retry_after
            {
                return Err(FetchError::RateLimited {
                    retry_after_secs: (*retry_after - Utc::now()).num_seconds().max(0) as u64,
                });
            }
        }

        let mut req = self
            .client
            .get(url)
            .header(header::ACCEPT, http::ACCEPT_FEED);
        if let Some(e) = etag {
            req = req.header(header::IF_NONE_MATCH, e);
        }
        if let Some(l) = last_modified {
            req = req.header(header::IF_MODIFIED_SINCE, l);
        }
        if let Some(a) = auth {
            req = req.basic_auth(&a.user, Some(&a.password));
        }
        tracing::debug!(
            url,
            conditional = etag.is_some() || last_modified.is_some(),
            "fetch: GET"
        );

        let response = req.send().await?;
        let status = response.status();

        if status == StatusCode::TOO_MANY_REQUESTS {
            // Honour a numeric `Retry-After` (delta-seconds). A missing header or
            // the HTTP-date form falls back to a default backoff, so a throttling
            // host always gets a cooldown rather than being re-hit next cycle
            // (the previous behaviour recorded none on a non-numeric value).
            let secs = header_str(&response, header::RETRY_AFTER)
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|s| *s >= 0)
                .unwrap_or(DEFAULT_RATE_LIMIT_COOLDOWN_SECS);
            {
                let mut cooldowns = self.cooldowns.lock().await;
                cooldowns.insert(host, Utc::now() + Duration::seconds(secs));
            }
            return Err(FetchError::RateLimited {
                retry_after_secs: secs as u64,
            });
        }

        if status == StatusCode::NOT_MODIFIED {
            tracing::debug!(url, "fetch: 304 (cached)");
            return Ok(FetchResult {
                status: status.as_u16(),
                body: Vec::new(),
                etag: None,
                last_modified: None,
                cache_control_max_age: None,
            });
        }

        let etag = header_str(&response, header::ETAG);
        let last_modified = header_str(&response, header::LAST_MODIFIED);
        let cache_control_max_age =
            header_str(&response, header::CACHE_CONTROL).and_then(|cc| parse_max_age(&cc));
        let body = response.bytes().await?.to_vec();
        tracing::debug!(
            url,
            status = status.as_u16(),
            body_bytes = body.len(),
            has_etag = etag.is_some(),
            "fetch: response"
        );

        Ok(FetchResult {
            status: status.as_u16(),
            body,
            etag,
            last_modified,
            cache_control_max_age,
        })
    }
}

/// Read a response header as an owned `String`, dropping non-ASCII values.
fn header_str(response: &reqwest::Response, name: header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Pull the `max-age=<secs>` directive out of a `Cache-Control` value.
fn parse_max_age(cache_control: &str) -> Option<i64> {
    cache_control
        .split(',')
        .filter_map(|part| part.trim().strip_prefix("max-age="))
        .find_map(|secs| secs.parse::<i64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_max_age_extracts_seconds() {
        assert_eq!(parse_max_age("max-age=3600"), Some(3600));
        assert_eq!(
            parse_max_age("public, max-age=120, must-revalidate"),
            Some(120)
        );
        assert_eq!(parse_max_age("no-cache"), None);
    }
}
