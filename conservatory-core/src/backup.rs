//! Database backup and restore (spec §9, the data-safety contract).
//!
//! The database is canonical and owns the files, so the curated layer a
//! re-import cannot rebuild (ratings, play counts, podcast triage, Perspectives,
//! shelf-genre overrides; spec §5.6) lives only here. `backup` snapshots it
//! through the single-writer worker with SQLite's `VACUUM INTO`: the statement
//! runs on the writer's own connection inside the blocking task (the same
//! discipline as every other write), takes a consistent copy including
//! committed WAL content, and never touches the live file. `restore` is the
//! documented replace path: the stale `-wal` / `-shm` sidecars are removed and
//! the backup is copied into place through a same-directory temp file (fsync,
//! atomic rename; the mover's `write_atomic` discipline). The caller reopens
//! the database afterwards (`spawn_worker` runs the migrations), which also
//! proves the restored file opens cleanly.

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

    // Copy through a same-directory temp file first: the rename below is the
    // commit point, so a failure anywhere before it leaves the live database
    // untouched (with at most a stray temp file, removed on the error path).
    let temp = restore_temp_path(db)?;
    let result = (|| -> Result<()> {
        fs::copy(backup_file, &temp)?;
        let f = File::open(&temp)?;
        f.sync_all()?;
        drop(f);
        // The old write-ahead log and shared-memory file belong to the
        // database being replaced; left behind they would be applied to (or
        // corrupt) the restored file, so they go before the rename.
        for sidecar in [wal_path(db), shm_path(db)] {
            let _ = fs::remove_file(&sidecar);
        }
        fs::rename(&temp, db)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
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
        // A directory cannot be copied over: the copy step fails.
        let not_a_file = dir.path().join("dir.db");
        std::fs::create_dir(&not_a_file).unwrap();

        assert!(restore(&db, &not_a_file).is_err());
        assert_eq!(fs::read(&db).unwrap(), b"live body");
    }
}
