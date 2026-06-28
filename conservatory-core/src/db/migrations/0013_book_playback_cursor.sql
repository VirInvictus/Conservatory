-- 0013_book_playback_cursor.sql — Phase 7c-ii (spec §6.4, §6.3, docs/schema.md).
--
-- Audiobook playback joins the unified queue. Two changes, both additive in
-- spirit:
--
-- 1. The transport cursor (`playback_state`, migration 0004/0007) learns books.
--    It already discriminates by `kind` with a `track_id` / `episode_id`; add a
--    `book_id` for `kind = 'audiobook'`, so a restart reopens the book that was
--    playing and seeks it to its absolute resume offset (`book_playback.position`,
--    migration 0011). The FK is added by ALTER because its default is NULL
--    (SQLite forbids ADD COLUMN with a non-NULL default referencing another
--    table; NULL is fine).
--
-- 2. `listening_sessions` (migration 0006) becomes media-agnostic. It was
--    episode-only (`episode_id NOT NULL`); audiobooks share the spoken-word
--    profile *including* the time-saved accounting (spec §6.3), so a session row
--    must be able to belong to a book instead. SQLite cannot drop a column's
--    NOT NULL in place, so the table is rebuilt: `episode_id` becomes nullable, a
--    nullable `book_id` is added, and a CHECK enforces that exactly one of the
--    two is set. The table is append-only history, so the rebuild just copies the
--    existing episode rows forward (book_id NULL). `listening_totals` aggregates
--    every row regardless of which id it carries, so it needs no change.
--
-- `user_version` is bumped by the runner (db/migrations.rs); this file is pure
-- DDL.

ALTER TABLE playback_state ADD COLUMN book_id INTEGER REFERENCES books(id) ON DELETE SET NULL;

CREATE TABLE listening_sessions_new (
    id                INTEGER PRIMARY KEY,
    episode_id        INTEGER REFERENCES episodes(id) ON DELETE CASCADE,
    book_id           INTEGER REFERENCES books(id) ON DELETE CASCADE,
    started_at        INTEGER NOT NULL,
    ended_at          INTEGER NOT NULL,
    real_seconds      REAL NOT NULL,                -- wall-clock listen time
    audio_seconds     REAL NOT NULL,                -- audio time covered
    smart_speed_saved REAL NOT NULL DEFAULT 0,
    -- Exactly one owner: episode XOR book (each `IS NULL` is 0/1, so the two
    -- nulls must sum to exactly one).
    CHECK (((episode_id IS NULL) + (book_id IS NULL)) = 1)
);

INSERT INTO listening_sessions_new
    (id, episode_id, book_id, started_at, ended_at, real_seconds, audio_seconds, smart_speed_saved)
SELECT id, episode_id, NULL, started_at, ended_at, real_seconds, audio_seconds, smart_speed_saved
FROM listening_sessions;

DROP TABLE listening_sessions;
ALTER TABLE listening_sessions_new RENAME TO listening_sessions;

CREATE INDEX idx_sessions_episode ON listening_sessions(episode_id);
CREATE INDEX idx_sessions_book ON listening_sessions(book_id);
CREATE INDEX idx_sessions_started ON listening_sessions(started_at);
