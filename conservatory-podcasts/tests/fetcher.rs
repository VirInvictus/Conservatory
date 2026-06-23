//! Phase 6a-ii-a: the conditional-GET fetcher against a local wiremock server.
//! Hermetic, no real network. Covers header extraction, the conditional-GET
//! request headers + 304 short-circuit, and the 429 / Retry-After cooldown.

use conservatory_podcasts::{FetchError, Fetcher};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn two_hundred_returns_body_and_validators() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<rss></rss>")
                .insert_header("ETag", "\"v1\"")
                .insert_header("Last-Modified", "Wed, 21 Oct 2026 07:28:00 GMT")
                .insert_header("Cache-Control", "public, max-age=600"),
        )
        .mount(&server)
        .await;

    let fetcher = Fetcher::new().unwrap();
    let res = fetcher
        .fetch(&format!("{}/feed.xml", server.uri()), None, None)
        .await
        .unwrap();

    assert_eq!(res.status, 200);
    assert_eq!(res.body, b"<rss></rss>");
    assert_eq!(res.etag.as_deref(), Some("\"v1\""));
    assert_eq!(
        res.last_modified.as_deref(),
        Some("Wed, 21 Oct 2026 07:28:00 GMT")
    );
    assert_eq!(res.cache_control_max_age, Some(600));
}

#[tokio::test]
async fn conditional_get_sends_validators_and_handles_304() {
    let server = MockServer::start().await;
    // The mock only matches if the conditional header is present, so a match
    // proves the fetcher sent it; the 304 proves the short-circuit. (The 200
    // test covers Last-Modified extraction; sending is symmetric.)
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .and(header("If-None-Match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;

    let fetcher = Fetcher::new().unwrap();
    let res = fetcher
        .fetch(
            &format!("{}/feed.xml", server.uri()),
            Some("\"v1\""),
            Some("Wed, 21 Oct 2026 07:28:00 GMT"),
        )
        .await
        .unwrap();

    assert_eq!(res.status, 304);
    assert!(res.body.is_empty());
    assert!(res.etag.is_none());
}

#[tokio::test]
async fn rate_limit_sets_a_cooldown_that_short_circuits_the_next_fetch() {
    let server = MockServer::start().await;
    // expect(1): only the first fetch may reach the server; the second must be
    // short-circuited by the cooldown. wiremock verifies the count on drop.
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .expect(1)
        .mount(&server)
        .await;

    let fetcher = Fetcher::new().unwrap();
    let url = format!("{}/feed.xml", server.uri());

    let first = fetcher.fetch(&url, None, None).await.unwrap_err();
    assert!(
        matches!(first, FetchError::RateLimited { retry_after_secs } if retry_after_secs == 3600),
        "expected RateLimited(3600), got {first:?}"
    );

    // Second call: the host is in cooldown, so this must not hit the network.
    let second = fetcher.fetch(&url, None, None).await.unwrap_err();
    assert!(matches!(second, FetchError::RateLimited { .. }));
}

#[tokio::test]
async fn rate_limit_without_retry_after_still_applies_a_default_cooldown() {
    let server = MockServer::start().await;
    // A 429 with no (numeric) Retry-After: the host must still be cooled down,
    // so the second fetch is short-circuited (expect(1) verifies one hit).
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(429))
        .expect(1)
        .mount(&server)
        .await;

    let fetcher = Fetcher::new().unwrap();
    let url = format!("{}/feed.xml", server.uri());

    let first = fetcher.fetch(&url, None, None).await.unwrap_err();
    assert!(
        matches!(first, FetchError::RateLimited { retry_after_secs } if retry_after_secs > 0),
        "a 429 without Retry-After should still report a positive cooldown, got {first:?}"
    );
    let second = fetcher.fetch(&url, None, None).await.unwrap_err();
    assert!(matches!(second, FetchError::RateLimited { .. }));
}

#[tokio::test]
async fn invalid_url_is_reported() {
    let fetcher = Fetcher::new().unwrap();
    let err = fetcher.fetch("not a url", None, None).await.unwrap_err();
    assert!(matches!(err, FetchError::InvalidUrl(_)), "got {err:?}");
}
