-- Track credits (19b-iii): role-tagged people links for music tracks, the
-- book_authors/book_narrators precedent (0011) generalized to one link table
-- with a role column. People are the existing `artists` rows, so credit names
-- share one namespace (and sort_name discipline) with track/album artists.
-- Roles are TEXT so the rest of the People & Organizations family (Conductor,
-- Lyricist, Arranger, ...) lands later without another migration; v1 writes
-- 'Composer', 'Performer', 'Producer' read from the embedded tags.
CREATE TABLE track_credits (
    track_id  INTEGER REFERENCES tracks(id)  ON DELETE CASCADE,
    artist_id INTEGER REFERENCES artists(id) ON DELETE CASCADE,
    role      TEXT NOT NULL,
    PRIMARY KEY (track_id, artist_id, role)
);
CREATE INDEX idx_track_credits_artist ON track_credits(artist_id);
