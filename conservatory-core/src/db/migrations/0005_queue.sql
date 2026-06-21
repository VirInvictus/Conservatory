-- 0005_queue.sql — Phase 4b unified queue (spec §4.3, §6.1, docs/schema.md).
--
-- One ordered queue across all three media kinds; the bridge that makes the
-- unified queue real. The full column set lands now (the schema is core-owned
-- and the unified queue is a core commitment, CLAUDE.md), but only `track_id`
-- carries a foreign key today: with `foreign_keys = ON`, SQLite refuses *any*
-- INSERT on a child table whose parent table does not exist yet, even when the
-- referencing column is NULL. So the `episode_id`/`book_id` foreign keys (with
-- their ON DELETE CASCADE) are added when the `episodes` (Phase 6) and `books`
-- (Phase 7) tables land, via a table rebuild; until then those columns are plain
-- and their integrity rests on the CHECK below plus app logic. `track_id` keeps
-- its FK because `tracks` already exists.
--
-- `position` is explicit and contiguous (0..n-1), maintained by the single
-- writer; drag-reorder and remove renumber within one transaction. The CHECK
-- keeps exactly one of the three id columns non-NULL per row, matched to `kind`.
CREATE TABLE queue (
    id          INTEGER PRIMARY KEY,
    position    INTEGER NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('track','episode','audiobook')),
    track_id    INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
    episode_id  INTEGER,
    book_id     INTEGER,
    CHECK ( (kind='track'     AND track_id   IS NOT NULL AND episode_id IS NULL AND book_id IS NULL)
         OR (kind='episode'   AND episode_id IS NOT NULL AND track_id   IS NULL AND book_id IS NULL)
         OR (kind='audiobook' AND book_id    IS NOT NULL AND track_id   IS NULL AND episode_id IS NULL) )
);
CREATE INDEX idx_queue_position ON queue(position);
