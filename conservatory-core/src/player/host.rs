//! The libmpv playback host (spec §6, docs/libmpv-profiles.md).
//!
//! A single libmpv instance kept alive across items (the `libmpv2` binding,
//! property API plus the input-command layer). This is the thin glue the rest
//! of the player is built to keep small: the profile resolution (`profile.rs`)
//! and the persistence logic (`state.rs`) are pure and tested headless; this
//! file is the part that actually talks to libmpv and so is exercised by the
//! CLI `play` verb and an `ao=null` smoke test rather than unit tests.
//!
//! Phase 4a drives the host directly from a single loop (the CLI). The threaded
//! `Player` handle, the unified queue, and the Now-bar transport (the GTK
//! consumer that needs cross-thread commands) land at Phase 4b; building that
//! plumbing now, with no second consumer, would be speculative.

use libmpv2::events::Event;
use libmpv2::{EndFileReason, Mpv, mpv_end_file_reason};

use crate::errors::{Error, Result};
use crate::player::profile::MusicProfile;
use crate::player::state::EndReason;

/// What a single [`MpvHost::pump`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEvent {
    /// The current item stopped; the reason is mapped to core's [`EndReason`].
    Ended(EndReason),
    /// libmpv is shutting down.
    Shutdown,
    /// Nothing notable this pump (a timeout, or an event we don't act on).
    Idle,
}

/// Owns the one libmpv instance. Not `Send` (libmpv's handle is not), which is
/// fine: the 4a consumer is a single-threaded CLI loop. Phase 4b moves it onto
/// a dedicated thread behind a command channel.
pub struct MpvHost {
    mpv: Mpv,
}

impl MpvHost {
    /// Build a host with the default audio output (real playback).
    pub fn new() -> Result<Self> {
        Self::build(false)
    }

    /// Build a host with a null audio output, for headless tests and CI: it
    /// decodes and advances exactly like the real path but produces no sound,
    /// so the event flow (load → end-of-file) can be asserted without a sound
    /// server.
    pub fn new_null() -> Result<Self> {
        Self::build(true)
    }

    fn build(silent: bool) -> Result<Self> {
        let mpv = Mpv::with_initializer(|init| {
            // Audio-only: no video output window even for files with embedded
            // cover art (mpv would otherwise treat the picture as a video).
            init.set_property("vo", "null")?;
            init.set_property("vid", "no")?;
            if silent {
                init.set_property("ao", "null")?;
            }
            Ok(())
        })
        .map_err(|e| Error::Player(format!("initializing libmpv: {e}")))?;
        Ok(Self { mpv })
    }

    /// Apply `profile` and start playing `path`. The profile properties are set
    /// before the load so they take effect for this item (spec §6.1: the engine
    /// applies the item's profile before playing).
    pub fn load(&mut self, path: &str, profile: &MusicProfile) -> Result<()> {
        self.mpv
            .set_property("gapless-audio", profile.gapless)
            .map_err(|e| Error::Player(format!("setting gapless-audio: {e}")))?;
        self.mpv
            .set_property("replaygain", profile.replaygain.as_mpv())
            .map_err(|e| Error::Player(format!("setting replaygain: {e}")))?;
        // libmpv2's `command` builds a single command *string* (no array form),
        // so an unescaped path with spaces would split into multiple args. mpv's
        // command parser reads a double-quoted token (with backslash escapes) as
        // one literal argument, which is how we pass an arbitrary path through
        // command_string safely.
        self.mpv
            .command("loadfile", &[&quote_arg(path)])
            .map_err(|e| Error::Player(format!("loadfile: {e}")))?;
        Ok(())
    }

    /// Pause or resume.
    pub fn set_paused(&mut self, paused: bool) -> Result<()> {
        self.mpv
            .set_property("pause", paused)
            .map_err(|e| Error::Player(format!("setting pause: {e}")))
    }

    /// Set the output volume (0–100, the schema's range; mpv itself allows more).
    pub fn set_volume(&mut self, volume: i64) -> Result<()> {
        self.mpv
            .set_property("volume", volume as f64)
            .map_err(|e| Error::Player(format!("setting volume: {e}")))
    }

    /// Seek to an absolute offset in seconds (the resume path, spec §6.4).
    pub fn seek_absolute(&mut self, secs: f64) -> Result<()> {
        self.mpv
            .command("seek", &[&format!("{secs}"), "absolute"])
            .map_err(|e| Error::Player(format!("seek: {e}")))
    }

    /// Current playback position in seconds, if known yet. `None` before the
    /// first frame is decoded (the property is briefly unavailable on load).
    pub fn time_pos(&self) -> Option<f64> {
        self.mpv.get_property::<f64>("time-pos").ok()
    }

    /// The current item's total duration in seconds, if known.
    pub fn duration(&self) -> Option<f64> {
        self.mpv.get_property::<f64>("duration").ok()
    }

    /// Wait up to `timeout` seconds for the next libmpv event, mapping it to a
    /// [`HostEvent`]. The caller polls position between pumps; only the end and
    /// shutdown transitions need to be acted on, so everything else is `Idle`.
    pub fn pump(&mut self, timeout: f64) -> HostEvent {
        match self.mpv.event_context_mut().wait_event(timeout) {
            Some(Ok(Event::EndFile(reason))) => HostEvent::Ended(map_end_reason(reason)),
            Some(Ok(Event::Shutdown)) => HostEvent::Shutdown,
            // libmpv surfaces an errored end-of-file (an unplayable or missing
            // file) as an event *error* rather than an `EndFile` event. Treat it
            // as an errored end so the loop stops instead of waiting forever.
            Some(Err(_)) => HostEvent::Ended(EndReason::Errored),
            // Other events (StartFile, FileLoaded, Seek, property changes) carry
            // no decision for the 4a loop.
            Some(Ok(_)) | None => HostEvent::Idle,
        }
    }
}

/// Map libmpv's end-of-file reason onto core's own [`EndReason`], so nothing
/// outside this module depends on the libmpv enum.
fn map_end_reason(reason: EndFileReason) -> EndReason {
    match reason {
        mpv_end_file_reason::Eof => EndReason::Eof,
        mpv_end_file_reason::Stop => EndReason::Stopped,
        mpv_end_file_reason::Error => EndReason::Errored,
        mpv_end_file_reason::Quit => EndReason::Quit,
        mpv_end_file_reason::Redirect => EndReason::Redirect,
        _ => EndReason::Stopped,
    }
}

/// Wrap `s` in a double-quoted mpv command token, backslash-escaping the two
/// characters the quoted-string parser treats specially (`"` and `\`), so an
/// arbitrary path passes through `command_string` as one literal argument.
fn quote_arg(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_escapes_paths() {
        assert_eq!(quote_arg("/m/a b.mp3"), "\"/m/a b.mp3\"");
        assert_eq!(quote_arg(r#"/m/a"b.mp3"#), r#""/m/a\"b.mp3""#);
        assert_eq!(quote_arg(r"/m/a\b.mp3"), r#""/m/a\\b.mp3""#);
    }
}
