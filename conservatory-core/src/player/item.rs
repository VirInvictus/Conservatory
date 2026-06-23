//! The resolved playable unit (spec §6.1).
//!
//! A [`PlayableItem`] is what the engine actually plays: a source path plus the
//! profile to apply before playing it. The consumer (CLI / GTK) resolves these
//! from the DB (read pool + [`crate::player::resolve_music_profile`]) and hands
//! a `Vec<PlayableItem>` to the player, so the engine itself does no DB reads.
//!
//! Phase 4b is music-only: every item is a `Track`. The `kind` is carried so the
//! queue view can badge rows and so episodes/audiobooks slot in at Phases 6/7.

use std::path::PathBuf;

use crate::db::MediaKind;
use crate::player::profile::MusicProfile;

/// One queue entry, resolved to something the libmpv host can load and play.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayableItem {
    pub track_id: i64,
    pub source: PathBuf,
    pub profile: MusicProfile,
    pub album_id: Option<i64>,
    pub kind: MediaKind,
    /// The source is a remote URL streamed over the network (an undownloaded
    /// episode), not a local file. Drives the Now-bar streaming glyph (v0.0.38).
    pub streaming: bool,
}
