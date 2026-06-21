-- 0004_playback_state.sql — Phase 4a (spec §6.4, docs/schema.md).
--
-- The transport cursor: a single row recording what was playing and where, so a
-- restart resumes where the user left off. The unified `queue` table (Phase 4b)
-- holds the ordered list; this stays the cursor into it. A singleton, enforced
-- by the id = 1 check, because there is exactly one "now playing" position.
--
-- track_id is ON DELETE SET NULL so removing the playing track from the library
-- never dangles the cursor (foreign_keys is ON). position is absolute seconds
-- into the current item, the same discipline the podcast/audiobook state tables
-- will follow at their phases.
CREATE TABLE playback_state (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    track_id   INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
    position   REAL    NOT NULL DEFAULT 0,
    paused     INTEGER NOT NULL DEFAULT 0,
    volume     INTEGER NOT NULL DEFAULT 100,
    updated_at INTEGER
);
