-- Phase 5.5b follow-on: built-in EQ presets (spec §6.2, docs/libmpv-profiles.md).
-- 0008 seeded only `Flat`; this stocks the equalizer with a useful starter set so
-- the Sound dialog's preset dropdown (and the `eq preset` CLI) are not empty until
-- the user builds their own. Bands are CSV gains in dB at the 10 ISO octave centres
-- 31 / 62 / 125 / 250 / 500 / 1k / 2k / 4k / 8k / 16k Hz (EqState::parse_bands).
--
-- Curves follow the classic iTunes/Winamp shapes, adapted to the peaking-band EQ
-- and kept conservative (mostly <= 6 dB) so octave-overlapping boosts do not stack
-- into clipping. `INSERT OR IGNORE`: a user preset that already shares a name keeps
-- its values, and this one-shot insert never re-seeds a built-in the user deleted.

-- Utility
INSERT OR IGNORE INTO eq_presets (name, bands) VALUES
  ('Bass Boost',     '6,5,4,2,0,0,0,0,0,0'),
  ('Bass Reducer',   '-6,-5,-4,-2,0,0,0,0,0,0'),
  ('Treble Boost',   '0,0,0,0,0,0,2,4,5,6'),
  ('Treble Reducer', '0,0,0,0,0,0,-2,-4,-5,-6'),
  ('Loudness',       '6,5,2,0,-1,-1,0,2,4,5'),
  ('Vocal Boost',    '-2,-2,-1,1,3,4,4,2,0,-1');

-- Spoken word (podcasts + audiobooks)
INSERT OR IGNORE INTO eq_presets (name, bands) VALUES
  ('Spoken Word',    '-4,-3,-1,0,2,3,4,3,1,-1'),
  ('Small Speakers', '-5,-3,1,2,1,0,1,2,2,0');

-- Genre
INSERT OR IGNORE INTO eq_presets (name, bands) VALUES
  ('Acoustic',       '4,4,3,1,1,1,2,3,3,2'),
  ('Classical',      '4,3,3,2,-1,-1,0,2,3,4'),
  ('Jazz',           '3,2,1,2,-1,-1,0,1,2,3'),
  ('Rock',           '4,3,2,0,-1,0,1,2,3,4'),
  ('Pop',            '-1,-1,0,2,4,4,2,0,-1,-2'),
  ('Electronic',     '4,4,2,0,-1,1,0,1,3,5'),
  ('Hip-Hop',        '5,4,2,3,-1,-1,1,2,2,3'),
  ('Dance',          '5,6,4,0,2,3,3,2,1,0');
