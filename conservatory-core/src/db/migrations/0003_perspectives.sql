-- Phase 3c: Perspectives, named saved searches (Calibre saved searches; spec
-- §3.4). Stored as text and re-parsed on load, so a saved Perspective inherits
-- later grammar additions for free. `scope` names the target list: tracks today,
-- with albums/episodes/books reusing the same table when those surfaces land.
CREATE TABLE perspectives (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    expression TEXT NOT NULL,
    scope      TEXT NOT NULL DEFAULT 'tracks',
    created_at INTEGER,
    UNIQUE (name)
);
