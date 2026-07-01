//! Schema migrations (spec §4, docs/schema.md).
//!
//! Versioned via the SQLite `user_version` PRAGMA, append-only and
//! backwards-compatible post-1.0. This is the **Atrium discipline, deliberately
//! not Viaduct's** `CREATE TABLE IF NOT EXISTS`: the library is the user's
//! irreplaceable data, so the schema history is an explicit numbered ledger,
//! not an idempotent best-effort.
//!
//! Phase 1b lands the first numbered migration (`0001`, the music schema and
//! FTS5 scaffolding). The runner owns `user_version`, so each `.sql` file is
//! pure DDL.

use rusqlite::Connection;

use crate::errors::Result;

/// One numbered migration step. `version` is the `user_version` the database
/// reaches once `sql` has applied.
pub(crate) struct Migration {
    pub version: i32,
    pub sql: &'static str,
}

/// The ordered migration ledger, ascending by `version`. Append-only and
/// backwards-compatible post-1.0 (the Atrium discipline).
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("migrations/0002_move_journal.sql"),
    },
    Migration {
        version: 3,
        sql: include_str!("migrations/0003_perspectives.sql"),
    },
    Migration {
        version: 4,
        sql: include_str!("migrations/0004_playback_state.sql"),
    },
    Migration {
        version: 5,
        sql: include_str!("migrations/0005_queue.sql"),
    },
    Migration {
        version: 6,
        sql: include_str!("migrations/0006_podcasts.sql"),
    },
    Migration {
        version: 7,
        sql: include_str!("migrations/0007_playback_cursor_kind.sql"),
    },
    Migration {
        version: 8,
        sql: include_str!("migrations/0008_eq.sql"),
    },
    Migration {
        version: 9,
        sql: include_str!("migrations/0009_audio_state.sql"),
    },
    Migration {
        version: 10,
        sql: include_str!("migrations/0010_eq_presets_builtin.sql"),
    },
    Migration {
        version: 11,
        sql: include_str!("migrations/0011_audiobooks.sql"),
    },
    Migration {
        version: 12,
        sql: include_str!("migrations/0012_move_book_ops.sql"),
    },
    Migration {
        version: 13,
        sql: include_str!("migrations/0013_book_playback_cursor.sql"),
    },
    Migration {
        version: 14,
        sql: include_str!("migrations/0014_verify_results.sql"),
    },
    Migration {
        version: 15,
        sql: include_str!("migrations/0015_ape_strips.sql"),
    },
    Migration {
        version: 16,
        sql: include_str!("migrations/0016_smart_speed_level.sql"),
    },
    Migration {
        version: 17,
        sql: include_str!("migrations/0017_playlists.sql"),
    },
];

/// The `user_version` a fully-migrated database reaches.
pub const CURRENT_VERSION: i32 = 16;

/// Apply any unapplied migrations. Idempotent: running this on a
/// fully-migrated database is a no-op.
pub(crate) fn run(conn: &mut Connection) -> Result<()> {
    apply(conn, MIGRATIONS)
}

/// Apply the steps in `migrations` whose `version` exceeds the database's
/// current `user_version`, in order, each in its own transaction. The version
/// bump rides inside the same transaction as the schema change, so a crash
/// mid-migration leaves `user_version` and schema in lockstep.
///
/// `migrations` must be sorted ascending by `version`.
fn apply(conn: &mut Connection, migrations: &[Migration]) -> Result<()> {
    debug_assert!(
        migrations.windows(2).all(|w| w[0].version < w[1].version),
        "migration registry must be sorted ascending by version with no duplicates",
    );

    let current: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    for migration in migrations.iter().filter(|m| m.version > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(migration.sql)?;
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection;
    use tempfile::tempdir;

    /// A synthetic two-step ledger that stands in for the real migrations the
    /// registry gains in Phase 1b, so the runner machinery is exercised before
    /// any real schema exists.
    const TEST_MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            sql: "CREATE TABLE one (id INTEGER PRIMARY KEY);",
        },
        Migration {
            version: 2,
            sql: "CREATE TABLE two (id INTEGER PRIMARY KEY);",
        },
    ];

    fn user_version(conn: &Connection) -> i32 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |_| Ok(()),
        )
        .is_ok()
    }

    fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            if row.get::<_, String>(1).unwrap() == column {
                return true;
            }
        }
        false
    }

    #[test]
    fn fresh_db_applies_all_steps() {
        let dir = tempdir().unwrap();
        let mut conn = connection::open_writer(&dir.path().join("test.db")).unwrap();

        apply(&mut conn, TEST_MIGRATIONS).unwrap();

        assert_eq!(user_version(&conn), 2);
        assert!(table_exists(&conn, "one"));
        assert!(table_exists(&conn, "two"));
    }

    #[test]
    fn migration_from_vn_skips_applied_steps() {
        let dir = tempdir().unwrap();
        let mut conn = connection::open_writer(&dir.path().join("test.db")).unwrap();

        // Pretend version 1 already ran (the vN fixture): only step 2 should apply.
        conn.pragma_update(None, "user_version", 1).unwrap();
        apply(&mut conn, TEST_MIGRATIONS).unwrap();

        assert_eq!(user_version(&conn), 2);
        // Step 1 was skipped, so its table was never created.
        assert!(!table_exists(&conn, "one"));
        assert!(table_exists(&conn, "two"));
    }

    #[test]
    fn idempotent_re_run() {
        let dir = tempdir().unwrap();
        let mut conn = connection::open_writer(&dir.path().join("test.db")).unwrap();

        apply(&mut conn, TEST_MIGRATIONS).unwrap();
        // A second pass must be a clean no-op, not a "table already exists" error.
        apply(&mut conn, TEST_MIGRATIONS).unwrap();

        assert_eq!(user_version(&conn), 2);
    }

    #[test]
    fn run_applies_the_real_ledger() {
        let dir = tempdir().unwrap();
        let mut conn = connection::open_writer(&dir.path().join("test.db")).unwrap();

        run(&mut conn).unwrap();
        assert_eq!(user_version(&conn), CURRENT_VERSION);

        // Spot-check the Phase 1b music schema, the Phase 2c move journal, and
        // the Phase 6a-i podcast tables landed.
        for t in [
            "artists",
            "albums",
            "tracks",
            "genres",
            "track_genres",
            "move_jobs",
            "move_operations",
            "perspectives",
            "playback_state",
            "queue",
            "shows",
            "episodes",
            "playback",
            "show_settings",
            "listening_sessions",
            "chapters",
            "tags",
            "show_tags",
            "eq_presets",
            "eq_state",
            "audio_state",
        ] {
            assert!(table_exists(&conn, t), "missing table: {t}");
        }

        // Migration 0007 adds the transport-cursor kind discriminator + the
        // episode reference to the singleton playback_state.
        assert!(column_exists(&conn, "playback_state", "kind"));
        assert!(column_exists(&conn, "playback_state", "episode_id"));
    }

    #[test]
    fn run_is_idempotent() {
        let dir = tempdir().unwrap();
        let mut conn = connection::open_writer(&dir.path().join("test.db")).unwrap();

        run(&mut conn).unwrap();
        run(&mut conn).unwrap(); // re-run must be a clean no-op, not "table already exists"
        assert_eq!(user_version(&conn), CURRENT_VERSION);
    }
}
