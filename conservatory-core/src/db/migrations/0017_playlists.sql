-- Phase 16d: playlists. Two kinds share one table:
--   * 'static' - a frozen, hand-ordered list; its members live in playlist_entries.
--   * 'smart'  - a live rule: `query` is a §3.4 filter expression, optionally
--                capped at `limit_n` and ordered by `order_by`; it holds no
--                entries and materialises from the query on demand.
-- This is distinct from a Perspective (migration 0003): a Perspective is a saved
-- query applied to the browse surface; a Smart Playlist is a query that is itself
-- a queue source, carrying its own limit and order. Three crisp primitives.
CREATE TABLE playlists (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,   -- 'static' | 'smart'
    query      TEXT,            -- smart: the filter expression; static: NULL
    limit_n    INTEGER,         -- smart: optional cap; NULL = unlimited
    order_by   TEXT,            -- smart: 'added'|'rating'|'lastplayed'|'title'|'artist'; NULL = default (added)
    created_at INTEGER NOT NULL
);

-- A static playlist's members, position-ordered. Mirrors the queue table exactly
-- (synthetic id PK, a plain non-unique `position` so the same reorder shift works
-- without transient uniqueness collisions, the multi-kind track/episode/book
-- columns, and the exactly-one-id CHECK). ON DELETE CASCADE keeps an entry from
-- outliving its item or its playlist. v1 wires tracks; the columns exist for later.
CREATE TABLE playlist_entries (
    id          INTEGER PRIMARY KEY,
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    kind        TEXT    NOT NULL CHECK (kind IN ('track','episode','audiobook')),
    track_id    INTEGER REFERENCES tracks(id)   ON DELETE CASCADE,
    episode_id  INTEGER REFERENCES episodes(id) ON DELETE CASCADE,
    book_id     INTEGER REFERENCES books(id)    ON DELETE CASCADE,
    CHECK ( (kind='track'     AND track_id   IS NOT NULL AND episode_id IS NULL AND book_id IS NULL)
         OR (kind='episode'   AND episode_id IS NOT NULL AND track_id   IS NULL AND book_id IS NULL)
         OR (kind='audiobook' AND book_id    IS NOT NULL AND track_id   IS NULL AND episode_id IS NULL) )
);
CREATE INDEX idx_playlist_entries_playlist ON playlist_entries(playlist_id, position);
