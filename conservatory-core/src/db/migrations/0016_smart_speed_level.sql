-- Phase 6c follow-on: the global Smart Speed aggressiveness level. The per-show /
-- per-book Smart Speed on/off stays in the show / book settings; this is the one
-- gate that applies wherever Smart Speed is on ('gentle' | 'balanced' |
-- 'aggressive'). Stored on the audio_state singleton next to the DSP settings (it
-- is an af-chain parameter). The DEFAULT backfills the existing row, so the retune
-- shipped in v0.1.1 (the 'gentle' gate) stays the effective default.
ALTER TABLE audio_state ADD COLUMN smart_speed_level TEXT NOT NULL DEFAULT 'gentle';
