# Database Schema Reference

> **Status: design reference; draft schema.** The first migrations land at roadmap Phase 1a/1b. This is the living companion to spec §4: the spec defines the contract, this file is where column-level detail and migration history accumulate as they firm up. Where they differ, spec §4 wins until this file is reconciled.

## Connection discipline

- **WAL** mode, mandatory.
- `foreign_keys = ON`, `synchronous = NORMAL`, `temp_store = MEMORY`, a bounded `mmap_size`, `journal_size_limit`.
- **Single writer:** one writable connection owned by a dedicated worker task; the rest of the engine holds a `Sender<Command>` (spec §2.1). Read commands open read-only at the process level, served by a small read-only pool.
- **FTS5** on titles (spec §4.4).

## Migrations

Versioned via `PRAGMA user_version`, append-only and backwards-compatible post-1.0 (the Atrium discipline). Each migration is a numbered step that bumps `user_version`. This is deliberately **not** Viaduct's `CREATE TABLE IF NOT EXISTS` setup: the library is the user's irreplaceable data, so the schema history is an explicit ledger, not an idempotent best-effort. The mover's re-import contract (spec §5.6) and the nightly backup protect the curated layer that a re-import cannot rebuild.

**All schema is core-owned, regardless of plugin features** (spec §2.2): the podcast and audiobook tables land in `conservatory-core`'s single ledger at Phases 6a/7a and apply in every build, so a music-only build (`--no-default-features`) has the same `user_version` and the same (empty) tables as a full build. Plugin crates never own migrations; the plugin boundary is code and dependencies, not the database.

## Music tables (draft, spec §4.1)

```sql
CREATE TABLE artists (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL,
    sort_name       TEXT NOT NULL,      -- "Beatles, The"; drives path + sort (Calibre author_sort)
    musicbrainz_id  TEXT,
    UNIQUE (sort_name)
);

CREATE TABLE albums (
    id                  INTEGER PRIMARY KEY,
    title               TEXT NOT NULL,
    album_artist_id     INTEGER REFERENCES artists(id),  -- NULL => Various Artists bucket
    shelf_genre         TEXT,           -- THE ONLY input to the genre folder level (spec §5.2)
    year                INTEGER,
    release_date        TEXT,
    musicbrainz_release_id TEXT,
    cover_path          TEXT,
    accent_rgb          INTEGER,        -- packed RGB, median-cut from cover (spec §7.4)
    folder_path         TEXT NOT NULL,  -- managed; rendered from the template (spec §5.1)
    added_at            INTEGER
);

CREATE TABLE tracks (
    id              INTEGER PRIMARY KEY,
    album_id        INTEGER REFERENCES albums(id) ON DELETE CASCADE,
    artist_id       INTEGER REFERENCES artists(id),  -- track artist (may differ from album artist)
    title           TEXT NOT NULL,
    track_no        INTEGER,
    disc_no         INTEGER,
    duration        REAL,               -- seconds
    file_path       TEXT NOT NULL,      -- managed; under the album folder
    format          TEXT,               -- flac/mp3/opus/aac/...
    bitrate         INTEGER,
    sample_rate     INTEGER,
    replaygain_track REAL,
    replaygain_album REAL,
    rating          INTEGER DEFAULT 0,  -- 0–5; foobar/Lattice loanword
    play_count      INTEGER DEFAULT 0,
    last_played     INTEGER,
    starred         INTEGER DEFAULT 0,
    musicbrainz_recording_id TEXT,
    added_at        INTEGER
);

-- Raw multi-value genres, preserved untouched for facets + search. NOT the shelving input.
CREATE TABLE genres (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL);
CREATE TABLE track_genres (
    track_id INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
    genre_id INTEGER REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (track_id, genre_id)
);

-- Genre normalization (spec §5.2; see docs/genre-normalization.md). Seed source OPEN (spec §16.4).
CREATE TABLE genre_aliases  (raw TEXT PRIMARY KEY, canonical TEXT NOT NULL);
CREATE TABLE genre_priority (genre TEXT PRIMARY KEY, rank INTEGER NOT NULL);
```

## Unified queue (spec §4.3)

One ordered queue across both media types; the bridge that makes the unified queue real.

```sql
CREATE TABLE queue (
    id          INTEGER PRIMARY KEY,
    position    INTEGER NOT NULL,       -- explicit, drag-reorderable
    kind        TEXT NOT NULL CHECK (kind IN ('track','episode','audiobook')),
    track_id    INTEGER REFERENCES tracks(id)   ON DELETE CASCADE,
    episode_id  INTEGER REFERENCES episodes(id) ON DELETE CASCADE,
    book_id     INTEGER REFERENCES books(id)    ON DELETE CASCADE,  -- audiobook = one entry (spec §3.8)
    CHECK ( (kind='track'     AND track_id   IS NOT NULL AND episode_id IS NULL AND book_id IS NULL)
         OR (kind='episode'   AND episode_id IS NOT NULL AND track_id   IS NULL AND book_id IS NULL)
         OR (kind='audiobook' AND book_id    IS NOT NULL AND track_id   IS NULL AND episode_id IS NULL) )
);
CREATE INDEX idx_queue_position ON queue(position);
```

The engine reads `queue` into an in-memory `Vec<PlayableItem>` (spec §6.1); position writes are debounced. A whole audiobook is a single queue entry (chapters are navigated within it, not enqueued separately). Resume position for long items lives in the per-kind state tables (`tracks.last_played` / the podcast `playback` table / the `book_playback` table).

## Move journal (Phase 2c, spec §5.4)

The crash-safety ledger for the file mover. A job and all its operations are written **before** any file is touched, so a crash mid-move is recoverable by replaying the `pending` operations (roll-forward; see [`mover.md`](mover.md)). DB paths are stored relative to `library_root`; the journal stores absolute `src_path`/`dst_path` for direct filesystem ops.

```sql
CREATE TABLE move_jobs (
    id INTEGER PRIMARY KEY, kind TEXT NOT NULL,   -- 'import' | 'organize'
    mode TEXT NOT NULL,                           -- 'move' | 'copy'
    library_root TEXT NOT NULL,
    state TEXT NOT NULL,                          -- 'in_progress' | 'completed' | 'undone' | 'failed'
    created_at INTEGER NOT NULL
);
CREATE TABLE move_operations (
    id INTEGER PRIMARY KEY, job_id INTEGER NOT NULL REFERENCES move_jobs(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL, track_id INTEGER, album_id INTEGER,
    src_path TEXT NOT NULL, dst_path TEXT NOT NULL,
    db_old_path TEXT, db_new_path TEXT,           -- relative DB values, for the path update + undo
    state TEXT NOT NULL                           -- 'pending' | 'done'
);
CREATE INDEX idx_move_ops_job ON move_operations(job_id, seq);
```

## Podcast tables (Phase 6, spec §4.2)

Ported from Belfry §4.1 at Phase 6a: `shows`, `episodes`, `playback`, `show_settings`, `listening_sessions`, `chapters`, `tags`, `show_tags`. One change from Belfry: triage Queue state is represented through the unified `queue` table above rather than a per-episode `in_queue` flag. The append-only `listening_sessions` discipline is preserved. Column-level detail migrates into this file as the absorption is implemented.

## Audiobook tables (Phase 7, spec §4.5)

Landed at Phase 7a, modeled on Audiobookshelf's relational shape and Cozy's Book → Chapter → file model. A book is the unit; chapters are ordered and come from embedded M4B markers or one-file-per-chapter folders; authors and narrators are distinct roles (many-to-many via `book_authors` / `book_narrators`); `series` carries a decimal `series_sequence`. Resume is a single `book_playback` row per book (the podcast `playback` analogue, never lost), holding the absolute `position`, `finished`, and per-book speed / Smart Speed / Voice Boost overrides.

```sql
CREATE TABLE book_people (              -- authors + narrators, role-tagged via the link tables
    id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort_name TEXT NOT NULL, UNIQUE (sort_name)
);
CREATE TABLE series (id INTEGER PRIMARY KEY, name TEXT NOT NULL, UNIQUE (name));

CREATE TABLE books (
    id INTEGER PRIMARY KEY, title TEXT NOT NULL, subtitle TEXT,
    series_id INTEGER REFERENCES series(id), series_sequence REAL,   -- decimal: "Book 1.5"
    year INTEGER, publisher TEXT, isbn TEXT, asin TEXT, description TEXT, language TEXT,
    shelf_genre TEXT, cover_path TEXT, accent_rgb INTEGER,
    folder_path TEXT NOT NULL,          -- managed; rendered from the audiobook template (spec §5.7)
    rating INTEGER DEFAULT 0, starred INTEGER DEFAULT 0, added_at INTEGER
);
CREATE TABLE book_authors   (book_id INTEGER REFERENCES books(id) ON DELETE CASCADE,
                             person_id INTEGER REFERENCES book_people(id) ON DELETE CASCADE,
                             PRIMARY KEY (book_id, person_id));
CREATE TABLE book_narrators (book_id INTEGER REFERENCES books(id) ON DELETE CASCADE,
                             person_id INTEGER REFERENCES book_people(id) ON DELETE CASCADE,
                             PRIMARY KEY (book_id, person_id));

CREATE TABLE book_chapters (            -- file_path + file_offset addresses a file OR an M4B span
    id INTEGER PRIMARY KEY, book_id INTEGER REFERENCES books(id) ON DELETE CASCADE,
    idx INTEGER NOT NULL, title TEXT, file_path TEXT NOT NULL,
    file_offset REAL NOT NULL DEFAULT 0, duration REAL, UNIQUE (book_id, idx)
);
CREATE TABLE book_playback (            -- one row per book; first-class resume (spec §6.4)
    book_id INTEGER PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    position REAL NOT NULL DEFAULT 0, finished INTEGER NOT NULL DEFAULT 0, last_played INTEGER,
    speed REAL, smart_speed INTEGER, voice_boost INTEGER   -- NULL = global default
);
```

## FTS5 (spec §4.4)

- `track_fts` (title, artist, album)
- `album_fts` (title, album artist)
- `episode_fts`, `show_fts` (Phase 6)
- `book_fts` (title, author, narrator, series) (Phase 7)

Triggers keep them in sync on insert/update/delete. Consumed by `conservatory-search` for the bare-text path and bm25 ranking (see [`search-grammar.md`](search-grammar.md)). Not transcripts (spec §14).
