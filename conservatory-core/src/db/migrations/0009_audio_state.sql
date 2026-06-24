-- Phase 5.5c: the audio engine's active configuration (spec §6.2, §6.5,
-- docs/libmpv-profiles.md). A single row holding the playback defaults
-- (ReplayGain mode / preamp / clip, gapless), the DSP modules (compressor,
-- brick-wall limiter, dynaudnorm leveler — each an `enabled` flag plus its
-- parameters, which persist while the module is off so a toggle restores them),
-- and the output backend / resampler. The `eq_state` precedent: one singleton
-- row (id = 0), read by `get_audio_state`, overwritten by `set_audio_state`. The
-- seeded defaults match `PlaybackConfig::default()` + all DSP off + auto output.
-- The DSP + output halves land at 5.5c-i / 5.5c-ii; the playback defaults are
-- consumed at 5.5c-ii (the queue builders read them instead of the hardcoded
-- default), so they ship here to avoid a second migration.
CREATE TABLE audio_state (
    id                  INTEGER PRIMARY KEY CHECK (id = 0),
    replaygain_mode     TEXT    NOT NULL,   -- 'off' | 'track' | 'album'
    replaygain_preamp   REAL    NOT NULL,   -- dB
    replaygain_clip     INTEGER NOT NULL,   -- bool
    gapless             INTEGER NOT NULL,   -- bool
    comp_enabled        INTEGER NOT NULL,   -- bool
    comp_threshold_db   REAL    NOT NULL,
    comp_ratio          REAL    NOT NULL,
    comp_attack_ms      REAL    NOT NULL,
    comp_release_ms     REAL    NOT NULL,
    limiter_enabled     INTEGER NOT NULL,   -- bool
    limiter_ceiling_db  REAL    NOT NULL,
    leveler_enabled     INTEGER NOT NULL,   -- bool
    leveler_target_peak REAL    NOT NULL,
    leveler_gausssize   INTEGER NOT NULL,
    output_backend      TEXT    NOT NULL,   -- 'auto' | 'pipewire' | 'pulse' | 'alsa' | 'jack'
    resampler_quality   TEXT    NOT NULL    -- 'default' | 'high'
);

INSERT INTO audio_state (
    id, replaygain_mode, replaygain_preamp, replaygain_clip, gapless,
    comp_enabled, comp_threshold_db, comp_ratio, comp_attack_ms, comp_release_ms,
    limiter_enabled, limiter_ceiling_db,
    leveler_enabled, leveler_target_peak, leveler_gausssize,
    output_backend, resampler_quality
) VALUES (
    0, 'album', 0.0, 1, 1,
    0, -18.0, 3.0, 20.0, 250.0,
    0, -1.0,
    0, 0.95, 31,
    'auto', 'default'
);
