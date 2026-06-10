-- Conservatory — music data model (Phase 1b). See spec.md §4.1 and docs/schema.md.
--
-- `user_version` is bumped by the migration runner (db/migrations.rs), not here,
-- so this file is pure DDL. The genre tables stay decoupled from the filesystem:
-- `track_genres` are raw multi-value tags for facets/search; `albums.shelf_genre`
-- (single-valued) is the only genre input to the path (spec §5.2).

CREATE TABLE artists (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL,
    sort_name       TEXT NOT NULL,      -- "Beatles, The"; drives path + sort (Calibre author_sort)
    musicbrainz_id  TEXT,
    UNIQUE (sort_name)
);

CREATE TABLE albums (
    id                     INTEGER PRIMARY KEY,
    title                  TEXT NOT NULL,
    album_artist_id        INTEGER REFERENCES artists(id),  -- NULL => Various Artists bucket
    shelf_genre            TEXT,         -- THE ONLY input to the genre folder level (spec §5.2)
    year                   INTEGER,
    release_date           TEXT,
    musicbrainz_release_id TEXT,
    cover_path             TEXT,
    accent_rgb             INTEGER,      -- packed RGB, median-cut from cover (spec §7.4)
    folder_path            TEXT NOT NULL,-- managed; rendered from the template (spec §5.1)
    added_at               INTEGER
);
CREATE INDEX idx_albums_artist ON albums(album_artist_id);

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
    rating          INTEGER NOT NULL DEFAULT 0,  -- 0–5; foobar/Lattice loanword
    play_count      INTEGER NOT NULL DEFAULT 0,
    last_played     INTEGER,
    starred         INTEGER NOT NULL DEFAULT 0,
    musicbrainz_recording_id TEXT,
    added_at        INTEGER
);
CREATE INDEX idx_tracks_album  ON tracks(album_id, disc_no, track_no);
CREATE INDEX idx_tracks_artist ON tracks(artist_id);

-- Raw multi-value genres, preserved untouched for facets + search. NOT the shelving input.
CREATE TABLE genres (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL,
    UNIQUE (name)
);
CREATE TABLE track_genres (
    track_id INTEGER REFERENCES tracks(id)  ON DELETE CASCADE,
    genre_id INTEGER REFERENCES genres(id)  ON DELETE CASCADE,
    PRIMARY KEY (track_id, genre_id)
);

-- Genre normalization (spec §5.2; see docs/genre-normalization.md). Seed source OPEN (spec §16.4).
CREATE TABLE genre_aliases  (raw TEXT PRIMARY KEY, canonical TEXT NOT NULL);
CREATE TABLE genre_priority (genre TEXT PRIMARY KEY, rank INTEGER NOT NULL);

-- FTS5 (spec §4.4). Ordinary (not external-content) tables: track_fts.artist and
-- track_fts.album are denormalized from joined tables, so the triggers below look
-- them up and keep them in sync, including when an artist or album is renamed.
CREATE VIRTUAL TABLE track_fts USING fts5(title, artist, album);
CREATE VIRTUAL TABLE album_fts USING fts5(title, album_artist);

-- track_fts sync on the tracks table.
CREATE TRIGGER tracks_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO track_fts(rowid, title, artist, album) VALUES (
        new.id,
        new.title,
        COALESCE((SELECT name  FROM artists WHERE id = new.artist_id), ''),
        COALESCE((SELECT title FROM albums  WHERE id = new.album_id),  '')
    );
END;
CREATE TRIGGER tracks_ad AFTER DELETE ON tracks BEGIN
    DELETE FROM track_fts WHERE rowid = old.id;
END;
CREATE TRIGGER tracks_au AFTER UPDATE ON tracks BEGIN
    DELETE FROM track_fts WHERE rowid = old.id;
    INSERT INTO track_fts(rowid, title, artist, album) VALUES (
        new.id,
        new.title,
        COALESCE((SELECT name  FROM artists WHERE id = new.artist_id), ''),
        COALESCE((SELECT title FROM albums  WHERE id = new.album_id),  '')
    );
END;

-- album_fts sync on the albums table; albums_au also fixes the denormalized
-- album column on every track of the album.
CREATE TRIGGER albums_ai AFTER INSERT ON albums BEGIN
    INSERT INTO album_fts(rowid, title, album_artist) VALUES (
        new.id,
        new.title,
        COALESCE((SELECT name FROM artists WHERE id = new.album_artist_id), '')
    );
END;
CREATE TRIGGER albums_ad AFTER DELETE ON albums BEGIN
    DELETE FROM album_fts WHERE rowid = old.id;
END;
CREATE TRIGGER albums_au AFTER UPDATE ON albums BEGIN
    DELETE FROM album_fts WHERE rowid = old.id;
    INSERT INTO album_fts(rowid, title, album_artist) VALUES (
        new.id,
        new.title,
        COALESCE((SELECT name FROM artists WHERE id = new.album_artist_id), '')
    );
    UPDATE track_fts SET album = new.title
        WHERE rowid IN (SELECT id FROM tracks WHERE album_id = new.id);
END;

-- A renamed artist propagates into both denormalized FTS columns.
CREATE TRIGGER artists_au AFTER UPDATE OF name ON artists BEGIN
    UPDATE track_fts SET artist = new.name
        WHERE rowid IN (SELECT id FROM tracks WHERE artist_id = new.id);
    UPDATE album_fts SET album_artist = new.name
        WHERE rowid IN (SELECT id FROM albums WHERE album_artist_id = new.id);
END;
