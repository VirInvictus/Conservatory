//! Playback profile resolution for music (spec §6.2, docs/libmpv-profiles.md).
//!
//! Pure: this module never touches libmpv. It turns a [`Track`] plus the user's
//! `[playback]` config (spec §10) into a [`MusicProfile`], which the host renders
//! into a labelled `af` filter chain ([`crate::player::chain`]) plus a couple of
//! flat mpv properties. Keeping it pure is what lets the resolution be unit-tested
//! headless (the CLAUDE.md rule: logic in core, the host is thin glue).
//!
//! Phase 5.5a turns ReplayGain into an explicit head-of-chain `volume` stage
//! (`replaygain_db`), recomputed per track, replacing mpv's built-in
//! `--replaygain` (which is applied *after* the `af` chain and inherits the first
//! track's gain across a gapless boundary, mpv bug #8267). Phase 6b-ii-c-3-a adds
//! per-show episode speed; the spoken-word `af` stages (Smart Speed, Voice Boost)
//! land at Phase 6c as presets on this chain.

#[cfg(test)]
use crate::db::models::InboxPolicy;
use crate::db::models::{AudioState, ShowSettings, Track};

/// Variable-speed bounds (spec §6.3, the podcast 1.2x–2x range plus headroom).
/// mpv accepts more, but a wild stored value should not produce unusable audio.
const MIN_SPEED: f64 = 0.25;
const MAX_SPEED: f64 = 4.0;

/// ReplayGain mode (spec §6.2): which stored gain to apply. The gain itself comes
/// from `tracks.replaygain_track` / `_album` (read from the file's RG tags at
/// import, or written by the rsgain scan, Phase 5c); the mode picks which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayGain {
    /// No normalization.
    Off,
    /// Per-track gain.
    Track,
    /// Per-album gain (preserves intra-album loudness relationships).
    Album,
}

impl ReplayGain {
    /// A display label (the CLI `debug-dsp` surface).
    pub fn as_str(self) -> &'static str {
        match self {
            ReplayGain::Off => "off",
            ReplayGain::Track => "track",
            ReplayGain::Album => "album",
        }
    }
}

/// The `[playback]` configuration (spec §10). The TOML loader / preferences UI is
/// Phase 10 / 5.5b, so this carries the values directly via [`Default`] for now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackConfig {
    /// Gapless playback within an album (mpv `--gapless-audio=weak` when on).
    pub gapless: bool,
    /// Requested ReplayGain mode; resolution downgrades it per available tags.
    pub replaygain: ReplayGain,
    /// A user gain offset added to the ReplayGain value, dB (Phase 5.5a). 0 = the
    /// scanned reference level.
    pub replaygain_preamp: f64,
    /// Prevent ReplayGain from clipping (Phase 5.5a). With no peak data stored,
    /// the safe clamp is attenuate-only: the net gain is capped at 0 dB so it can
    /// never push a sample over full scale. Off applies the raw gain + preamp
    /// (full normalization, may clip until the 5.5c brick-wall limiter).
    pub replaygain_clip: bool,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        // Spec §10 [playback] defaults.
        Self {
            gapless: true,
            replaygain: ReplayGain::Album,
            replaygain_preamp: 0.0,
            replaygain_clip: true,
        }
    }
}

impl PlaybackConfig {
    /// Build the playback config from the persisted [`AudioState`] singleton
    /// (Phase 5.5c-ii). This is the player-layer half of the db/player split: the
    /// db stores `replaygain_mode` as TEXT to stay free of this enum, and this is
    /// the one place the string becomes a [`ReplayGain`]. An unrecognized stored
    /// mode degrades to `Album` (the default), matching `get_audio_state`'s
    /// forgiving read. The DSP / output halves of `AudioState` are applied
    /// directly to the host, not through here.
    pub fn from_audio_state(state: &AudioState) -> Self {
        let replaygain = match state.replaygain_mode.as_str() {
            "off" => ReplayGain::Off,
            "track" => ReplayGain::Track,
            _ => ReplayGain::Album,
        };
        Self {
            gapless: state.gapless,
            replaygain,
            replaygain_preamp: state.replaygain_preamp,
            replaygain_clip: state.replaygain_clip,
        }
    }
}

/// The resolved playback profile for one item, ready to render into the libmpv
/// host's `af` chain + properties. The single per-item profile of spec §6.1
/// (named `MusicProfile` for now; episodes/audiobooks fill the spoken-word
/// fields). `speed` + `pitch_correction` drive mpv's `speed` /
/// `audio-pitch-correction` (scaletempo2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicProfile {
    pub gapless: bool,
    /// The ReplayGain head-stage gain in dB to apply (preamp-adjusted, clamped),
    /// or `None` for no normalization (Phase 5.5a). Rendered as a `volume` filter
    /// at the head of the `af` chain, recomputed per track (the #8267 fix).
    pub replaygain_db: Option<f64>,
    /// Playback rate (1.0 = native). Episodes resolve it from the show's
    /// `playback_speed`; music plays at 1.0.
    pub speed: f64,
    /// Keep pitch constant when `speed != 1.0` (mpv `audio-pitch-correction`,
    /// scaletempo2). On for spoken word, off for music (it is a no-op at 1.0).
    pub pitch_correction: bool,
    /// Smart Speed (Phase 6c, spoken word): remove dead air via the `@ss`
    /// `silenceremove` stage. Always false for music; for episodes it follows the
    /// show's `smart_speed` setting.
    pub smart_speed: bool,
    /// Voice Boost (Phase 6c, spoken word): the compressor + voice-band EQ +
    /// leveler preset (the `@vb*` stages), tuned to make uneven spoken audio
    /// intelligible. Always false for music; for episodes it follows the show's
    /// `voice_boost` setting.
    pub voice_boost: bool,
}

/// Resolve which stored gain (dB) ReplayGain should apply for `track` under the
/// mode, downgrading to what the track actually carries: an album-mode request on
/// a track with only track gain falls back to track gain, and a track with no RG
/// tags resolves to `None` (no normalization against absent data). The read-only
/// stance (spec §16.7): we consult the tags import / the rsgain scan stored,
/// never invent a value.
fn resolve_replaygain_raw(track: &Track, mode: ReplayGain) -> Option<f64> {
    match mode {
        ReplayGain::Off => None,
        ReplayGain::Album => track.replaygain_album.or(track.replaygain_track),
        ReplayGain::Track => track.replaygain_track.or(track.replaygain_album),
    }
}

/// Resolve the music profile for `track` under `cfg`. The ReplayGain gain is the
/// stored value plus the preamp, then clamped to ≤ 0 dB when `replaygain_clip`
/// (the no-peak-data clip guard, Phase 5.5a).
pub fn resolve_music_profile(track: &Track, cfg: &PlaybackConfig) -> MusicProfile {
    let replaygain_db = resolve_replaygain_raw(track, cfg.replaygain).map(|raw| {
        let net = raw + cfg.replaygain_preamp;
        if cfg.replaygain_clip {
            net.min(0.0)
        } else {
            net
        }
    });

    MusicProfile {
        gapless: cfg.gapless,
        replaygain_db,
        speed: 1.0,
        pitch_correction: false,
        smart_speed: false,
        voice_boost: false,
    }
}

/// A spoken-word profile for episode playback: no ReplayGain (podcasts carry
/// none) and no gapless (episodes are single items), with per-show variable
/// speed resolved from the show's settings (Phase 6b-ii-c-3-a). `settings` is
/// `None` for a show with no overrides; the stored speed is clamped to
/// `[MIN_SPEED, MAX_SPEED]` so a bad value never yields unusable audio. Pitch
/// correction is on so faster speech stays natural.
///
/// Smart Speed and Voice Boost (Phase 6c) follow the show's flags, driving the
/// `@ss` / `@vb*` spoken-word stages of the chain. A show with **no** settings
/// row resolves both to `false` (conservative: the feature applies only to shows
/// the user has explicitly configured; the settings dialog defaults Smart Speed
/// on, so saving a show's settings opts it in).
pub fn resolve_episode_profile(settings: Option<&ShowSettings>) -> MusicProfile {
    let speed = settings
        .map(|s| s.playback_speed)
        .unwrap_or(1.0)
        .clamp(MIN_SPEED, MAX_SPEED);
    MusicProfile {
        gapless: false,
        replaygain_db: None,
        speed,
        pitch_correction: true,
        smart_speed: settings.is_some_and(|s| s.smart_speed),
        voice_boost: settings.is_some_and(|s| s.voice_boost),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// A bare track with both RG fields unset; tests set what they need.
    fn track() -> Track {
        Track {
            id: 1,
            album_id: Some(1),
            artist_id: Some(1),
            title: "t".into(),
            track_no: Some(1),
            disc_no: Some(1),
            duration: Some(120.0),
            file_path: "x.flac".into(),
            format: Some("flac".into()),
            bitrate: Some(1000),
            sample_rate: Some(44100),
            replaygain_track: None,
            replaygain_album: None,
            rating: 0,
            play_count: 0,
            last_played: None,
            starred: false,
            musicbrainz_recording_id: None,
            added_at: Some(Utc::now()),
        }
    }

    #[test]
    fn gapless_passes_through() {
        let cfg = PlaybackConfig {
            gapless: false,
            ..PlaybackConfig::default()
        };
        assert!(!resolve_music_profile(&track(), &cfg).gapless);
    }

    #[test]
    fn off_stays_off_even_with_tags() {
        let mut t = track();
        t.replaygain_album = Some(-7.0);
        let cfg = PlaybackConfig {
            replaygain: ReplayGain::Off,
            ..PlaybackConfig::default()
        };
        assert_eq!(resolve_music_profile(&t, &cfg).replaygain_db, None);
    }

    #[test]
    fn album_request_uses_album_gain_when_present() {
        let mut t = track();
        t.replaygain_album = Some(-7.0);
        t.replaygain_track = Some(-6.0);
        let cfg = PlaybackConfig::default(); // album, preamp 0, clip on
        assert_eq!(resolve_music_profile(&t, &cfg).replaygain_db, Some(-7.0));
    }

    #[test]
    fn album_request_falls_back_to_track_gain() {
        let mut t = track();
        t.replaygain_track = Some(-6.0); // album absent
        let cfg = PlaybackConfig::default();
        assert_eq!(resolve_music_profile(&t, &cfg).replaygain_db, Some(-6.0));
    }

    #[test]
    fn track_request_falls_back_to_album_gain() {
        let mut t = track();
        t.replaygain_album = Some(-7.0); // track absent
        let cfg = PlaybackConfig {
            replaygain: ReplayGain::Track,
            ..PlaybackConfig::default()
        };
        assert_eq!(resolve_music_profile(&t, &cfg).replaygain_db, Some(-7.0));
    }

    #[test]
    fn no_tags_resolves_to_none() {
        let cfg = PlaybackConfig::default(); // album requested
        assert_eq!(resolve_music_profile(&track(), &cfg).replaygain_db, None);
    }

    #[test]
    fn preamp_adds_then_clip_clamps_to_zero() {
        let mut t = track();
        t.replaygain_album = Some(-3.0);
        // Preamp +6 → net +3; clip on clamps to 0.
        let cfg = PlaybackConfig {
            replaygain_preamp: 6.0,
            replaygain_clip: true,
            ..PlaybackConfig::default()
        };
        assert_eq!(resolve_music_profile(&t, &cfg).replaygain_db, Some(0.0));

        // A net-negative result is untouched by the clamp.
        let cfg2 = PlaybackConfig {
            replaygain_preamp: 1.0,
            ..PlaybackConfig::default()
        };
        assert_eq!(resolve_music_profile(&t, &cfg2).replaygain_db, Some(-2.0));
    }

    #[test]
    fn clip_off_allows_positive_boost() {
        let mut t = track();
        t.replaygain_album = Some(-3.0);
        let cfg = PlaybackConfig {
            replaygain_preamp: 6.0,
            replaygain_clip: false,
            ..PlaybackConfig::default()
        };
        assert_eq!(resolve_music_profile(&t, &cfg).replaygain_db, Some(3.0));
    }

    #[test]
    fn mode_label() {
        assert_eq!(ReplayGain::Off.as_str(), "off");
        assert_eq!(ReplayGain::Track.as_str(), "track");
        assert_eq!(ReplayGain::Album.as_str(), "album");
    }

    #[test]
    fn music_plays_at_native_speed() {
        let p = resolve_music_profile(&track(), &PlaybackConfig::default());
        assert_eq!(p.speed, 1.0);
        assert!(!p.pitch_correction);
    }

    fn show_settings(speed: f64) -> ShowSettings {
        ShowSettings {
            show_id: 1,
            playback_speed: speed,
            smart_speed: true,
            voice_boost: false,
            skip_intro: 0,
            skip_outro: 0,
            skip_forward: None,
            skip_back: None,
            inbox_policy: InboxPolicy::Inbox,
        }
    }

    #[test]
    fn episode_speed_resolves_from_show_settings() {
        let p = resolve_episode_profile(Some(&show_settings(1.5)));
        assert_eq!(p.speed, 1.5);
        assert!(p.pitch_correction, "spoken word keeps pitch correction on");
        assert_eq!(p.replaygain_db, None);
        assert!(!p.gapless);
    }

    #[test]
    fn episode_speed_defaults_to_one_without_settings() {
        assert_eq!(resolve_episode_profile(None).speed, 1.0);
    }

    #[test]
    fn episode_speed_is_clamped() {
        assert_eq!(
            resolve_episode_profile(Some(&show_settings(99.0))).speed,
            MAX_SPEED
        );
        assert_eq!(
            resolve_episode_profile(Some(&show_settings(0.0))).speed,
            MIN_SPEED
        );
    }

    #[test]
    fn playback_config_maps_from_audio_state() {
        // Each stored mode maps to the enum; an unknown mode degrades to Album
        // (the forgiving read), and the scalar fields carry through verbatim.
        let mut state = AudioState {
            replaygain_mode: "off".to_string(),
            replaygain_preamp: -3.0,
            replaygain_clip: false,
            gapless: false,
            ..AudioState::default()
        };
        let cfg = PlaybackConfig::from_audio_state(&state);
        assert_eq!(cfg.replaygain, ReplayGain::Off);
        assert_eq!(cfg.replaygain_preamp, -3.0);
        assert!(!cfg.replaygain_clip);
        assert!(!cfg.gapless);

        state.replaygain_mode = "track".to_string();
        assert_eq!(
            PlaybackConfig::from_audio_state(&state).replaygain,
            ReplayGain::Track
        );
        state.replaygain_mode = "album".to_string();
        assert_eq!(
            PlaybackConfig::from_audio_state(&state).replaygain,
            ReplayGain::Album
        );
        state.replaygain_mode = "nonsense".to_string();
        assert_eq!(
            PlaybackConfig::from_audio_state(&state).replaygain,
            ReplayGain::Album
        );
    }
}
