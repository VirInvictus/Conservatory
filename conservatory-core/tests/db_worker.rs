//! Phase 1a integration suite: the worker + read-pool round-trip that is the
//! sub-phase's usable artifact, exercised through the crate's public API.
//!
//! The migration-runner and writer-panic-restart tests live as in-crate unit
//! tests (they need crate-internal hooks); these cover the public path a real
//! consumer takes.

use conservatory_core::db::{ReadPool, probe_read, spawn_worker};
use tempfile::tempdir;

#[tokio::test]
async fn write_through_worker_reads_back_through_pool() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");

    let worker = spawn_worker(path.clone()).unwrap();
    worker
        .probe_write("artist", "Boards of Canada")
        .await
        .unwrap();

    let pool = ReadPool::new(path, 3).unwrap();
    let value = probe_read(&pool, "artist").unwrap();
    assert_eq!(value.as_deref(), Some("Boards of Canada"));

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn missing_key_reads_back_as_none() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");

    let worker = spawn_worker(path.clone()).unwrap();
    worker.probe_write("present", "yes").await.unwrap();

    let pool = ReadPool::new(path, 3).unwrap();
    assert_eq!(probe_read(&pool, "absent").unwrap(), None);

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn pool_connections_are_read_only() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");

    // The writer must create the file first, so the read-only open succeeds.
    let worker = spawn_worker(path.clone()).unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    let conn = pool.open().unwrap();
    // A write through a pool connection is rejected at the engine level.
    assert!(conn.execute("CREATE TABLE foo (id INTEGER)", []).is_err());

    worker.shutdown_ack().await.unwrap();
}
