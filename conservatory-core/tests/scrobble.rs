//! Phase 9a integration tests: the scrobble outbox survives an offline window
//! and retries with backoff, permanent failures park, and the real
//! ListenBrainz client speaks the wire protocol (via a mock server).

use std::sync::Mutex;

use conservatory_core::db::{NewScrobble, ReadPool, count_pending_scrobbles, spawn_worker};
use conservatory_core::scrobble::{
    LastfmClient, Listen, ListenBrainzClient, ListenSubmitter, ScrobbleService, SubmitError,
    drain_ready,
};
use tempfile::tempdir;
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A programmable submitter: it returns whatever `mode` says and records the
/// listens it accepted, so a test can simulate an offline window and then check
/// exactly what reached the service.
enum Mode {
    Online,
    Transient,
    Permanent,
}

struct FakeSubmitter {
    mode: Mutex<Mode>,
    accepted: Mutex<Vec<Listen>>,
}

impl FakeSubmitter {
    fn new(mode: Mode) -> Self {
        Self {
            mode: Mutex::new(mode),
            accepted: Mutex::new(Vec::new()),
        }
    }
    fn set(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }
    fn accepted(&self) -> Vec<Listen> {
        self.accepted.lock().unwrap().clone()
    }
}

impl ListenSubmitter for FakeSubmitter {
    async fn submit(&self, listen: &Listen) -> Result<(), SubmitError> {
        match *self.mode.lock().unwrap() {
            Mode::Online => {
                self.accepted.lock().unwrap().push(listen.clone());
                Ok(())
            }
            Mode::Transient => Err(SubmitError::Transient("offline".into())),
            Mode::Permanent => Err(SubmitError::Permanent("bad token".into())),
        }
    }
}

fn listen(service: &str, track: &str, listened_at: i64) -> NewScrobble {
    NewScrobble {
        service: service.to_string(),
        kind: "track".to_string(),
        listened_at,
        artist: "Aphex Twin".to_string(),
        track: track.to_string(),
        album: Some("Selected Ambient Works 85-92".to_string()),
        track_number: Some(1),
        duration_secs: Some(300),
        recording_mbid: None,
    }
}

/// The headline test: two listens are queued, an offline window keeps them (with
/// backoff), and when the service returns they submit and clear. Nothing is lost.
#[tokio::test]
async fn outbox_survives_offline_then_submits() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    worker
        .enqueue_scrobble(listen("listenbrainz", "Xtal", 1000), 1000)
        .await
        .unwrap();
    worker
        .enqueue_scrobble(listen("listenbrainz", "Tha", 1001), 1001)
        .await
        .unwrap();

    let count = |pool: &ReadPool| count_pending_scrobbles(&pool.open().unwrap()).unwrap();
    assert_eq!(count(&pool), 2, "both listens queued");

    let fake = FakeSubmitter::new(Mode::Transient);
    let now = 2000;

    // Offline: both fail transiently and are rescheduled with backoff, not lost.
    let r = drain_ready(
        &worker,
        &pool,
        ScrobbleService::ListenBrainz,
        &fake,
        now,
        50,
    )
    .await
    .unwrap();
    assert_eq!(r.retried, 2);
    assert_eq!(r.submitted, 0);
    assert_eq!(count(&pool), 2, "retried listens stay queued");

    // Still inside the backoff window: the rows are not yet ready, so a drain at
    // the same instant does nothing (proves backoff parked them, not a busy loop).
    let r = drain_ready(
        &worker,
        &pool,
        ScrobbleService::ListenBrainz,
        &fake,
        now,
        50,
    )
    .await
    .unwrap();
    assert_eq!(r.retried, 0);
    assert_eq!(r.submitted, 0);
    assert_eq!(count(&pool), 2);

    // Service returns; drain past the backoff point submits both and clears them.
    fake.set(Mode::Online);
    let later = now + 10_000;
    let r = drain_ready(
        &worker,
        &pool,
        ScrobbleService::ListenBrainz,
        &fake,
        later,
        50,
    )
    .await
    .unwrap();
    assert_eq!(r.submitted, 2);
    assert_eq!(
        count(&pool),
        0,
        "submitted listens are gone from the outbox"
    );

    let accepted = fake.accepted();
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].track, "Xtal");
    assert_eq!(accepted[0].artist, "Aphex Twin");
    assert_eq!(accepted[1].track, "Tha");
}

/// A permanent failure (bad token / rejected payload) parks the row far out
/// rather than deleting it, so the listen is not lost and does not hammer.
#[tokio::test]
async fn permanent_failure_parks_the_row() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    worker
        .enqueue_scrobble(listen("listenbrainz", "Ptolemy", 1000), 1000)
        .await
        .unwrap();

    let fake = FakeSubmitter::new(Mode::Permanent);
    let now = 2000;
    let r = drain_ready(
        &worker,
        &pool,
        ScrobbleService::ListenBrainz,
        &fake,
        now,
        50,
    )
    .await
    .unwrap();
    assert_eq!(r.parked, 1);
    assert_eq!(count_pending_scrobbles(&pool.open().unwrap()).unwrap(), 1);

    // A drain the next hour still sees the row parked (not retried within a day).
    let r = drain_ready(
        &worker,
        &pool,
        ScrobbleService::ListenBrainz,
        &fake,
        now + 3600,
        50,
    )
    .await
    .unwrap();
    assert_eq!(r.parked, 0);
    assert_eq!(r.retried, 0);
}

/// A listen queued for a different service is not submitted by this service's
/// drain (the snapshotted `service` routes it), and an empty-for-us pass is a
/// clean no-op.
#[tokio::test]
async fn drain_ignores_other_services_and_empty_is_noop() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    // Empty outbox: a drain does nothing.
    let fake = FakeSubmitter::new(Mode::Online);
    let r = drain_ready(
        &worker,
        &pool,
        ScrobbleService::ListenBrainz,
        &fake,
        5000,
        50,
    )
    .await
    .unwrap();
    assert_eq!(r, Default::default());

    // A Last.fm-bound listen is left untouched by a ListenBrainz drain.
    worker
        .enqueue_scrobble(listen("lastfm", "Green Calx", 1000), 1000)
        .await
        .unwrap();
    let r = drain_ready(
        &worker,
        &pool,
        ScrobbleService::ListenBrainz,
        &fake,
        5000,
        50,
    )
    .await
    .unwrap();
    assert_eq!(r, Default::default());
    assert_eq!(count_pending_scrobbles(&pool.open().unwrap()).unwrap(), 1);
}

/// The real ListenBrainz client speaks the wire protocol against a mock server:
/// a 200 submits, the auth header is sent, 5xx is transient, 4xx is permanent,
/// and validate-token reads the user name.
#[tokio::test]
async fn listenbrainz_client_speaks_the_protocol() {
    let server = MockServer::start().await;
    let sample = Listen {
        listened_at: 1000,
        artist: "Autechre".into(),
        track: "Rae".into(),
        album: None,
        track_number: None,
        duration_secs: None,
        recording_mbid: None,
    };

    // 200 + the Token auth header -> Ok.
    Mock::given(method("POST"))
        .and(path("/1/submit-listens"))
        .and(header("Authorization", "Token tok-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ok"})))
        .mount(&server)
        .await;
    let client = ListenBrainzClient::new("tok-123").with_base_url(server.uri());
    assert!(client.submit(&sample).await.is_ok());

    // 503 -> transient, 401 -> permanent (separate servers keep the mocks simple).
    let s503 = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&s503)
        .await;
    let c503 = ListenBrainzClient::new("t").with_base_url(s503.uri());
    assert!(matches!(
        c503.submit(&sample).await,
        Err(SubmitError::Transient(_))
    ));

    let s401 = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&s401)
        .await;
    let c401 = ListenBrainzClient::new("t").with_base_url(s401.uri());
    assert!(matches!(
        c401.submit(&sample).await,
        Err(SubmitError::Permanent(_))
    ));

    // validate-token -> the user name.
    let sval = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/1/validate-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"valid": true, "user_name": "brandon"})),
        )
        .mount(&sval)
        .await;
    let cval = ListenBrainzClient::new("t").with_base_url(sval.uri());
    assert_eq!(
        cval.validate_token().await.unwrap().as_deref(),
        Some("brandon")
    );
}

#[tokio::test]
async fn lastfm_client_scrobbles_and_classifies() {
    let sample = Listen {
        listened_at: 1000,
        artist: "Autechre".into(),
        track: "Rae".into(),
        album: None,
        track_number: None,
        duration_secs: None,
        recording_mbid: None,
    };

    // A signed track.scrobble POST that Last.fm accepts (HTTP 200, no error).
    let ok = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("method=track.scrobble"))
        .and(body_string_contains("api_sig="))
        .and(body_string_contains("sk=sess-1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"scrobbles": {"@attr": {"accepted": 1}}})),
        )
        .mount(&ok)
        .await;
    let client = LastfmClient::new("key-1", "secret-1", "sess-1").with_base_url(ok.uri());
    assert!(client.submit(&sample).await.is_ok());

    // HTTP 200 but an error body: 29 (rate limit) is transient, 9 (bad session)
    // is permanent. Last.fm signals failure in the body, not the status.
    let rl = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"error": 29, "message": "rate limit"})),
        )
        .mount(&rl)
        .await;
    let crl = LastfmClient::new("k", "s", "sk").with_base_url(rl.uri());
    assert!(matches!(
        crl.submit(&sample).await,
        Err(SubmitError::Transient(_))
    ));

    let bad = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"error": 9, "message": "invalid session key"})),
        )
        .mount(&bad)
        .await;
    let cbad = LastfmClient::new("k", "s", "sk").with_base_url(bad.uri());
    assert!(matches!(
        cbad.submit(&sample).await,
        Err(SubmitError::Permanent(_))
    ));
}

#[tokio::test]
async fn lastfm_connect_flow_exchanges_token_for_session() {
    // auth.getToken -> a request token, then auth.getSession(token) -> the
    // permanent session key + username. Both are signed GETs.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("method", "auth.getToken"))
        .and(query_param("api_key", "key-1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"token": "req-tok"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("method", "auth.getSession"))
        .and(query_param("token", "req-tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"session": {"name": "brandon", "key": "sess-xyz", "subscriber": 0}}),
        ))
        .mount(&server)
        .await;

    let client = LastfmClient::new("key-1", "secret-1", "").with_base_url(server.uri());
    let token = client.get_token().await.unwrap();
    assert_eq!(token, "req-tok");
    // The authorization URL carries the app key and the request token.
    let url = client.auth_url(&token);
    assert!(url.contains("api_key=key-1"));
    assert!(url.contains("token=req-tok"));
    let (session_key, name) = client.get_session(&token).await.unwrap();
    assert_eq!(session_key, "sess-xyz");
    assert_eq!(name, "brandon");
}

#[tokio::test]
async fn now_playing_pings_both_services() {
    let sample = Listen {
        listened_at: 1000,
        artist: "Aphex Twin".into(),
        track: "Xtal".into(),
        album: Some("Selected Ambient Works 85-92".into()),
        track_number: None,
        duration_secs: Some(300),
        recording_mbid: None,
    };

    // ListenBrainz: a signed POST to submit-listens with listen_type=playing_now.
    let lb = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/1/submit-listens"))
        .and(header("Authorization", "Token tok-1"))
        .and(body_string_contains("playing_now"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ok"})))
        .mount(&lb)
        .await;
    let lb_client = ListenBrainzClient::new("tok-1").with_base_url(lb.uri());
    assert!(lb_client.update_now_playing(&sample).await.is_ok());

    // Last.fm: a signed POST with method=track.updateNowPlaying and no timestamp.
    let lf = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("method=track.updateNowPlaying"))
        .and(body_string_contains("api_sig="))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"nowplaying": {}})),
        )
        .mount(&lf)
        .await;
    let lf_client = LastfmClient::new("key", "secret", "sess").with_base_url(lf.uri());
    assert!(lf_client.update_now_playing(&sample).await.is_ok());
}
