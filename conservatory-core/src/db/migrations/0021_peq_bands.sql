-- Phase 5.5b follow-on: the parametric equalizer option. User-defined peaking
-- bands (arbitrary centre frequency, Q, gain) rendered into the `@eq` stage of
-- the `af` chain after the graphic bands, as named `equalizer@p<idx>` biquads.
-- `idx` is the stable band identity: it orders the stage and names the live
-- af-command gain target. An empty table renders to a no-op (the `@eq` stage
-- then depends on the graphic EQ alone). Deliberately NOT anequalizer: its
-- `change` command hangs the mpv command pipeline on the current stack
-- (measured 2026-09-04; see the roadmap 5.5b re-scope).
CREATE TABLE peq_bands (
    idx       INTEGER PRIMARY KEY,  -- band identity: render order + live target p<idx>
    frequency REAL NOT NULL,          -- centre frequency, Hz (20 .. 20000)
    q         REAL NOT NULL,          -- Q factor (0.1 .. 16)
    gain_db   REAL NOT NULL           -- gain, dB (-24 .. 24)
);
