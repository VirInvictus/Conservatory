-- Conservatory — file-move journal (Phase 2c). See spec.md §5.4 and docs/mover.md.
--
-- `user_version` is bumped by the migration runner (db/migrations.rs), not here,
-- so this file is pure DDL. The journal is the crash-safety ledger: a job and all
-- its operations are written here (pending) BEFORE any file is touched, so a
-- crash mid-move is recoverable by replaying the pending operations (roll-forward).

CREATE TABLE move_jobs (
    id           INTEGER PRIMARY KEY,
    kind         TEXT NOT NULL,    -- 'import' | 'organize'
    mode         TEXT NOT NULL,    -- 'move' | 'copy'
    library_root TEXT NOT NULL,    -- absolute root the relative DB paths hang off
    state        TEXT NOT NULL,    -- 'in_progress' | 'completed' | 'undone' | 'failed'
    created_at   INTEGER NOT NULL  -- unix epoch seconds
);

CREATE TABLE move_operations (
    id          INTEGER PRIMARY KEY,
    job_id      INTEGER NOT NULL REFERENCES move_jobs(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,  -- order within the job
    track_id    INTEGER,           -- the track whose file_path this op moves (nullable)
    album_id    INTEGER,           -- the album whose folder_path resyncs on completion (nullable)
    src_path    TEXT NOT NULL,     -- absolute source (for direct filesystem ops)
    dst_path    TEXT NOT NULL,     -- absolute target
    db_old_path TEXT,              -- relative DB value before the move (for undo)
    db_new_path TEXT,              -- relative DB value after the move
    state       TEXT NOT NULL      -- 'pending' | 'done'
);

CREATE INDEX idx_move_ops_job ON move_operations(job_id, seq);
