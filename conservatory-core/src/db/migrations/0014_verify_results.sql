-- Phase 8a: integrity-verification cache (spec §8, roadmap Phase 8a).
--
-- One row per verified file, keyed by its library-relative path (the canonical
-- id), with the file size + mtime at verification time so a re-verify skips
-- files that have not changed. Path-keyed, NOT track-keyed: the same table
-- serves podcasts and audiobooks in a later pass with no schema change, and an
-- orphan row after a file is removed is harmless (and cheap).
--
-- verdict is the Lattice four-tier classification, stored as text:
--   'ok'       decoded clean
--   'metadata' audio intact, only a container/tag warning
--   'suspect'  decoded to the end but the tool complained (or trailing data)
--   'corrupt'  the decoder errored, or a FLAC decoded fewer samples than declared
CREATE TABLE verify_results (
    file_path   TEXT PRIMARY KEY,   -- relative to the library root, the canonical id
    file_size   INTEGER NOT NULL,   -- bytes, fs::metadata().len()
    file_mtime  INTEGER NOT NULL,   -- unix seconds, the file's mtime at check time
    verdict     TEXT NOT NULL,      -- 'ok' | 'metadata' | 'suspect' | 'corrupt'
    detail      TEXT,               -- short tool message (suspect/corrupt only)
    checked_at  INTEGER NOT NULL    -- unix seconds of this verification
);

-- The report query filters by verdict ("show me the corrupt/suspect files").
CREATE INDEX idx_verify_verdict ON verify_results(verdict);
