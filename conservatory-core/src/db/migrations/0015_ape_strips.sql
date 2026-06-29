-- Conservatory — APE-strip undo journal (Phase 8c-iii). See spec.md §8.
--
-- The `apestrip` verb removes a stray APEv2 tag from an MP3 by byte surgery.
-- Before it touches a file it records the excised tag bytes here, so the strip
-- is exactly reversible. The row is written first and deleted on undo; the
-- size/mtime are a staleness guard so undo never re-splices into a file that
-- changed after the strip.
CREATE TABLE ape_strips (
    file_path   TEXT PRIMARY KEY,   -- root-relative, the strip target
    ape_bytes   BLOB NOT NULL,      -- the excised APEv2 tag, for undo
    tag_start   INTEGER NOT NULL,   -- byte offset the tag was removed at
    orig_size   INTEGER NOT NULL,   -- pre-strip file size (staleness guard)
    orig_mtime  INTEGER NOT NULL,   -- pre-strip mtime, unix seconds
    stripped_at INTEGER NOT NULL    -- unix seconds
);
