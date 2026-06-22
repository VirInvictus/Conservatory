//! Phase 6a-iii-b: episode download against a local wiremock server and a real
//! core worker (temp DB). Covers the happy path (file written, `audio_path`
//! recorded) and a Basic-auth-gated feed (401 without the credential, 200 with
//! it, the password flowing through the in-memory credential store).

use std::path::Path;

use conservatory_core::db::{Episode, ReadPool, Show, WorkerHandle, get_episode, spawn_worker};
use conservatory_podcasts::{CredentialStore, Fetcher, download_episode};
use tempfile::tempdir;
use wiremock::matchers::{basic_auth, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const AUDIO: &str = "ID3 fake-but-deterministic audio bytes";

fn fresh() -> (tempfile::TempDir, WorkerHandle, ReadPool) {
    let dir = tempdir().unwrap();
    let db = dir.path().join("t.db");
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db, 3).unwrap();
    (dir, worker, pool)
}

fn sample_show(feed_url: &str, auth_user: Option<&str>, auth_pass_ref: Option<&str>) -> Show {
    Show {
        id: 0,
        slug: "sample".to_string(),
        feed_url: feed_url.to_string(),
        title: "Sample".to_string(),
        author: None,
        description: None,
        homepage_url: None,
        cover_path: None,
        accent_rgb: None,
        apple_podcasts_id: None,
        last_fetched: None,
        last_modified: None,
        etag: None,
        fetch_interval: 3600,
        auth_user: auth_user.map(str::to_string),
        auth_pass_ref: auth_pass_ref.map(str::to_string),
        auto_download: true,
        keep_count: 0,
        priority: 0,
        folder_path: "Podcasts/sample".to_string(),
    }
}

fn sample_episode(show_id: i64, audio_url: &str) -> Episode {
    Episode {
        id: 0,
        show_id,
        guid: "ep-guid".to_string(),
        title: "Episode One".to_string(),
        description: None,
        pub_date: None,
        duration: None,
        file_size: None,
        audio_url: Some(audio_url.to_string()),
        audio_path: None,
        folder_path: "Podcasts/sample/2024-01-01--episode-one".to_string(),
        mime_type: Some("audio/mpeg".to_string()),
        season: None,
        episode_number: None,
        episode_type: None,
    }
}

async fn seed_episode(worker: &WorkerHandle, pool: &ReadPool, episode: Episode) -> Episode {
    let id = worker.upsert_episode(episode).await.unwrap();
    let conn = pool.open().unwrap();
    get_episode(&conn, id).unwrap().unwrap()
}

#[tokio::test]
async fn download_writes_file_and_sets_audio_path() {
    let (dir, worker, pool) = fresh();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ep.mp3"))
        .respond_with(ResponseTemplate::new(200).set_body_string(AUDIO))
        .mount(&server)
        .await;

    let show_id = worker
        .get_or_create_show(sample_show(
            &format!("{}/feed.xml", server.uri()),
            None,
            None,
        ))
        .await
        .unwrap();
    let audio_url = format!("{}/ep.mp3", server.uri());
    let episode = seed_episode(&worker, &pool, sample_episode(show_id, &audio_url)).await;

    let root = dir.path().join("library");
    let client = Fetcher::new().unwrap().client();
    let dst = download_episode(&client, &worker, &root, &episode, None)
        .await
        .unwrap();

    // The file landed at <root>/<folder_path>/ep.mp3 with the served bytes.
    let expected = root.join("Podcasts/sample/2024-01-01--episode-one/ep.mp3");
    assert_eq!(dst, expected);
    assert_eq!(std::fs::read_to_string(&expected).unwrap(), AUDIO);
    assert!(!Path::new(&format!("{}.part", expected.display())).exists());

    // audio_path is recorded, relative to the root.
    let conn = pool.open().unwrap();
    let stored = get_episode(&conn, episode.id).unwrap().unwrap();
    assert_eq!(
        stored.audio_path.as_deref(),
        Some("Podcasts/sample/2024-01-01--episode-one/ep.mp3")
    );

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn basic_auth_gates_the_download() {
    let (dir, worker, pool) = fresh();
    let server = MockServer::start().await;
    // Only an authenticated request gets the audio; everything else is a 401.
    Mock::given(method("GET"))
        .and(path("/ep.mp3"))
        .and(basic_auth("alice", "s3cret"))
        .respond_with(ResponseTemplate::new(200).set_body_string(AUDIO))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/ep.mp3"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let show_id = worker
        .get_or_create_show(sample_show(
            &format!("{}/feed.xml", server.uri()),
            Some("alice"),
            Some("sample-ref"),
        ))
        .await
        .unwrap();
    let audio_url = format!("{}/ep.mp3", server.uri());
    let episode = seed_episode(&worker, &pool, sample_episode(show_id, &audio_url)).await;

    let root = dir.path().join("library");
    let client = Fetcher::new().unwrap().client();

    // Without credentials: 401 -> error, no file, audio_path stays None.
    assert!(
        download_episode(&client, &worker, &root, &episode, None)
            .await
            .is_err()
    );
    {
        let conn = pool.open().unwrap();
        assert_eq!(
            get_episode(&conn, episode.id).unwrap().unwrap().audio_path,
            None
        );
    }

    // With the stored credential resolved through the in-memory store: success.
    let store = CredentialStore::in_memory();
    store.set("sample-ref", "s3cret").await.unwrap();
    let auth = store
        .resolve(Some("alice"), Some("sample-ref"))
        .await
        .unwrap();
    assert!(auth.is_some());

    download_episode(&client, &worker, &root, &episode, auth.as_ref())
        .await
        .unwrap();
    let conn = pool.open().unwrap();
    assert!(
        get_episode(&conn, episode.id)
            .unwrap()
            .unwrap()
            .audio_path
            .is_some()
    );

    worker.shutdown_ack().await.unwrap();
}
