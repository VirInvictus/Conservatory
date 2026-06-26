-- 0011_audiobooks.sql — Phase 7a-i audiobook schema (spec §4.5, §4.4, docs/schema.md).
--
-- The third media type (spec §3.8), landing in the conservatory-audiobooks
-- plugin crate. The schema, however, is core-owned: the plugin boundary is code
-- and dependencies, not the database (spec §2.2), so these tables live in the
-- single append-only ledger and apply in every build. A music-only build simply
-- has empty book tables, so `user_version` never diverges between builds (the
-- 0006 invariant).
--
-- Modeled on Audiobookshelf's relational shape and Cozy's Book → Chapter → file
-- model. A *book* is the unit; *chapters* are ordered and come from either
-- embedded M4B markers or one-file-per-chapter folders; *authors* and
-- *narrators* are distinct roles (many-to-many); *series* carries a decimal
-- sequence. Resume is a single row per book (the podcast `playback` analogue,
-- never lost). Format/bitrate/sample-rate are read per chapter file at import
-- and total duration is derived by summing chapter durations, so neither is
-- persisted here (spec §4.5).
--
-- `user_version` is bumped by the runner (db/migrations.rs); this file is pure
-- DDL.

-- People (authors and narrators share a table, role-tagged via the link tables).
CREATE TABLE book_people (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    sort_name   TEXT NOT NULL,          -- "Sanderson, Brandon"; drives path + sort
    UNIQUE (sort_name)
);

CREATE TABLE series (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    UNIQUE (name)
);

CREATE TABLE books (
    id              INTEGER PRIMARY KEY,
    title           TEXT NOT NULL,
    subtitle        TEXT,
    series_id       INTEGER REFERENCES series(id),
    series_sequence REAL,               -- decimal: "Book 1.5"
    year            INTEGER,
    publisher       TEXT,
    isbn            TEXT,
    asin            TEXT,
    description     TEXT,
    language        TEXT,
    shelf_genre     TEXT,               -- same decoupling as music (§5.2); single-valued path input
    cover_path      TEXT,
    accent_rgb      INTEGER,            -- packed RGB, median-cut from cover (§7.4)
    folder_path     TEXT NOT NULL,      -- managed; rendered from the audiobook template (§5.7)
    rating          INTEGER DEFAULT 0,
    starred         INTEGER DEFAULT 0,
    added_at        INTEGER
);
CREATE INDEX idx_books_series ON books(series_id);

-- Author / narrator links (role-tagged many-to-many).
CREATE TABLE book_authors (
    book_id   INTEGER REFERENCES books(id)        ON DELETE CASCADE,
    person_id INTEGER REFERENCES book_people(id)  ON DELETE CASCADE,
    PRIMARY KEY (book_id, person_id)
);
CREATE TABLE book_narrators (
    book_id   INTEGER REFERENCES books(id)        ON DELETE CASCADE,
    person_id INTEGER REFERENCES book_people(id)  ON DELETE CASCADE,
    PRIMARY KEY (book_id, person_id)
);

-- Ordered chapters. `file_path` + `file_offset` lets one row address either a
-- standalone per-chapter file (offset 0) or a span inside a single M4B.
CREATE TABLE book_chapters (
    id          INTEGER PRIMARY KEY,
    book_id     INTEGER REFERENCES books(id) ON DELETE CASCADE,
    idx         INTEGER NOT NULL,       -- 0-based order within the book
    title       TEXT,
    file_path   TEXT NOT NULL,          -- managed; under the book folder
    file_offset REAL NOT NULL DEFAULT 0,-- seconds into file_path where this chapter starts
    duration    REAL,                   -- seconds
    UNIQUE (book_id, idx)
);
CREATE INDEX idx_book_chapters_book ON book_chapters(book_id, idx);

-- First-class resume (§6.4). One row per book; never append-only (a book is one
-- thing you resume). Per-book speed/smart_speed/voice_boost overrides default to
-- NULL, meaning inherit the global default.
CREATE TABLE book_playback (
    book_id        INTEGER PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    position       REAL NOT NULL DEFAULT 0,  -- absolute seconds across the whole book
    finished       INTEGER NOT NULL DEFAULT 0,
    last_played    INTEGER,
    speed          REAL,                     -- per-book override; NULL = global default
    smart_speed    INTEGER,                  -- per-book override; NULL = global default
    voice_boost    INTEGER                   -- per-book override; NULL = global default
);

-- FTS5 (spec §4.4). Ordinary (not external-content) table, matching the music
-- and podcast FTS. `title` and `series` denormalize from the `books` row (the
-- series name via a lookup); `author` and `narrator` denormalize from the
-- role-tagged link tables, so unlike the single-table episode_fts the index is
-- maintained from triggers on `book_authors` / `book_narrators` as well, and a
-- person or series rename propagates back in (the 0001 artists_au precedent).
-- Each aggregated column is a space-joined group_concat so FTS tokenizes the
-- individual names.
CREATE VIRTUAL TABLE book_fts USING fts5(title, author, narrator, series);

-- A new book seeds title + series; author/narrator fill in as links are added.
CREATE TRIGGER books_ai AFTER INSERT ON books BEGIN
    INSERT INTO book_fts(rowid, title, author, narrator, series) VALUES (
        new.id,
        new.title,
        '',
        '',
        COALESCE((SELECT name FROM series WHERE id = new.series_id), '')
    );
END;
CREATE TRIGGER books_ad AFTER DELETE ON books BEGIN
    DELETE FROM book_fts WHERE rowid = old.id;
END;
-- Title or series change; author/narrator are link-driven and left untouched.
CREATE TRIGGER books_au AFTER UPDATE ON books BEGIN
    UPDATE book_fts SET
        title  = new.title,
        series = COALESCE((SELECT name FROM series WHERE id = new.series_id), '')
    WHERE rowid = new.id;
END;

-- Re-aggregate the denormalized `author` column whenever a book's author set
-- changes. group_concat over the role link keeps every credited author searchable.
CREATE TRIGGER book_authors_ai AFTER INSERT ON book_authors BEGIN
    UPDATE book_fts SET author = COALESCE((
        SELECT group_concat(p.name, ' ') FROM book_people p
            JOIN book_authors ba ON ba.person_id = p.id
            WHERE ba.book_id = new.book_id
    ), '') WHERE rowid = new.book_id;
END;
CREATE TRIGGER book_authors_ad AFTER DELETE ON book_authors BEGIN
    UPDATE book_fts SET author = COALESCE((
        SELECT group_concat(p.name, ' ') FROM book_people p
            JOIN book_authors ba ON ba.person_id = p.id
            WHERE ba.book_id = old.book_id
    ), '') WHERE rowid = old.book_id;
END;

CREATE TRIGGER book_narrators_ai AFTER INSERT ON book_narrators BEGIN
    UPDATE book_fts SET narrator = COALESCE((
        SELECT group_concat(p.name, ' ') FROM book_people p
            JOIN book_narrators bn ON bn.person_id = p.id
            WHERE bn.book_id = new.book_id
    ), '') WHERE rowid = new.book_id;
END;
CREATE TRIGGER book_narrators_ad AFTER DELETE ON book_narrators BEGIN
    UPDATE book_fts SET narrator = COALESCE((
        SELECT group_concat(p.name, ' ') FROM book_people p
            JOIN book_narrators bn ON bn.person_id = p.id
            WHERE bn.book_id = old.book_id
    ), '') WHERE rowid = old.book_id;
END;

-- A renamed person propagates into the author and narrator columns of every
-- book they are credited on.
CREATE TRIGGER book_people_au AFTER UPDATE OF name ON book_people BEGIN
    UPDATE book_fts SET author = COALESCE((
        SELECT group_concat(p.name, ' ') FROM book_people p
            JOIN book_authors ba ON ba.person_id = p.id
            WHERE ba.book_id = book_fts.rowid
    ), '') WHERE rowid IN (SELECT book_id FROM book_authors WHERE person_id = new.id);
    UPDATE book_fts SET narrator = COALESCE((
        SELECT group_concat(p.name, ' ') FROM book_people p
            JOIN book_narrators bn ON bn.person_id = p.id
            WHERE bn.book_id = book_fts.rowid
    ), '') WHERE rowid IN (SELECT book_id FROM book_narrators WHERE person_id = new.id);
END;

-- A renamed series propagates into every book of that series.
CREATE TRIGGER series_au AFTER UPDATE OF name ON series BEGIN
    UPDATE book_fts SET series = new.name
        WHERE rowid IN (SELECT id FROM books WHERE series_id = new.id);
END;

-- Rebuild `queue` to add the deferred `book_id` foreign key now that `books`
-- exists. Migration 0006 left `book_id` a plain column with exactly this note:
-- `foreign_keys = ON` refuses DML on a child whose parent table is absent, so
-- the FK was parked until Phase 7. The 0006 episode_id rebuild is the template:
-- nothing references `queue`, so the drop/rename is safe inside the migration
-- transaction; the saved playback queue is copied across. The CHECK (which
-- already enumerates the `audiobook` kind) and the position index are recreated
-- unchanged.
CREATE TABLE queue_new (
    id          INTEGER PRIMARY KEY,
    position    INTEGER NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('track','episode','audiobook')),
    track_id    INTEGER REFERENCES tracks(id)   ON DELETE CASCADE,
    episode_id  INTEGER REFERENCES episodes(id) ON DELETE CASCADE,
    book_id     INTEGER REFERENCES books(id)    ON DELETE CASCADE,
    CHECK ( (kind='track'     AND track_id   IS NOT NULL AND episode_id IS NULL AND book_id IS NULL)
         OR (kind='episode'   AND episode_id IS NOT NULL AND track_id   IS NULL AND book_id IS NULL)
         OR (kind='audiobook' AND book_id    IS NOT NULL AND track_id   IS NULL AND episode_id IS NULL) )
);
INSERT INTO queue_new (id, position, kind, track_id, episode_id, book_id)
    SELECT id, position, kind, track_id, episode_id, book_id FROM queue;
DROP TABLE queue;
ALTER TABLE queue_new RENAME TO queue;
CREATE INDEX idx_queue_position ON queue(position);
