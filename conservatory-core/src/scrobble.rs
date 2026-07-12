//! Listening-history scrobbling (Phase 9, spec §14 carve-out).
//!
//! A one-way, local-first, off-by-default submission of completed plays to an
//! external history service. This is the deliberate, scoped reversal of the
//! §14 "no social" line: with `[scrobble] enabled = false` (the default) the
//! whole subsystem is inert and the app is unchanged and fully offline.
//!
//! Local-first shape (spec §9 usable-artifact): a completed play is written to
//! the `scrobble_outbox` table *first* (Phase 9a, snapshotting the metadata),
//! and a background submitter ([`run`]) drains it, so a listen survives an
//! offline window or a down service and is never lost. **ListenBrainz leads**
//! (open, self-hostable, fits the offline-first rule); Last.fm is the optional
//! second target added at Phase 9c.
//!
//! Layering (the CLAUDE.md rule: logic in core, CLI-testable):
//! - The neutral [`Listen`] and the pure ListenBrainz payload builder
//!   ([`listenbrainz_submit_body`]) are unit-tested without a network.
//! - [`ListenSubmitter`] is the drain loop's dependency, so the loop is tested
//!   with a fake that programs failures; [`ListenBrainzClient`] is the real
//!   reqwest implementation, exercised against a wiremock server.

use std::time::Duration;

use md5::{Digest, Md5};
use serde_json::{Value, json};

use crate::db::models::PendingScrobble;
use crate::db::{ReadPool, WorkerHandle, pending_scrobbles};
use crate::errors::Result;

/// The version string stamped into a submission's `submission_client_version`.
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The public ListenBrainz API root. A self-hosted instance overrides this via
/// [`ListenBrainzClient::with_base_url`].
pub const LISTENBRAINZ_API_ROOT: &str = "https://api.listenbrainz.org";

/// Which history service a listen is bound for. The serde form is the lowercase
/// token the config file and the `scrobble_outbox.service` column carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrobbleService {
    ListenBrainz,
    Lastfm,
}

impl ScrobbleService {
    /// The stable string used in config and the outbox `service` column.
    pub fn as_str(self) -> &'static str {
        match self {
            ScrobbleService::ListenBrainz => "listenbrainz",
            ScrobbleService::Lastfm => "lastfm",
        }
    }

    /// Parse the config/DB token; unknown values fall back to ListenBrainz (the
    /// forgiving default), matching the rest of the config's degrade-not-error
    /// stance.
    pub fn parse(s: &str) -> ScrobbleService {
        match s.trim().to_ascii_lowercase().as_str() {
            "lastfm" | "last.fm" => ScrobbleService::Lastfm,
            _ => ScrobbleService::ListenBrainz,
        }
    }

    /// The libsecret reference key the service's user token is stored under (one
    /// token per service, so switching services keeps both).
    pub fn token_ref(self) -> &'static str {
        match self {
            ScrobbleService::ListenBrainz => "scrobble.listenbrainz.token",
            ScrobbleService::Lastfm => "scrobble.lastfm.session",
        }
    }
}

/// A service-neutral completed listen, built from a `scrobble_outbox` row. The
/// per-service clients translate it into their wire format.
#[derive(Debug, Clone, PartialEq)]
pub struct Listen {
    /// Unix seconds the play completed.
    pub listened_at: i64,
    pub artist: String,
    pub track: String,
    pub album: Option<String>,
    pub track_number: Option<i64>,
    pub duration_secs: Option<i64>,
    pub recording_mbid: Option<String>,
}

/// Why a submission failed, split by whether retrying can help (the drain loop's
/// decision). `Transient` is retried with backoff; `Permanent` is parked (a bad
/// token or a rejected payload will not fix itself by retrying).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitError {
    /// Network, timeout, 429, or 5xx: retry later.
    Transient(String),
    /// 4xx (bad token / rejected payload): parking, not retrying.
    Permanent(String),
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::Transient(m) => write!(f, "transient: {m}"),
            SubmitError::Permanent(m) => write!(f, "permanent: {m}"),
        }
    }
}

/// The drain loop's dependency: submit one completed listen. Implemented by
/// [`ListenBrainzClient`] (real) and, in tests, a fake that programs failures.
#[allow(async_fn_in_trait)]
pub trait ListenSubmitter {
    /// Submit a single completed listen. `Ok(())` means the service accepted it
    /// (the outbox row can be deleted).
    async fn submit(&self, listen: &Listen) -> std::result::Result<(), SubmitError>;
}

/// Build the ListenBrainz `submit-listens` request body for one completed listen
/// (`listen_type: "single"`). Pure and unit-tested; the client just POSTs it.
///
/// See <https://listenbrainz.readthedocs.io/en/latest/users/api/core.html>.
pub fn listenbrainz_submit_body(listen: &Listen) -> Value {
    let mut additional = json!({
        "media_player": "Conservatory",
        "submission_client": "Conservatory",
        "submission_client_version": CLIENT_VERSION,
    });
    let obj = additional
        .as_object_mut()
        .expect("additional_info is an object");
    if let Some(d) = listen.duration_secs {
        obj.insert("duration_ms".to_string(), json!(d * 1000));
    }
    if let Some(n) = listen.track_number {
        obj.insert("tracknumber".to_string(), json!(n));
    }
    if let Some(mbid) = &listen.recording_mbid {
        obj.insert("recording_mbid".to_string(), json!(mbid));
    }

    let mut track_metadata = json!({
        "artist_name": listen.artist,
        "track_name": listen.track,
        "additional_info": additional,
    });
    if let Some(album) = &listen.album {
        track_metadata
            .as_object_mut()
            .expect("track_metadata is an object")
            .insert("release_name".to_string(), json!(album));
    }

    json!({
        "listen_type": "single",
        "payload": [ { "listened_at": listen.listened_at, "track_metadata": track_metadata } ],
    })
}

/// A ListenBrainz client (Phase 9a). Holds the user token and a reqwest client;
/// the base URL defaults to the public instance and is overridable for a
/// self-hosted server or a wiremock test.
#[derive(Clone)]
pub struct ListenBrainzClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl ListenBrainzClient {
    /// Build a client for `token` against the public ListenBrainz instance.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            http: default_http(),
            base_url: LISTENBRAINZ_API_ROOT.to_string(),
            token: token.into(),
        }
    }

    /// Point the client at a different API root (a self-hosted instance, or a
    /// wiremock server in tests). The URL is used without a trailing slash.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    /// Validate the token against `GET /1/validate-token`, returning the
    /// associated user name when valid. A transport failure is an `Err`; a
    /// well-formed "invalid token" response is `Ok(None)`.
    pub async fn validate_token(&self) -> std::result::Result<Option<String>, SubmitError> {
        let url = format!("{}/1/validate-token", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Token {}", self.token))
            .send()
            .await
            .map_err(|e| SubmitError::Transient(e.to_string()))?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| SubmitError::Transient(e.to_string()))?;
        if body.get("valid").and_then(Value::as_bool) == Some(true) {
            Ok(body
                .get("user_name")
                .and_then(Value::as_str)
                .map(str::to_string))
        } else {
            Ok(None)
        }
    }
}

impl ListenSubmitter for ListenBrainzClient {
    async fn submit(&self, listen: &Listen) -> std::result::Result<(), SubmitError> {
        let url = format!("{}/1/submit-listens", self.base_url);
        let body = listenbrainz_submit_body(listen);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Token {}", self.token))
            .json(&body)
            .send()
            .await
            .map_err(|e| SubmitError::Transient(e.to_string()))?;
        classify_status(resp.status().as_u16())
    }
}

/// Map an HTTP status to the submit outcome. 2xx succeeds; 429 and 5xx are
/// transient (retry); other 4xx are permanent (park). Shared so Last.fm (9c)
/// classifies the same way.
pub fn classify_status(status: u16) -> std::result::Result<(), SubmitError> {
    match status {
        200..=299 => Ok(()),
        429 => Err(SubmitError::Transient(format!("http {status}"))),
        500..=599 => Err(SubmitError::Transient(format!("http {status}"))),
        _ => Err(SubmitError::Permanent(format!("http {status}"))),
    }
}

// --- Last.fm (Phase 9c) --------------------------------------------------

/// The Last.fm API endpoint every method POSTs (scrobbles) or GETs (auth)
/// against. Overridable for a wiremock test via [`LastfmClient::with_base_url`].
pub const LASTFM_API_ROOT: &str = "https://ws.audioscrobbler.com/2.0";

/// The user-facing authorization page. The desktop web-auth flow sends the user
/// here (with the app key and a request token) to approve access.
pub const LASTFM_AUTH_ROOT: &str = "https://www.last.fm/api/auth/";

/// Sign a Last.fm request: MD5 of every `name` immediately followed by its
/// `value`, ordered by name, with the shared secret appended, hex lowercase.
/// `format` and `api_sig` are excluded from the signature by the API contract,
/// so callers pass only the signed params here and add `format` afterward. Pure
/// and unit-tested against a known vector.
pub fn lastfm_sign(params: &[(&str, &str)], secret: &str) -> String {
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut buf = String::new();
    for (name, value) in sorted {
        buf.push_str(name);
        buf.push_str(value);
    }
    buf.push_str(secret);
    Md5::digest(buf.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Build the (unsigned, `format`-free) `track.scrobble` form parameters for one
/// completed listen. The client signs this list and appends `api_sig` +
/// `format=json` before sending; kept secret-free so it is pure and unit-tested.
/// A single listen uses the unindexed param form (`artist`, not `artist[0]`),
/// which Last.fm accepts for a one-track scrobble (the outbox is one row = one
/// listen).
pub fn lastfm_scrobble_params(
    listen: &Listen,
    api_key: &str,
    session_key: &str,
) -> Vec<(String, String)> {
    let mut params = vec![
        ("method".to_string(), "track.scrobble".to_string()),
        ("api_key".to_string(), api_key.to_string()),
        ("sk".to_string(), session_key.to_string()),
        ("artist".to_string(), listen.artist.clone()),
        ("track".to_string(), listen.track.clone()),
        ("timestamp".to_string(), listen.listened_at.to_string()),
    ];
    if let Some(album) = &listen.album {
        params.push(("album".to_string(), album.clone()));
    }
    if let Some(n) = listen.track_number {
        params.push(("trackNumber".to_string(), n.to_string()));
    }
    if let Some(d) = listen.duration_secs {
        params.push(("duration".to_string(), d.to_string()));
    }
    if let Some(mbid) = &listen.recording_mbid {
        params.push(("mbid".to_string(), mbid.clone()));
    }
    params
}

/// Map a Last.fm API error code to the submit outcome. 11 (service offline), 16
/// (temporarily unavailable), and 29 (rate limit) are transient (retry);
/// everything else (9 invalid session, bad params / method / api key, suspended)
/// is permanent (park). Unlike ListenBrainz, Last.fm returns HTTP 200 with an
/// error body, so this classifies the body, not the status.
pub fn classify_lastfm_error(code: i64) -> SubmitError {
    match code {
        11 | 16 | 29 => SubmitError::Transient(format!("lastfm error {code}")),
        _ => SubmitError::Permanent(format!("lastfm error {code}")),
    }
}

/// A Last.fm client (Phase 9c). Holds the app key + shared secret (config-backed,
/// spec §14 / roadmap: not baked into the binary) and the user session key
/// (libsecret). For the connect flow, build one with an empty `session_key` and
/// use [`get_token`](Self::get_token) / [`get_session`](Self::get_session).
#[derive(Clone)]
pub struct LastfmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    secret: String,
    session_key: String,
}

impl LastfmClient {
    /// Build a client against the public Last.fm endpoint.
    pub fn new(
        api_key: impl Into<String>,
        secret: impl Into<String>,
        session_key: impl Into<String>,
    ) -> Self {
        Self {
            http: default_http(),
            base_url: LASTFM_API_ROOT.to_string(),
            api_key: api_key.into(),
            secret: secret.into(),
            session_key: session_key.into(),
        }
    }

    /// Point the client at a different endpoint (a wiremock server in tests).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    /// Sign a signed-param set with the shared secret, returning `api_sig`.
    fn sign(&self, signed: &[(&str, &str)]) -> String {
        lastfm_sign(signed, &self.secret)
    }

    /// Parse a Last.fm JSON reply, mapping an `{error, ...}` body to a
    /// [`SubmitError`] via [`classify_lastfm_error`].
    async fn read_json(resp: reqwest::Response) -> std::result::Result<Value, SubmitError> {
        let status = resp.status().as_u16();
        if !(200..=299).contains(&status) {
            // A transport-level non-2xx (rare for Last.fm) still classifies
            // by HTTP status, sharing the ListenBrainz rule.
            classify_status(status)?;
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| SubmitError::Transient(e.to_string()))?;
        if let Some(code) = body.get("error").and_then(Value::as_i64) {
            return Err(classify_lastfm_error(code));
        }
        Ok(body)
    }

    /// Step 1 of the desktop web-auth flow: fetch an unauthorized request token
    /// (`auth.getToken`). The user then approves it at [`auth_url`](Self::auth_url).
    pub async fn get_token(&self) -> std::result::Result<String, SubmitError> {
        let signed = [
            ("api_key", self.api_key.as_str()),
            ("method", "auth.getToken"),
        ];
        let sig = self.sign(&signed);
        let params = [
            ("api_key", self.api_key.as_str()),
            ("method", "auth.getToken"),
            ("api_sig", sig.as_str()),
            ("format", "json"),
        ];
        let resp = self
            .http
            .get(&self.base_url)
            .query(&params)
            .send()
            .await
            .map_err(|e| SubmitError::Transient(e.to_string()))?;
        let body = Self::read_json(resp).await?;
        body.get("token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| SubmitError::Permanent("no token in getToken reply".to_string()))
    }

    /// The URL to send the user to so they authorize the request `token`.
    pub fn auth_url(&self, token: &str) -> String {
        format!(
            "{LASTFM_AUTH_ROOT}?api_key={}&token={}",
            self.api_key, token
        )
    }

    /// Step 2: exchange an approved request `token` for a permanent session key
    /// (`auth.getSession`), returning `(session_key, username)`.
    pub async fn get_session(
        &self,
        token: &str,
    ) -> std::result::Result<(String, String), SubmitError> {
        let signed = [
            ("api_key", self.api_key.as_str()),
            ("method", "auth.getSession"),
            ("token", token),
        ];
        let sig = self.sign(&signed);
        let params = [
            ("api_key", self.api_key.as_str()),
            ("method", "auth.getSession"),
            ("token", token),
            ("api_sig", sig.as_str()),
            ("format", "json"),
        ];
        let resp = self
            .http
            .get(&self.base_url)
            .query(&params)
            .send()
            .await
            .map_err(|e| SubmitError::Transient(e.to_string()))?;
        let body = Self::read_json(resp).await?;
        let session = body
            .get("session")
            .ok_or_else(|| SubmitError::Permanent("no session in getSession reply".to_string()))?;
        let key = session
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| SubmitError::Permanent("no session key".to_string()))?;
        let name = session
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Ok((key.to_string(), name))
    }
}

impl ListenSubmitter for LastfmClient {
    async fn submit(&self, listen: &Listen) -> std::result::Result<(), SubmitError> {
        let mut params = lastfm_scrobble_params(listen, &self.api_key, &self.session_key);
        // Sign the params as they stand, then append the signature and format
        // (both excluded from the signature).
        let sig = {
            let refs: Vec<(&str, &str)> = params
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            self.sign(&refs)
        };
        params.push(("api_sig".to_string(), sig));
        params.push(("format".to_string(), "json".to_string()));
        let resp = self
            .http
            .post(&self.base_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| SubmitError::Transient(e.to_string()))?;
        Self::read_json(resp).await.map(|_| ())
    }
}

// --- The drain loop (Phase 9a) -------------------------------------------

/// Base retry delay after the first transient failure (doubled per attempt).
const BASE_BACKOFF_SECS: i64 = 60;
/// Retry-delay ceiling: a persistently-down service is retried at most hourly.
const MAX_BACKOFF_SECS: i64 = 3600;
/// A permanent failure (bad token / rejected payload) is parked this far out
/// rather than deleted, so the listen is never lost: once the user fixes the
/// token it retries within a day (or the CLI `flush` verb forces it sooner).
const PARK_SECS: i64 = 86_400;
/// How many outbox rows one drain pass submits (a bound, not a cap on the
/// queue: the next pass takes the next batch).
const DRAIN_BATCH: i64 = 50;
/// How often the background loop wakes to drain. Scrobbles are not latency-
/// sensitive; a completed play waits at most this long before its first attempt.
const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// The delay (seconds) before the `attempts`-th retry: exponential from
/// [`BASE_BACKOFF_SECS`], capped at [`MAX_BACKOFF_SECS`]. `attempts` is the
/// count *after* the failing attempt (so the first failure, `attempts == 1`,
/// waits the base delay). Pure and unit-tested.
pub fn backoff_secs(attempts: i64) -> i64 {
    if attempts <= 1 {
        return BASE_BACKOFF_SECS;
    }
    // Shift can overflow for large attempt counts; saturate to the cap.
    let shift = (attempts - 1).min(20) as u32;
    BASE_BACKOFF_SECS
        .saturating_mul(1_i64 << shift)
        .min(MAX_BACKOFF_SECS)
}

/// Build a neutral [`Listen`] from a queued outbox row.
fn listen_of(row: &PendingScrobble) -> Listen {
    Listen {
        listened_at: row.listened_at,
        artist: row.artist.clone(),
        track: row.track.clone(),
        album: row.album.clone(),
        track_number: row.track_number,
        duration_secs: row.duration_secs,
        recording_mbid: row.recording_mbid.clone(),
    }
}

/// What one drain pass did, for logging and the CLI `flush` verb.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrainReport {
    /// Rows accepted by the service and deleted from the outbox.
    pub submitted: usize,
    /// Rows that failed transiently and were rescheduled with backoff.
    pub retried: usize,
    /// Rows that failed permanently and were parked.
    pub parked: usize,
}

/// Drain the ready rows (`next_attempt_at <= now`) for `service` once: submit
/// each through `submitter`, then delete on success, reschedule with backoff on
/// a transient failure, or park on a permanent one. Reads via the pool, writes
/// via the worker (spec §2.1). Rows for a different service are left untouched.
///
/// Returns `Err` only if the worker channel is gone (the caller's cue to stop).
pub async fn drain_ready<S: ListenSubmitter>(
    worker: &WorkerHandle,
    pool: &ReadPool,
    service: ScrobbleService,
    submitter: &S,
    now: i64,
    limit: i64,
) -> Result<DrainReport> {
    let ready = {
        let conn = pool.open()?;
        pending_scrobbles(&conn, now, limit)?
    };

    let mut report = DrainReport::default();
    for row in ready {
        if ScrobbleService::parse(&row.service) != service {
            continue; // a different service's listen; not ours to submit.
        }
        match submitter.submit(&listen_of(&row)).await {
            Ok(()) => {
                worker.delete_scrobble(row.id).await?;
                report.submitted += 1;
            }
            Err(SubmitError::Transient(msg)) => {
                let next = now + backoff_secs(row.attempts + 1);
                worker.bump_scrobble_attempt(row.id, next).await?;
                report.retried += 1;
                tracing::debug!(id = row.id, attempts = row.attempts + 1, %msg, "scrobble retry");
            }
            Err(SubmitError::Permanent(msg)) => {
                worker
                    .bump_scrobble_attempt(row.id, now + PARK_SECS)
                    .await?;
                report.parked += 1;
                tracing::warn!(id = row.id, %msg, "scrobble parked (permanent failure)");
            }
        }
    }
    Ok(report)
}

/// The background submitter (Phase 9a): wake on [`POLL_INTERVAL`] and drain the
/// outbox for `service` through `submitter`. Mirrors [`crate::mpris::run`]; the
/// GUI spawns it on its runtime (Phase 9b) and aborts it on teardown. Returns
/// when the worker channel closes (a clean shutdown signal).
pub async fn run<S: ListenSubmitter>(
    worker: WorkerHandle,
    pool: ReadPool,
    service: ScrobbleService,
    submitter: S,
) -> Result<()> {
    tracing::info!(service = service.as_str(), "scrobble submitter started");
    loop {
        let now = chrono::Utc::now().timestamp();
        match drain_ready(&worker, &pool, service, &submitter, now, DRAIN_BATCH).await {
            Ok(report) => {
                if report.submitted + report.retried + report.parked > 0 {
                    tracing::debug!(?report, "scrobble drain");
                }
            }
            Err(e) => {
                tracing::info!(error = %e, "scrobble submitter stopping");
                return Ok(());
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The scrobble HTTP client: rustls, a descriptive UA, and sane timeouts (the
/// podcast `http.rs` baseline, minus the feed-specific `Accept`).
fn default_http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(format!(
            "Conservatory/{CLIENT_VERSION} (+https://github.com/virinvictus/Conservatory)"
        ))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Listen {
        Listen {
            listened_at: 1_700_000_000,
            artist: "Boards of Canada".to_string(),
            track: "Roygbiv".to_string(),
            album: Some("Music Has the Right to Children".to_string()),
            track_number: Some(4),
            duration_secs: Some(150),
            recording_mbid: Some("abc-123".to_string()),
        }
    }

    #[test]
    fn service_round_trips_and_is_forgiving() {
        assert_eq!(ScrobbleService::ListenBrainz.as_str(), "listenbrainz");
        assert_eq!(ScrobbleService::Lastfm.as_str(), "lastfm");
        assert_eq!(
            ScrobbleService::parse("listenbrainz"),
            ScrobbleService::ListenBrainz
        );
        assert_eq!(ScrobbleService::parse("LastFM"), ScrobbleService::Lastfm);
        assert_eq!(ScrobbleService::parse("last.fm"), ScrobbleService::Lastfm);
        // Unknown degrades to the default, not an error.
        assert_eq!(
            ScrobbleService::parse("spotify"),
            ScrobbleService::ListenBrainz
        );
    }

    #[test]
    fn submit_body_has_the_listenbrainz_shape() {
        let body = listenbrainz_submit_body(&sample());
        assert_eq!(body["listen_type"], "single");
        let payload = &body["payload"][0];
        assert_eq!(payload["listened_at"], 1_700_000_000);
        let meta = &payload["track_metadata"];
        assert_eq!(meta["artist_name"], "Boards of Canada");
        assert_eq!(meta["track_name"], "Roygbiv");
        assert_eq!(meta["release_name"], "Music Has the Right to Children");
        let ai = &meta["additional_info"];
        assert_eq!(ai["submission_client"], "Conservatory");
        assert_eq!(ai["duration_ms"], 150_000); // seconds -> ms
        assert_eq!(ai["tracknumber"], 4);
        assert_eq!(ai["recording_mbid"], "abc-123");
    }

    #[test]
    fn submit_body_omits_absent_optional_fields() {
        let listen = Listen {
            album: None,
            track_number: None,
            duration_secs: None,
            recording_mbid: None,
            ..sample()
        };
        let body = listenbrainz_submit_body(&listen);
        let meta = &body["payload"][0]["track_metadata"];
        assert!(meta.get("release_name").is_none());
        let ai = &meta["additional_info"];
        assert!(ai.get("duration_ms").is_none());
        assert!(ai.get("tracknumber").is_none());
        assert!(ai.get("recording_mbid").is_none());
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_secs(1), 60); // first failure: base
        assert_eq!(backoff_secs(2), 120);
        assert_eq!(backoff_secs(3), 240);
        assert_eq!(backoff_secs(4), 480);
        // Exponential, but capped at the hourly ceiling.
        assert_eq!(backoff_secs(100), 3600);
        // Defensive: attempts <= 1 (including 0) never underflows.
        assert_eq!(backoff_secs(0), 60);
    }

    #[test]
    fn status_classification_splits_transient_from_permanent() {
        assert!(classify_status(200).is_ok());
        assert_eq!(
            classify_status(429),
            Err(SubmitError::Transient("http 429".to_string()))
        );
        assert_eq!(
            classify_status(503),
            Err(SubmitError::Transient("http 503".to_string()))
        );
        assert_eq!(
            classify_status(401),
            Err(SubmitError::Permanent("http 401".to_string()))
        );
        assert_eq!(
            classify_status(400),
            Err(SubmitError::Permanent("http 400".to_string()))
        );
    }

    #[test]
    fn lastfm_signature_matches_known_vector() {
        // MD5("api_key" "abc" "method" "auth.getToken" "secret"), the params
        // sorted by name then the shared secret appended (verified with md5sum).
        let sig = lastfm_sign(&[("api_key", "abc"), ("method", "auth.getToken")], "secret");
        assert_eq!(sig, "f86444211049e605f18c05a5964aabfc");
        // The signer sorts, so param order at the call site does not matter.
        let reordered = lastfm_sign(&[("method", "auth.getToken"), ("api_key", "abc")], "secret");
        assert_eq!(sig, reordered);
    }

    #[test]
    fn lastfm_error_splits_transient_from_permanent() {
        // Service down / temporarily unavailable / rate-limited: retry.
        assert!(matches!(
            classify_lastfm_error(11),
            SubmitError::Transient(_)
        ));
        assert!(matches!(
            classify_lastfm_error(16),
            SubmitError::Transient(_)
        ));
        assert!(matches!(
            classify_lastfm_error(29),
            SubmitError::Transient(_)
        ));
        // Invalid session key (9) and anything else: park, do not retry.
        assert!(matches!(
            classify_lastfm_error(9),
            SubmitError::Permanent(_)
        ));
        assert!(matches!(
            classify_lastfm_error(4),
            SubmitError::Permanent(_)
        ));
    }

    #[test]
    fn lastfm_scrobble_params_carry_required_and_present_optional() {
        let params = lastfm_scrobble_params(&sample(), "APIKEY", "SESSION");
        let get = |k: &str| {
            params
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("method"), Some("track.scrobble"));
        assert_eq!(get("api_key"), Some("APIKEY"));
        assert_eq!(get("sk"), Some("SESSION"));
        assert_eq!(get("artist"), Some("Boards of Canada"));
        assert_eq!(get("track"), Some("Roygbiv"));
        assert_eq!(get("timestamp"), Some("1700000000"));
        assert_eq!(get("album"), Some("Music Has the Right to Children"));
        assert_eq!(get("trackNumber"), Some("4"));
        assert_eq!(get("duration"), Some("150"));
        assert_eq!(get("mbid"), Some("abc-123"));
        // `format` and `api_sig` are added by the client after signing, not here.
        assert!(get("format").is_none());
        assert!(get("api_sig").is_none());
    }

    #[test]
    fn lastfm_scrobble_params_omit_absent_optional() {
        let listen = Listen {
            album: None,
            track_number: None,
            duration_secs: None,
            recording_mbid: None,
            ..sample()
        };
        let params = lastfm_scrobble_params(&listen, "k", "s");
        for absent in ["album", "trackNumber", "duration", "mbid"] {
            assert!(
                !params.iter().any(|(name, _)| name == absent),
                "expected {absent} to be omitted"
            );
        }
    }
}
