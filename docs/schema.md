# Database Schema Reference

> **Status: living reference.** Migrations landed so far: `0001` (music schema + FTS5, Phase 1b), `0002` (move journal, Phase 2c), `0003` (perspectives, Phase 3c), `0004` (playback state, Phase 4a), `0005` (unified queue, Phase 4b-i), `0006` (podcast tables + the queue `episode_id` foreign key, Phase 6a-i), `0007` (the per-kind playback cursor: `playback_state.kind` + `episode_id`, Phase 6b-ii-c-2), `0008` (the equalizer: `eq_presets` + `eq_state`, Phase 5.5b), `0009` (the audio config: `audio_state`, Phase 5.5c), `0010` (the 16 built-in EQ presets, Phase 5.5b follow-on), and `0011` (the audiobook tables + `book_fts` + the queue `book_id` foreign key, Phase 7a-i). This is the living companion to spec §4: the spec defines the contract, this file is where column-level detail and migration history accumulate as they firm up. Where they differ, spec §4 wins until this file is reconciled.

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

## Unified queue (Phase 4b-i, migration `0005`; episode FK at `0006`, spec §4.3)

One ordered queue across all three media kinds; the bridge that makes the unified queue real. The full column set landed at `0005`, but **only `track_id` carried a foreign key then**: with `foreign_keys = ON`, SQLite refuses any INSERT/UPDATE/DELETE on a child table whose parent does not exist yet, even when the referencing column is NULL. So `episode_id`/`book_id` were plain columns until their parent tables land, at which point a table rebuild re-adds the `REFERENCES ... ON DELETE CASCADE`. **Migration `0006` rebuilt `queue` to add the `episode_id` FK now that `episodes` exists** (Phase 6a-i); **migration `0011` did the same for `book_id` once `books` landed** (Phase 7a-i). All three id columns now carry their `REFERENCES ... ON DELETE CASCADE`; the CHECK still guards the one-id-per-row invariant.

```sql
CREATE TABLE queue (
    id          INTEGER PRIMARY KEY,
    position    INTEGER NOT NULL,       -- explicit, contiguous 0..n-1, drag-reorderable
    kind        TEXT NOT NULL CHECK (kind IN ('track','episode','audiobook')),
    track_id    INTEGER REFERENCES tracks(id)   ON DELETE CASCADE,
    episode_id  INTEGER REFERENCES episodes(id) ON DELETE CASCADE,  -- FK added at 0006 (Phase 6a-i)
    book_id     INTEGER REFERENCES books(id)    ON DELETE CASCADE,  -- FK added at 0011 (Phase 7a-i); audiobook = one entry (spec §3.8)
    CHECK ( (kind='track'     AND track_id   IS NOT NULL AND episode_id IS NULL AND book_id IS NULL)
         OR (kind='episode'   AND episode_id IS NOT NULL AND track_id   IS NULL AND book_id IS NULL)
         OR (kind='audiobook' AND book_id    IS NOT NULL AND track_id   IS NULL AND episode_id IS NULL) )
);
CREATE INDEX idx_queue_position ON queue(position);
```

The engine reads `queue` into an in-memory `Vec<PlayableItem>` (spec §6.1); positions are kept contiguous and renumbered transactionally on the single writer (enqueue/remove/reorder/clear). A whole audiobook is a single queue entry (chapters are navigated within it, not enqueued separately). `is:queued` (search §3.4) tests membership: the SQL path emits `tracks.id IN (SELECT track_id FROM queue WHERE kind='track' ...)`, the eval path reads the same via `SearchRow.queued`. Resume position for long items lives in the per-kind state tables (`tracks.last_played` / the podcast `playback` table / the `book_playback` table); the *current* item and its offset live in `playback_state` (below).

## Playback state (Phase 4a, migration `0004`; cursor kind at `0007`, spec §6.4)

The transport cursor: a single row recording what is playing and where, so a restart resumes. The unified `queue` above holds the ordered list; this is the cursor *into* it. A singleton (the `id = 1` check), because there is exactly one "now playing" position. `position` is absolute seconds into the current item.

The cursor is **per-kind** (Phase 6b-ii-c-2): the unified queue interleaves tracks and episodes, so the cursor records its `kind` plus the matching id, and a restart reopens an episode rather than only the last track. **Migration `0007`** adds `kind` (defaulting to `'track'`, so the pre-existing singleton stays a valid music cursor) and `episode_id`. `track_id` is set when `kind = 'track'`, `episode_id` when `kind = 'episode'`; both are `ON DELETE SET NULL` so removing the playing item from the library never dangles the cursor (`foreign_keys = ON`). The episode FK can be added by `ALTER ... ADD COLUMN` because its default is NULL.

```sql
CREATE TABLE playback_state (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    track_id   INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
    position   REAL    NOT NULL DEFAULT 0,
    paused     INTEGER NOT NULL DEFAULT 0,
    volume     INTEGER NOT NULL DEFAULT 100,
    updated_at INTEGER,
    kind       TEXT    NOT NULL DEFAULT 'track',                    -- added at 0007 (Phase 6b-ii-c-2)
    episode_id INTEGER REFERENCES episodes(id) ON DELETE SET NULL   -- added at 0007
);
```

Writes are debounced (the 30 s insurance interval, plus the forced pause/seek/end/quit points, spec §6.4) and go through the single writer. The **per-episode** resume position + played state live in the podcast `playback` table (the engine writes them on an episode's tick/EOF, so they survive after the queue moves on); this singleton only records the *current* item to reopen. `play_count` / `last_played` are bumped separately, only on a natural end-of-file: on `tracks` for a track, in `playback` for an episode.

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

## Perspectives (Phase 3c, spec §3.4)

Named saved searches (Calibre saved searches). The `expression` is the raw filter text, stored verbatim and re-parsed on load so a Perspective inherits later grammar additions for free. `vl:NAME` references in any expression resolve against this table at parse time. `scope` names the target list: `tracks` today, with albums/episodes/books reusing the same table when those surfaces land.

```sql
CREATE TABLE perspectives (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    expression TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'tracks',
    created_at INTEGER,
    UNIQUE (name)
);
```

## Equalizer (Phase 5.5b, migration `0008`, spec §6.2)

The graphic equalizer (10-band ISO octave) rendered as the `@eq` stage of the `af` chain. `eq_presets` are named gain sets (the `perspectives` precedent), seeded with `Flat` (0008) plus 16 built-in starter curves (migration `0010`: Bass Boost / Reducer, Treble Boost / Reducer, Loudness, Vocal Boost, Spoken Word, Small Speakers, and the genre set Acoustic / Classical / Jazz / Rock / Pop / Electronic / Hip-Hop / Dance). The built-ins are `INSERT OR IGNORE`, so a user preset of the same name is preserved and a deleted built-in does not return. `eq_state` is the singleton active EQ: the live band values plus the selected preset name (`NULL` once a band is edited away from a preset). `bands` is a CSV of ten gains in dB (no serde in core; the read is forgiving — a bad value reads as 0 dB for that band). A flat state renders to a no-op chain (no `@eq` stage). Live per-band mutation (`af-command`) and the GTK Sound dialog land at 5.5b-ii.

```sql
CREATE TABLE eq_presets (
    name  TEXT PRIMARY KEY,
    bands TEXT NOT NULL              -- CSV of 10 gains, dB
);

CREATE TABLE eq_state (
    id          INTEGER PRIMARY KEY CHECK (id = 0),  -- singleton
    preset_name TEXT,                                -- selected preset; NULL = custom
    bands       TEXT NOT NULL                        -- live band values, CSV of 10 gains, dB
);
```

## Audio configuration (Phase 5.5c, migration `0009`, spec §6.2, §6.5)

The singleton active audio config: the playback defaults (ReplayGain mode / preamp / clip, gapless), the DSP modules, and the output backend / resampler. The `eq_state` precedent (one row, `id = 0`); `get_audio_state` reads it, `set_audio_state` overwrites it. Each DSP module is an `enabled` flag plus its parameters, written unconditionally so the parameters survive an off toggle (only `enabled` gates whether the module contributes an `af`-chain stage). The compressor threshold and limiter ceiling are stored in dBFS and converted to the filters' linear forms at stage-build time. The DSP + output halves are consumed at 5.5c-i / 5.5c-ii; the playback defaults are consumed at 5.5c-ii (the queue builders read them instead of the hardcoded `PlaybackConfig::default()`). They all land in this one migration so 5.5c-ii needs no second one.

```sql
CREATE TABLE audio_state (
    id                  INTEGER PRIMARY KEY CHECK (id = 0),  -- singleton
    replaygain_mode     TEXT    NOT NULL,   -- 'off' | 'track' | 'album'
    replaygain_preamp   REAL    NOT NULL,   -- dB
    replaygain_clip     INTEGER NOT NULL,   -- bool
    gapless             INTEGER NOT NULL,   -- bool
    comp_enabled        INTEGER NOT NULL,   -- bool
    comp_threshold_db   REAL    NOT NULL,
    comp_ratio          REAL    NOT NULL,
    comp_attack_ms      REAL    NOT NULL,
    comp_release_ms     REAL    NOT NULL,
    limiter_enabled     INTEGER NOT NULL,   -- bool
    limiter_ceiling_db  REAL    NOT NULL,
    leveler_enabled     INTEGER NOT NULL,   -- bool
    leveler_target_peak REAL    NOT NULL,
    leveler_gausssize   INTEGER NOT NULL,
    output_backend      TEXT    NOT NULL,   -- 'auto' | 'pipewire' | 'pulse' | 'alsa' | 'jack'
    resampler_quality   TEXT    NOT NULL    -- 'default' | 'high'
);
```

## Podcast tables (Phase 6a-i, migration `0006`, spec §4.2)

Ported from Belfry §4.1: `shows`, `episodes`, `playback`, `show_settings`, `listening_sessions`, `chapters`, `tags`, `show_tags`. **One change from Belfry:** triage Queue state is represented through the unified `queue` table above, not a per-episode flag, so `playback` drops Belfry's `in_queue` / `queue_position` columns (and the index that paired them). Inbox / Queue / Played is derived from `playback.played` plus membership in `queue`. The append-only `listening_sessions` discipline is preserved. `episode_type` is stored as the feed's raw string rather than a closed enum, so an unexpected value never fails a read.

The conditional-GET bookkeeping (`shows.etag` / `last_modified` / `last_fetched`) and `auth_user` / `auth_pass_ref` (an oo7/libsecret reference, never an inline secret) are the hooks the Phase 6a-ii fetch loop writes; the columns land here so the schema is complete from the first podcast migration.

```sql
CREATE TABLE shows (
    id INTEGER PRIMARY KEY, slug TEXT UNIQUE NOT NULL, feed_url TEXT UNIQUE NOT NULL,
    title TEXT NOT NULL, author TEXT, description TEXT, homepage_url TEXT,
    cover_path TEXT, accent_rgb INTEGER, apple_podcasts_id TEXT,     -- accent (spec §7.4); applePodcastsID on OPML round-trip
    last_fetched INTEGER, last_modified TEXT, etag TEXT,             -- conditional-GET state (Phase 6a-ii)
    fetch_interval INTEGER NOT NULL DEFAULT 3600,
    auth_user TEXT, auth_pass_ref TEXT,                             -- HTTP Basic; pass is a libsecret ref (oo7)
    auto_download INTEGER NOT NULL DEFAULT 0, keep_count INTEGER NOT NULL DEFAULT 0,
    priority INTEGER NOT NULL DEFAULT 0,                            -- Overcast-style ordering
    folder_path TEXT NOT NULL                                       -- <root>/Podcasts/<slug> (spec §5.3)
);
CREATE TABLE episodes (
    id INTEGER PRIMARY KEY, show_id INTEGER NOT NULL REFERENCES shows(id) ON DELETE CASCADE,
    guid TEXT NOT NULL, title TEXT NOT NULL, description TEXT,
    pub_date INTEGER, duration INTEGER, file_size INTEGER,
    audio_url TEXT, audio_path TEXT,                                -- audio_path NULL until downloaded (spec §5.3)
    folder_path TEXT NOT NULL, mime_type TEXT,
    season INTEGER, episode_number INTEGER, episode_type TEXT,
    UNIQUE (show_id, guid)                                          -- episode identity (spec §8)
);
CREATE INDEX idx_episodes_show_pub ON episodes(show_id, pub_date DESC);

-- Triage + playback. Queue membership is NOT here (it's the unified queue). played:
-- 0=unplayed, 1=in-progress, 2=played-fully, 3=archived-unlistened.
CREATE TABLE playback (
    episode_id INTEGER PRIMARY KEY REFERENCES episodes(id) ON DELETE CASCADE,
    position REAL NOT NULL DEFAULT 0, played INTEGER NOT NULL DEFAULT 0,
    last_played INTEGER, play_count INTEGER NOT NULL DEFAULT 0, starred INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_playback_inbox   ON playback(played)  WHERE played = 0;
CREATE INDEX idx_playback_starred ON playback(starred) WHERE starred = 1;

CREATE TABLE show_settings (                                        -- per-show overrides (spec §3.7)
    show_id INTEGER PRIMARY KEY REFERENCES shows(id) ON DELETE CASCADE,
    playback_speed REAL NOT NULL DEFAULT 1.0, smart_speed INTEGER NOT NULL DEFAULT 1,
    voice_boost INTEGER NOT NULL DEFAULT 0, skip_intro INTEGER NOT NULL DEFAULT 0,
    skip_outro INTEGER NOT NULL DEFAULT 0, skip_forward INTEGER, skip_back INTEGER,  -- NULL = inherit global
    inbox_policy TEXT NOT NULL DEFAULT 'inbox'                      -- 'inbox' | 'always_queue' | 'always_archive'
);
CREATE TABLE listening_sessions (                                  -- append-only; Smart Speed time-saved (spec §6.3)
    id INTEGER PRIMARY KEY, episode_id INTEGER NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL,
    real_seconds REAL NOT NULL, audio_seconds REAL NOT NULL, smart_speed_saved REAL NOT NULL DEFAULT 0
);
CREATE TABLE chapters (                                            -- podcast:chapters JSON or ID3 CHAP (spec §8)
    id INTEGER PRIMARY KEY, episode_id INTEGER NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    start_time REAL NOT NULL, end_time REAL, title TEXT, url TEXT, image_path TEXT
);
CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL);
CREATE TABLE show_tags (                                           -- preserved on OPML round-trip (spec §8)
    show_id INTEGER REFERENCES shows(id) ON DELETE CASCADE,
    tag_id  INTEGER REFERENCES tags(id)  ON DELETE CASCADE,
    PRIMARY KEY (show_id, tag_id)
);
```

## Audiobook tables (Phase 7a-i, migration `0011`, spec §4.5)

Landed at Phase 7a-i, modeled on Audiobookshelf's relational shape and Cozy's Book → Chapter → file model. A book is the unit; chapters are ordered and come from embedded M4B markers or one-file-per-chapter folders; authors and narrators are distinct roles (many-to-many via `book_authors` / `book_narrators`); `series` carries a decimal `series_sequence`. Resume is a single `book_playback` row per book (the podcast `playback` analogue, never lost), holding the absolute `position`, `finished`, and per-book speed / Smart Speed / Voice Boost overrides. `format` / `bitrate` / `sample_rate` are read per chapter file at import and total duration is derived by summing chapter durations, so none is persisted. Indexes: `idx_books_series` on `books(series_id)`, `idx_book_chapters_book` on `book_chapters(book_id, idx)`. The same migration rebuilds `queue` to add the deferred `book_id` foreign key (below).

`book_fts` is unlike `episode_fts`: its `author` / `narrator` / `series` columns denormalize from the *link* tables, not the `books` row. So `books_ai`/`au`/`ad` maintain `title` + `series`, triggers on `book_authors` / `book_narrators` re-aggregate the author / narrator columns (a space-joined `group_concat`) as links change, and `book_people_au` / `series_au` propagate a rename back into the index (the `0001` `artists_au` precedent).

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
- `episode_fts` (title, description), `show_fts` (title, author, description) — Phase 6a-i, migration `0006`
- `book_fts` (title, author, narrator, series) — Phase 7a-i, migration `0011`; author/narrator/series denormalized from the link tables (see the audiobook section)

Triggers keep them in sync on insert/update/delete. Consumed by `conservatory-search` for the bare-text path and bm25 ranking (see [`search-grammar.md`](search-grammar.md)). Not transcripts (spec §14).
