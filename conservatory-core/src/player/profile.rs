//! Playback profile resolution for music (spec §6.2, docs/libmpv-profiles.md).
//!
//! Pure: this module never touches libmpv. It turns a [`Track`] plus the user's
//! `[playback]` config (spec §10) into a [`MusicProfile`] the host applies as
//! mpv properties. Keeping it pure is what lets the resolution be unit-tested
//! headless (the CLAUDE.md rule: logic in core, the host is thin glue).
//!
//! Phase 4a covers the music profile only. The podcast/audiobook spoken-word
//! profile (Smart Speed, Voice Boost) lands with the absorbed Belfry engine at
//! Phase 6c and reuses none of this.

use crate::db::models::Track;

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

/// The resolved music profile for one item, ready to apply to the libmpv host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicProfile {
    pub gapless: bool,
    pub replaygain: ReplayGain,
    pub crossfade_seconds: u32,
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
}
