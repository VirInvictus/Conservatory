-- 0006_podcasts.sql — Phase 6a-i podcast schema (spec §4.2, §8, docs/schema.md).
--
-- The Belfry subsystem is absorbed (CLAUDE.md, spec §16.8). The eight podcast
-- tables are ported from belfry-core's `0001_initial.sql` verbatim, with one
-- deliberate change (spec §4.2): triage Queue state is expressed through the
-- unified `queue` table (§4.3), not a per-episode flag, so `playback` drops
-- Belfry's `in_queue` / `queue_position` columns and the index that paired
-- them. Inbox / Queue / Played is derived from `playback.played` plus
-- membership in `queue`.
--
-- `user_version` is bumped by the runner (db/migrations.rs); this file is pure
-- DDL. The schema is core-owned and applies in every build, plugin features on
-- or off (the §2.2 boundary rule): a music-only build simply has empty podcast
-- tables, so `user_version` never diverges between builds.

-- Subscriptions ----------------------------------------------------------
CREATE TABLE shows (
    id                INTEGER PRIMARY KEY,
    slug              TEXT UNIQUE NOT NULL,
    feed_url          TEXT UNIQUE NOT NULL,
    title             TEXT NOT NULL,
    author            TEXT,
    description       TEXT,
    homepage_url      TEXT,
    cover_path        TEXT,
    accent_rgb        INTEGER,                      -- packed RGB, median-cut from cover (spec §7.4)
    apple_podcasts_id TEXT,                         -- preserved on OPML round-trip (spec §8)
    last_fetched      INTEGER,                      -- unix epoch; conditional-GET bookkeeping
    last_modified     TEXT,                         -- HTTP If-Modified-Since
    etag              TEXT,                         -- HTTP If-None-Match
    fetch_interval    INTEGER NOT NULL DEFAULT 3600,
    auth_user         TEXT,                         -- HTTP Basic; NULL = anonymous
    auth_pass_ref     TEXT,                         -- libsecret schema name; never inline (oo7)
    auto_download     INTEGER NOT NULL DEFAULT 1,
    keep_count        INTEGER NOT NULL DEFAULT 0,   -- 0 = keep all
    priority          INTEGER NOT NULL DEFAULT 0,   -- Overcast-style ordering
    folder_path       TEXT NOT NULL                 -- managed; <root>/Podcasts/<slug> (spec §5.3)
);

CREATE TABLE episodes (
    id              INTEGER PRIMARY KEY,
    show_id         INTEGER NOT NULL REFERENCES shows(id) ON DELETE CASCADE,
    guid            TEXT NOT NULL,
    title           TEXT NOT NULL,
    description     TEXT,
    pub_date        INTEGER,
    duration        INTEGER,
    file_size       INTEGER,
    audio_url       TEXT,
    audio_path      TEXT,                           -- NULL until downloaded (spec §5.3)
    folder_path     TEXT NOT NULL,
    mime_type       TEXT,
    season          INTEGER,
    episode_number  INTEGER,
    episode_type    TEXT,                           -- full / trailer / bonus
    UNIQUE (show_id, guid)                          -- episode identity (spec §8)
);
CREATE INDEX idx_episodes_show_pub ON episodes(show_id, pub_date DESC);

-- Triage + playback state. Inbox / Queue / Played derived from `played` plus
-- unified-queue membership (spec §4.2). played: 0=unplayed, 1=in-progress,
-- 2=played-fully, 3=archived-unlistened.
CREATE TABLE playback (
    episode_id      INTEGER PRIMARY KEY REFERENCES episodes(id) ON DELETE CASCADE,
    position        REAL NOT NULL DEFAULT 0,
    played          INTEGER NOT NULL DEFAULT 0,
    last_played     INTEGER,
    play_count      INTEGER NOT NULL DEFAULT 0,
    starred         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_playback_inbox   ON playback(played) WHERE played = 0;
CREATE INDEX idx_playback_starred ON playback(starred) WHERE starred = 1;

-- Per-show overrides (Overcast pattern + Castro inbox policy).
CREATE TABLE show_settings (
    show_id         INTEGER PRIMARY KEY REFERENCES shows(id) ON DELETE CASCADE,
    playback_speed  REAL NOT NULL DEFAULT 1.0,
    smart_speed     INTEGER NOT NULL DEFAULT 1,
    voice_boost     INTEGER NOT NULL DEFAULT 0,
    skip_intro      INTEGER NOT NULL DEFAULT 0,     -- seconds shaved from start
    skip_outro      INTEGER NOT NULL DEFAULT 0,
    skip_forward    INTEGER,                        -- NULL = inherit global
    skip_back       INTEGER,                        -- NULL = inherit global
    inbox_policy    TEXT NOT NULL DEFAULT 'inbox'   -- 'inbox' | 'always_queue' | 'always_archive'
);

-- One row per playback session — drives history + Smart Speed time-saved
-- accounting (spec §6.3). Append-only.
CREATE TABLE listening_sessions (
    id                INTEGER PRIMARY KEY,
    episode_id        INTEGER NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    started_at        INTEGER NOT NULL,
    ended_at          INTEGER NOT NULL,
    real_seconds      REAL NOT NULL,                -- wall-clock listen time
    audio_seconds     REAL NOT NULL,                -- audio time covered
    smart_speed_saved REAL NOT NULL DEFAULT 0
);
CREATE INDEX idx_sessions_episode ON listening_sessions(episode_id);
CREATE INDEX idx_sessions_started ON listening_sessions(started_at);

-- Chapters (podcast:chapters JSON or ID3 CHAP; three-source precedence, spec §8).
CREATE TABLE chapters (
    id          INTEGER PRIMARY KEY,
    episode_id  INTEGER NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    start_time  REAL NOT NULL,
    end_time    REAL,
    title       TEXT,
    url         TEXT,
    image_path  TEXT
);
CREATE INDEX idx_chapters_episode ON chapters(episode_id, start_time);

-- Tags on shows (not episodes). Calibre loanword; secondary organization,
-- preserved on OPML round-trip (spec §8).
CREATE TABLE tags (
    id    INTEGER PRIMARY KEY,
    name  TEXT UNIQUE NOT NULL
);
CREATE TABLE show_tags (
    show_id  INTEGER REFERENCES shows(id) ON DELETE CASCADE,
    tag_id   INTEGER REFERENCES tags(id)  ON DELETE CASCADE,
    PRIMARY KEY (show_id, tag_id)
);

-- FTS5 (spec §4.4). Ordinary (not external-content) tables, matching the music
-- FTS in 0001: title + description/author live on the source row, so the
-- triggers copy them straight across (no cross-table denormalization). NOT
-- transcripts (spec §14).
CREATE VIRTUAL TABLE episode_fts USING fts5(title, description);
CREATE VIRTUAL TABLE show_fts    USING fts5(title, author, description);

-- episode_fts sync on the episodes table.
CREATE TRIGGER episodes_ai AFTER INSERT ON episodes BEGIN
    INSERT INTO episode_fts(rowid, title, description)
        VALUES (new.id, new.title, COALESCE(new.description, ''));
END;
CREATE TRIGGER episodes_ad AFTER DELETE ON episodes BEGIN
    DELETE FROM episode_fts WHERE rowid = old.id;
END;
CREATE TRIGGER episodes_au AFTER UPDATE ON episodes BEGIN
    DELETE FROM episode_fts WHERE rowid = old.id;
    INSERT INTO episode_fts(rowid, title, description)
        VALUES (new.id, new.title, COALESCE(new.description, ''));
END;

-- show_fts sync on the shows table.
CREATE TRIGGER shows_ai AFTER INSERT ON shows BEGIN
    INSERT INTO show_fts(rowid, title, author, description)
        VALUES (new.id, new.title, COALESCE(new.author, ''), COALESCE(new.description, ''));
END;
CREATE TRIGGER shows_ad AFTER DELETE ON shows BEGIN
    DELETE FROM show_fts WHERE rowid = old.id;
END;
CREATE TRIGGER shows_au AFTER UPDATE ON shows BEGIN
    DELETE FROM show_fts WHERE rowid = old.id;
    INSERT INTO show_fts(rowid, title, author, description)
        VALUES (new.id, new.title, COALESCE(new.author, ''), COALESCE(new.description, ''));
END;

-- Rebuild `queue` to add the deferred `episode_id` foreign key now that
-- `episodes` exists. At migration 0005 the FK was parked because `foreign_keys
-- = ON` refuses any DML on a child whose parent table is absent; the column was
-- plain until now. `book_id` stays plain until `books` lands (Phase 7), when it
-- gets the same treatment. Nothing references `queue`, so the drop/rename is
-- safe inside the migration transaction; the saved playback queue is copied
-- across. The CHECK and the position index are recreated unchanged.
CREATE TABLE queue_new (
    id          INTEGER PRIMARY KEY,
    position    INTEGER NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('track','episode','audiobook')),
    track_id    INTEGER REFERENCES tracks(id)   ON DELETE CASCADE,
    episode_id  INTEGER REFERENCES episodes(id) ON DELETE CASCADE,
    book_id     INTEGER,
    CHECK ( (kind='track'     AND track_id   IS NOT NULL AND episode_id IS NULL AND book_id IS NULL)
         OR (kind='episode'   AND episode_id IS NOT NULL AND track_id   IS NULL AND book_id IS NULL)
         OR (kind='audiobook' AND book_id    IS NOT NULL AND track_id   IS NULL AND episode_id IS NULL) )
);
INSERT INTO queue_new (id, position, kind, track_id, episode_id, book_id)
    SELECT id, position, kind, track_id, episode_id, book_id FROM queue;
DROP TABLE queue;
ALTER TABLE queue_new RENAME TO queue;
CREATE INDEX idx_queue_position ON queue(position);
