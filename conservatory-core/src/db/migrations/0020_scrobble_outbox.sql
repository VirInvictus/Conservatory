-- Phase 9a: the scrobble outbox (spec §14 carve-out, off by default).
-- A local-first "listen" submission queue: a completed play (music track or
-- podcast episode) is recorded here first, then a background submitter drains it
-- to the configured service (ListenBrainz / Last.fm). The row SNAPSHOTS the
-- listen metadata at completion time, so a later library rename cannot corrupt
-- history and submission needs no join. On success the row is deleted; a
-- transient failure bumps `attempts` and pushes `next_attempt_at` out (backoff),
-- so an offline window or a down service never loses a listen.
CREATE TABLE scrobble_outbox (
    id              INTEGER PRIMARY KEY,
    -- The destination this listen is bound for, snapshotted at enqueue time so
    -- switching the configured service later cannot misroute a queued listen.
    service         TEXT    NOT NULL,           -- 'listenbrainz' | 'lastfm'
    kind            TEXT    NOT NULL,           -- 'track' | 'episode' (scope/accounting)
    listened_at     INTEGER NOT NULL,           -- unix seconds the play completed
    artist          TEXT    NOT NULL,
    track           TEXT    NOT NULL,
    album           TEXT,                        -- release / show name; nullable
    track_number    INTEGER,
    duration_secs   INTEGER,                     -- track length, if known
    recording_mbid  TEXT,                        -- MusicBrainz recording id, if tagged
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,  -- unix seconds; drain skips rows in the future
    created_at      INTEGER NOT NULL
);
-- The drain loop wants the oldest ready row first (next_attempt_at <= now).
CREATE INDEX idx_scrobble_outbox_ready ON scrobble_outbox(next_attempt_at, id);
