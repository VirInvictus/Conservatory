-- Phase 17a/17b: the transport play-order modes, persisted so they survive a
-- restart (the Spotify / Apple Music convention). Both live on the audio_state
-- singleton next to the ReplayGain / gapless playback defaults, the same
-- DB-canonical home the engine already reads (spec §6.2); `repeat` is TEXT the
-- `replaygain_mode` idiom, `shuffle` is a bool. The DEFAULTs backfill the
-- existing row to "off", so an upgrade lands with both modes disabled.
--
-- `shuffle` is in-place (Phase 17b): enabling it physically reorders the
-- upcoming queue tail, so the queue view stays the play order; the flag persists
-- so a later Play / repeat-all lap keeps honouring it.
ALTER TABLE audio_state ADD COLUMN repeat  TEXT    NOT NULL DEFAULT 'off';  -- 'off' | 'all' | 'one'
ALTER TABLE audio_state ADD COLUMN shuffle INTEGER NOT NULL DEFAULT 0;      -- bool
