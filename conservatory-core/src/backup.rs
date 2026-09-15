//! Database backup and restore (spec §9, the data-safety contract).
//!
//! The database is canonical and owns the files, so the curated layer a
//! re-import cannot rebuild (ratings, play counts, podcast triage, Perspectives,
//! shelf-genre overrides; spec §5.6) lives only here. `backup` snapshots it
//! through the single-writer worker with SQLite's `VACUUM INTO`: the statement
//! runs on the writer's own connection inside the blocking task (the same
//! discipline as every other write), takes a consistent copy including
//! committed WAL content, and never touches the live file. `restore` is the
//! documented replace path: the live database's write-ahead log is
//! checkpointed first (SQLite's own crash-safe path, folding WAL-resident
//! commits into the main file), then the backup is copied into place through
//! a same-directory temp file (fsync, atomic rename; the mover's
//! `write_atomic` discipline), and the emptied sidecars are swept after the
//! rename. Every intermediate state is safe: a crash before the rename loses
//! nothing, and a crash after it leaves only an inert empty `-wal` beside the
//! restored file, which the next open re-initializes instead of replaying.
//! The caller reopens the database afterwards (`spawn_worker` runs the
//! migrations), which also proves the restored file opens cleanly.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::db::WorkerHandle;
use crate::errors::{Error, Result};

/// The 16-byte header every SQLite 3 database starts with.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// Snapshot the live database to `out` via `VACUUM INTO`, executed through the
/// worker. Refuses an existing target so a backup is never silently
/// overwritten (the engine refuses too; this is the readable front line).
pub async fn backup(worker: &WorkerHandle, out: &Path) -> Result<()> {
    if out.exists() {
        return Err(Error::Backup(format!(
            "backup target {} already exists",
            out.display()
        )));
    }
    worker.vacuum_into(out.to_string_lossy().into_owned()).await
}

/// Replace the database at `db` with `backup_file`. File-level only: no worker
/// may be open against `db` (the CLI restores before spawning one), and the
/// caller should reopen afterwards so migrations run on the restored file.
pub fn restore(db: &Path, backup_file: &Path) -> Result<()> {
    if db == backup_file {
        return Err(Error::Backup(
            "restore source and target are the same file".into(),
        ));
    }
    let mut header = [0u8; 16];
    let mut f = File::open(backup_file)
        .map_err(|e| Error::Backup(format!("cannot open backup {}: {e}", backup_file.display())))?;
    f.read_exact(&mut header)
        .map_err(|e| Error::Backup(format!("cannot read backup {}: {e}", backup_file.display())))?;
    if header != *SQLITE_HEADER {
        return Err(Error::Backup(format!(
            "{} is not a SQLite database",
            backup_file.display()
        )));
    }

    // The pre-commit-point durability step: fold WAL-resident commits into
    // the live file before anything is touched. The sidecars used to be
    // removed here, before the rename, and a crash in that window silently
    // discarded non-checkpointed commits (routine under synchronous=NORMAL).
    if db.exists() {
        checkpoint_live_wal(db)?;
    }

    // Copy through a same-directory temp file first: the rename below is the
    // commit point, so a failure anywhere before it leaves the live database
    // untouched (with at most a stray temp file, removed on the error path).
    let temp = restore_temp_path(db)?;
    let result = (|| -> Result<()> {
        fs::copy(backup_file, &temp)?;
        let f = File::open(&temp)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&temp, db)?;
        // The stale `-wal` / `-shm` sidecars belong to the replaced database
        // and are swept after the commit point: the checkpoint above emptied
        // the WAL, so sidecars a crash leaves in this window are inert (an
        // empty `-wal` beside a fresh file is re-initialized, never replayed).
        for sidecar in [wal_path(db), shm_path(db)] {
            let _ = fs::remove_file(&sidecar);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Checkpoint (and truncate) the live database's write-ahead log, so every
/// committed transaction is durable in the main file before the rename.
/// Refuses while another connection holds the database busy, rather than
/// proceeding with commits still outside the main file.
fn checkpoint_live_wal(db: &Path) -> Result<()> {
    let conn = Connection::open(db).map_err(|e| {
        Error::Backup(format!(
            "cannot open {} for checkpointing: {e}",
            db.display()
        ))
    })?;
    let busy: i64 = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
        .map_err(|e| Error::Backup(format!("cannot checkpoint {}: {e}", db.display())))?;
    if busy != 0 {
        return Err(Error::Backup(
            "the live database is busy; close other connections before restoring".into(),
        ));
    }
    Ok(())
}

/// The `VACUUM INTO` half of [`backup`], on the writer connection. Internal:
/// reached only through the worker (`Command::VacuumInto`).
pub(crate) fn vacuum_into(conn: &Connection, out: &str) -> Result<()> {
    conn.execute("VACUUM INTO ?1", [out])?;
    Ok(())
}

fn restore_temp_path(db: &Path) -> Result<PathBuf> {
    let name = db
        .file_name()
        .ok_or_else(|| Error::Backup(format!("{} has no file name", db.display())))?;
    let mut temp = name.to_os_string();
    temp.push(".conservatory-restore");
    Ok(db.with_file_name(temp))
}

fn wal_path(db: &Path) -> PathBuf {
    sidecar(db, "-wal")
}

fn shm_path(db: &Path) -> PathBuf {
    sidecar(db, "-shm")
}

fn sidecar(db: &Path, suffix: &str) -> PathBuf {
    let mut s = db.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn restore_refuses_a_same_path_pair() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("library.db");
        assert!(restore(&db, &db).is_err());
    }

    #[test]
    fn restore_refuses_a_missing_or_non_sqlite_backup() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("library.db");
        let missing = dir.path().join("nope.db");
        assert!(restore(&db, &missing).is_err());

        let junk = dir.path().join("junk.db");
        std::fs::write(&junk, b"not a database at all").unwrap();
        assert!(restore(&db, &junk).is_err());
    }

    #[test]
    fn restore_replaces_the_file_and_clears_the_sidecars() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("library.db");
        let backup_file = dir.path().join("backup.db");
        std::fs::write(&backup_file, b"SQLite format 3\0restored-body").unwrap();
        std::fs::write(wal_path(&db), b"stale wal").unwrap();
        std::fs::write(shm_path(&db), b"stale shm").unwrap();

        restore(&db, &backup_file).unwrap();

        assert_eq!(fs::read(&db).unwrap(), b"SQLite format 3\0restored-body");
        assert!(!wal_path(&db).exists());
        assert!(!shm_path(&db).exists());
        assert!(
            !dir.path().join("library.db.conservatory-restore").exists(),
            "no temp file left behind"
        );
    }

    #[test]
    fn a_failed_restore_leaves_the_live_file_alone() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("library.db");
        std::fs::write(&db, b"live body").unwrap();
        let backup_file = dir.path().join("backup.db");
        std::fs::write(&backup_file, b"SQLite format 3\0restored-body").unwrap();

        // A live file that is not a database fails at the checkpoint step.
        assert!(restore(&db, &backup_file).is_err());
        assert_eq!(fs::read(&db).unwrap(), b"live body");

        // A directory cannot be read as a backup: the header step fails.
        let not_a_file = dir.path().join("dir.db");
        std::fs::create_dir(&not_a_file).unwrap();
        assert!(restore(&db, &not_a_file).is_err());
        assert_eq!(fs::read(&db).unwrap(), b"live body");
    }

    #[test]
    fn the_checkpoint_folds_wal_resident_commits_into_the_main_file() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("library.db");
        let live = Connection::open(&db).unwrap();
        live.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE t(v TEXT);
             INSERT INTO t VALUES ('precious');",
        )
        .unwrap();
        // Keep a second connection attached (a plain read, so no read
        // transaction blocks the checkpoint below) so the writer's close
        // cannot run the checkpoint-and-remove for us: the commit must stay
        // WAL-resident.
        let keeper = Connection::open(&db).unwrap();
        keeper.execute_batch("SELECT count(*) FROM t;").unwrap();
        drop(live);
        assert!(
            wal_path(&db).metadata().unwrap().len() > 0,
            "the commit is WAL-resident"
        );

        checkpoint_live_wal(&db).unwrap();

        // The crash-window property: the main file alone now carries every
        // committed transaction, no sidecar required.
        let alone = dir.path().join("alone.db");
        fs::copy(&db, &alone).unwrap();
        let v: String = Connection::open(&alone)
            .unwrap()
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "precious");
        drop(keeper);
    }

    #[test]
    fn restore_refuses_while_a_reader_holds_the_live_database() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("library.db");
        let backup_file = dir.path().join("backup.db");
        let live = Connection::open(&db).unwrap();
        live.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE t(v TEXT);
             INSERT INTO t VALUES ('live');",
        )
        .unwrap();
        let backup = Connection::open(&backup_file).unwrap();
        backup
            .execute_batch("CREATE TABLE t(v TEXT); INSERT INTO t VALUES ('backup');")
            .unwrap();

        // An open read transaction keeps a checkpoint from completing: the
        // restore must refuse rather than replace a database with commits
        // still outside its main file.
        let reader = Connection::open(&db).unwrap();
        reader
            .execute_batch(
                "BEGIN;
             SELECT count(*) FROM t;",
            )
            .unwrap();

        let err = restore(&db, &backup_file).unwrap_err();
        assert!(err.to_string().contains("busy"), "got: {err}");
        let v: String = reader
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "live", "the live database is untouched by the refusal");
    }

    #[test]
    fn the_post_rename_crash_window_is_inert() {
        let dir = tempdir().unwrap();

        // The restored database: a WAL-mode file (as every VACUUM INTO
        // snapshot of this library is), single file after its close.
        let restored_src = dir.path().join("restored.db");
        let restored = Connection::open(&restored_src).unwrap();
        restored
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE t(v TEXT);
                 INSERT INTO t VALUES ('fresh');",
            )
            .unwrap();
        drop(restored);

        // An old database with a committed frame stranded in its WAL, kept
        // WAL-resident by a second attached connection (one plain read, so
        // the sidecars survive the writer's close).
        let old_src = dir.path().join("old.db");
        let old = Connection::open(&old_src).unwrap();
        old.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE t(v TEXT);
             INSERT INTO t VALUES ('stale');",
        )
        .unwrap();
        let keeper = Connection::open(&old_src).unwrap();
        keeper.execute_batch("SELECT count(*) FROM t;").unwrap();
        drop(old);
        let stale_shm = fs::read(shm_path(&old_src)).unwrap();
        assert!(wal_path(&old_src).metadata().unwrap().len() > 0);
        drop(keeper);

        // Assemble the crash window the new ordering can leave behind: the
        // rename has installed the restored file, the checkpoint emptied the
        // WAL, and the sweep has not run (0-byte -wal plus the old -shm).
        let db = dir.path().join("library.db");
        fs::copy(&restored_src, &db).unwrap();
        fs::write(wal_path(&db), b"").unwrap();
        fs::write(shm_path(&db), &stale_shm).unwrap();

        let conn = Connection::open(&db).unwrap();
        let v: String = conn.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "fresh", "the stale WAL must not replay over the restore");
        let ok: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ok, "ok");
    }
}
