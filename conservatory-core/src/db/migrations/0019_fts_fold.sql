-- Phase 18a: accent-fold bare-text search on the SQL fast path. Rebuild every FTS
-- table with the `unicode61 remove_diacritics 2` tokenizer so a search for `bjork`
-- matches `Björk` through FTS `MATCH`, mirroring the eval path's `fold` (search
-- crate) so the all-or-nothing dual path agrees. The default tokenizer left
-- diacritics in (or only partially removed under mode 1), so bare text was
-- accent-sensitive.
--
-- These are ordinary (not external-content) FTS tables kept in sync by triggers
-- (0001 / 0006 / 0011). Dropping and recreating each with the same name and
-- columns leaves those triggers valid; no DML on the source tables happens between
-- the drop and the repopulate, so the triggers never fire into a missing table.
-- Repopulation mirrors each table's trigger insert exactly. One-time re-tokenize of
-- the whole library, crash-safe inside the migration transaction.

-- track_fts: title on the row, artist/album denormalized via lookup (0001).
DROP TABLE track_fts;
CREATE VIRTUAL TABLE track_fts USING fts5(
    title, artist, album,
    tokenize = "unicode61 remove_diacritics 2"
);
INSERT INTO track_fts(rowid, title, artist, album)
SELECT t.id, t.title,
       COALESCE(ar.name, ''),
       COALESCE(al.title, '')
FROM tracks t
    LEFT JOIN artists ar ON ar.id = t.artist_id
    LEFT JOIN albums  al ON al.id = t.album_id;

-- album_fts: title on the row, album_artist via lookup (0001).
DROP TABLE album_fts;
CREATE VIRTUAL TABLE album_fts USING fts5(
    title, album_artist,
    tokenize = "unicode61 remove_diacritics 2"
);
INSERT INTO album_fts(rowid, title, album_artist)
SELECT a.id, a.title, COALESCE(ar.name, '')
FROM albums a
    LEFT JOIN artists ar ON ar.id = a.album_artist_id;

-- episode_fts / show_fts: single-table copies (0006).
DROP TABLE episode_fts;
CREATE VIRTUAL TABLE episode_fts USING fts5(
    title, description,
    tokenize = "unicode61 remove_diacritics 2"
);
INSERT INTO episode_fts(rowid, title, description)
SELECT id, title, COALESCE(description, '') FROM episodes;

DROP TABLE show_fts;
CREATE VIRTUAL TABLE show_fts USING fts5(
    title, author, description,
    tokenize = "unicode61 remove_diacritics 2"
);
INSERT INTO show_fts(rowid, title, author, description)
SELECT id, title, COALESCE(author, ''), COALESCE(description, '') FROM shows;

-- book_fts: title + series on/near the row; author/narrator group_concat over the
-- role link tables (0011).
DROP TABLE book_fts;
CREATE VIRTUAL TABLE book_fts USING fts5(
    title, author, narrator, series,
    tokenize = "unicode61 remove_diacritics 2"
);
INSERT INTO book_fts(rowid, title, author, narrator, series)
SELECT b.id, b.title,
    COALESCE((SELECT group_concat(p.name, ' ') FROM book_people p
                JOIN book_authors ba ON ba.person_id = p.id
                WHERE ba.book_id = b.id), ''),
    COALESCE((SELECT group_concat(p.name, ' ') FROM book_people p
                JOIN book_narrators bn ON bn.person_id = p.id
                WHERE bn.book_id = b.id), ''),
    COALESCE((SELECT name FROM series WHERE id = b.series_id), '')
FROM books b;
