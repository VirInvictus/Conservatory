-- Phase 5.5b: the graphic equalizer (spec §6.2, docs/libmpv-profiles.md). A
-- 10-band ISO-octave graphic EQ rendered as the `@eq` stage of the `af` chain;
-- `bands` is a CSV of ten gains in dB. `eq_presets` are named (the `perspectives`
-- precedent), seeded with `Flat`. `eq_state` is the singleton active EQ: the live
-- band values plus the selected preset name (NULL once a band is edited away from
-- a preset). `Flat` (all zeros) renders to a no-op chain (no `@eq` stage).
CREATE TABLE eq_presets (
    name  TEXT PRIMARY KEY,
    bands TEXT NOT NULL              -- CSV of 10 gains, dB
);

INSERT INTO eq_presets (name, bands) VALUES ('Flat', '0,0,0,0,0,0,0,0,0,0');

CREATE TABLE eq_state (
    id          INTEGER PRIMARY KEY CHECK (id = 0),
    preset_name TEXT,                -- the selected preset; NULL = custom edit
    bands       TEXT NOT NULL        -- the live band values, CSV of 10 gains, dB
);

INSERT INTO eq_state (id, preset_name, bands) VALUES (0, 'Flat', '0,0,0,0,0,0,0,0,0,0');
