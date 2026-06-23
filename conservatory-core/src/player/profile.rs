//! Playback profile resolution for music (spec §6.2, docs/libmpv-profiles.md).
//!
//! Pure: this module never touches libmpv. It turns a [`Track`] plus the user's
//! `[playback]` config (spec §10) into a [`MusicProfile`] the host applies as
//! mpv properties. Keeping it pure is what lets the resolution be unit-tested
//! headless (the CLAUDE.md rule: logic in core, the host is thin glue).
//!
//! Phase 4a covers the music profile only. Phase 6b-ii-c-3-a adds per-show
//! playback speed for episodes (mpv `speed` + `audio-pitch-correction`); the
//! spoken-word `af` chain (Smart Speed, Voice Boost) lands at Phase 6c.

#[cfg(test)]
use crate::db::models::InboxPolicy;
use crate::db::models::{ShowSettings, Track};

/// Variable-speed bounds (spec §6.3, the podcast 1.2x–2x range plus headroom).
/// mpv accepts more, but a wild stored value should not produce unusable audio.
const MIN_SPEED: f64 = 0.25;
const MAX_SPEED: f64 = 4.0;

/// ReplayGain mode (spec §6.2). mpv applies the gain from the file's own RG
/// tags (the same tags `lofty` read into the DB at import); we choose the mode.
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
    /// The mpv `replaygain` property value.
    pub fn as_mpv(self) -> &'static str {
        match self {
            ReplayGain::Off => "no",
            ReplayGain::Track => "track",
            ReplayGain::Album => "album",
        }
    }
}

/// The `[playback]` configuration (spec §10). Phase 4a uses the defaults; the
/// TOML loader is Phase 10, so this carries the values directly for now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackConfig {
    /// Gapless playback within an album (`--gapless-audio`).
    pub gapless: bool,
    /// Requested ReplayGain mode; resolution downgrades it per available tags.
    pub replaygain: ReplayGain,
    /// Crossfade between non-gapless tracks, seconds. 0 = off (the default).
    /// Rendered at Phase 4b with the queue; 4a only carries it through.
    pub crossfade_seconds: u32,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        // Spec §10 [playback] defaults.
        Self {
            gapless: true,
            replaygain: ReplayGain::Album,
            crossfade_seconds: 0,
        }
    }
}

/// The resolved playback profile for one item, ready to apply to the libmpv
/// host. The single per-item profile of spec §6.1 (named `MusicProfile` for now;
/// episodes/audiobooks fill the spoken-word fields). `speed` + `pitch_correction`
/// drive mpv's `speed` / `audio-pitch-correction` (scaletempo2): per-show speed
/// for episodes (Phase 6b-ii-c-3-a), native speed for music.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicProfile {
    pub gapless: bool,
    pub replaygain: ReplayGain,
    pub crossfade_seconds: u32,
    /// Playback rate (1.0 = native). Episodes resolve it from the show's
    /// `playback_speed`; music plays at 1.0.
    pub speed: f64,
    /// Keep pitch constant when `speed != 1.0` (mpv `audio-pitch-correction`,
    /// scaletempo2). On for spoken word, off for music (it is a no-op at 1.0).
    pub pitch_correction: bool,
}

/// Resolve the music profile for `track` under `cfg`.
///
/// ReplayGain is downgraded to what the track actually carries: an album-mode
/// request on a track with only track gain falls back to track mode, and a
/// track with no RG tags at all resolves to `Off` rather than asking mpv to
/// normalize against absent data. This is the read-only stance settled for 4a
/// (spec §16.7): we never scan, only consult the tags import already stored.
pub fn resolve_music_profile(track: &Track, cfg: &PlaybackConfig) -> MusicProfile {
    let has_album = track.replaygain_album.is_some();
    let has_track = track.replaygain_track.is_some();

    let replaygain = match cfg.replaygain {
        ReplayGain::Off => ReplayGain::Off,
        ReplayGain::Album => {
            if has_album {
                ReplayGain::Album
            } else if has_track {
                ReplayGain::Track
            } else {
                ReplayGain::Off
            }
        }
        ReplayGain::Track => {
            if has_track {
                ReplayGain::Track
            } else if has_album {
                ReplayGain::Album
            } else {
                ReplayGain::Off
            }
        }
    };

    MusicProfile {
        gapless: cfg.gapless,
        replaygain,
        crossfade_seconds: cfg.crossfade_seconds,
        speed: 1.0,
        pitch_correction: false,
    }
}

/// A spoken-word profile for episode playback: no ReplayGain (podcasts carry
/// none) and no gapless (episodes are single items), with per-show variable
/// speed resolved from the show's settings (Phase 6b-ii-c-3-a). `settings` is
/// `None` for a show with no overrides (the schema default 1.0). The stored
/// speed is clamped to `[MIN_SPEED, MAX_SPEED]` so a bad value never yields
/// unusable audio. Pitch correction is on so faster speech stays natural. The
/// Smart Speed / Voice Boost `af` chain (and the `smart_speed`/`voice_boost`
/// flags those consume) is Phase 6c.
pub fn resolve_episode_profile(settings: Option<&ShowSettings>) -> MusicProfile {
    let speed = settings
        .map(|s| s.playback_speed)
        .unwrap_or(1.0)
        .clamp(MIN_SPEED, MAX_SPEED);
    MusicProfile {
        gapless: false,
        replaygain: ReplayGain::Off,
        crossfade_seconds: 0,
        speed,
        pitch_correction: true,
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
    fn gapless_and_crossfade_pass_through() {
        let cfg = PlaybackConfig {
            gapless: false,
            replaygain: ReplayGain::Off,
            crossfade_seconds: 7,
        };
        let p = resolve_music_profile(&track(), &cfg);
        assert!(!p.gapless);
        assert_eq!(p.crossfade_seconds, 7);
    }

    #[test]
    fn off_stays_off_even_with_tags() {
        let mut t = track();
        t.replaygain_album = Some(-7.0);
        let cfg = PlaybackConfig {
            replaygain: ReplayGain::Off,
            ..PlaybackConfig::default()
        };
        assert_eq!(resolve_music_profile(&t, &cfg).replaygain, ReplayGain::Off);
    }

    #[test]
    fn album_request_uses_album_gain_when_present() {
        let mut t = track();
        t.replaygain_album = Some(-7.0);
        t.replaygain_track = Some(-6.0);
        let cfg = PlaybackConfig::default(); // album
        assert_eq!(
            resolve_music_profile(&t, &cfg).replaygain,
            ReplayGain::Album
        );
    }

    #[test]
    fn album_request_falls_back_to_track_gain() {
        let mut t = track();
        t.replaygain_track = Some(-6.0); // album absent
        let cfg = PlaybackConfig::default(); // album
        assert_eq!(
            resolve_music_profile(&t, &cfg).replaygain,
            ReplayGain::Track
        );
    }

    #[test]
    fn track_request_falls_back_to_album_gain() {
        let mut t = track();
        t.replaygain_album = Some(-7.0); // track absent
        let cfg = PlaybackConfig {
            replaygain: ReplayGain::Track,
            ..PlaybackConfig::default()
        };
        assert_eq!(
            resolve_music_profile(&t, &cfg).replaygain,
            ReplayGain::Album
        );
    }

    #[test]
    fn no_tags_resolves_to_off() {
        let cfg = PlaybackConfig::default(); // album requested
        assert_eq!(
            resolve_music_profile(&track(), &cfg).replaygain,
            ReplayGain::Off
        );
    }

    #[test]
    fn mode_maps_to_mpv_string() {
        assert_eq!(ReplayGain::Off.as_mpv(), "no");
        assert_eq!(ReplayGain::Track.as_mpv(), "track");
        assert_eq!(ReplayGain::Album.as_mpv(), "album");
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
        assert_eq!(p.replaygain, ReplayGain::Off);
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
}
