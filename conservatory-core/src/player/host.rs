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
use libmpv2::mpv_node::MpvNode;
use libmpv2::{EndFileReason, Mpv, mpv_end_file_reason};

use crate::db::models::{DspState, EqState, ResamplerQuality};
use crate::errors::{Error, Result};
use crate::player::profile::MusicProfile;
use crate::player::spoken::SmartSpeedLevel;
use crate::player::state::EndReason;

/// An audio output device mpv can play to (spec §6.5, the output-sink picker).
/// `name` is mpv's device id (e.g. `pipewire/…`, or `auto`); `description` is the
/// human label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub name: String,
    pub description: String,
}

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
    /// The active equalizer (Phase 5.5b), applied into the `af` chain on each
    /// load. Defaults to flat (no `@eq` stage); the engine updates it via
    /// [`MpvHost::set_eq`] / [`MpvHost::set_eq_band`].
    eq: EqState,
    /// The active DSP modules (Phase 5.5c: compressor / limiter / leveler),
    /// applied into the `af` chain on each load. Defaults to off (no dynamics
    /// stages); the engine updates it via [`MpvHost::set_dsp`].
    dsp: DspState,
    /// The global Smart Speed aggressiveness (Phase 6c follow-on), folded into the
    /// `@ss` gate whenever a spoken-word item has Smart Speed on. The engine
    /// updates it via [`MpvHost::set_smart_speed_level`]; the per-item on/off stays
    /// in the show / book settings.
    smart_speed_level: SmartSpeedLevel,
    /// The currently-loaded item's profile (Phase 5.5b-ii), kept so the `af`
    /// chain can be rebuilt mid-playback on an EQ change. `None` when nothing is
    /// loaded (an EQ change then just updates state, applied on the next load).
    current_profile: Option<MusicProfile>,
    /// The active output backend (Phase 5.5c-ii, spec §6.5): mpv's `ao` driver,
    /// e.g. `auto` / `pipewire` / `pulse` / `alsa` / `jack`. Defaults to `auto`
    /// (mpv's own driver autoprobe); the engine updates it via
    /// [`MpvHost::set_output_backend`].
    output_backend: String,
    /// The active resampler quality (Phase 5.5c-ii, spec §6.5). `Default` leaves
    /// mpv's resampler alone; `High` raises the `audio-resample-*` knobs for the
    /// unavoidable-resample case. Re-asserted on each load.
    resampler: ResamplerQuality,
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
        // libmpv's `mpv_create()` returns NULL unless `LC_NUMERIC` is "C", and a
        // GTK app sets the locale from the environment at startup (the CLI never
        // does, which is why it played but the GUI didn't). Force it here, at the
        // libmpv boundary, so every consumer is covered. Idempotent and harmless.
        unsafe {
            libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
        }
        let mpv = Mpv::with_initializer(|init| {
            // Audio-only: no video output window even for files with embedded
            // cover art (mpv would otherwise treat the picture as a video).
            init.set_property("vo", "null")?;
            init.set_property("vid", "no")?;
            // A stable client name so the visualizer can target *our* output node
            // on PipeWire (the spectrum taps this node specifically, not the whole
            // sink, so other apps' audio does not move the bars). Surfaces as the
            // node's application.name; see `crate::player::AUDIO_CLIENT_NAME`.
            init.set_property("audio-client-name", crate::player::AUDIO_CLIENT_NAME)?;
            if silent {
                init.set_property("ao", "null")?;
            }
            Ok(())
        })
        .map_err(|e| Error::Player(format!("initializing libmpv: {e}")))?;
        Ok(Self {
            mpv,
            eq: EqState::flat(),
            dsp: DspState::off(),
            smart_speed_level: SmartSpeedLevel::default(),
            current_profile: None,
            output_backend: "auto".to_string(),
            resampler: ResamplerQuality::Default,
        })
    }

    /// Set the whole active equalizer (Phase 5.5b): a preset switch / launch
    /// state. Applied immediately when playing (a structural `af` rebuild — an
    /// explicit settings change, gap-acceptable per docs/libmpv-profiles.md),
    /// else stored for the next [`MpvHost::load`].
    pub fn set_eq(&mut self, eq: EqState) {
        self.eq = eq;
        let _ = self.rebuild_af();
    }

    /// Set the active DSP modules (Phase 5.5c: compressor / limiter / leveler).
    /// Applied immediately when playing (a structural `af` rebuild — an explicit
    /// settings change, gap-acceptable per docs/libmpv-profiles.md; DSP has no
    /// per-slider live path like the EQ), else stored for the next
    /// [`MpvHost::load`].
    pub fn set_dsp(&mut self, dsp: DspState) {
        self.dsp = dsp;
        let _ = self.rebuild_af();
    }

    /// Set the global Smart Speed aggressiveness (Phase 6c follow-on). Applied live
    /// when playing (a structural `af` rebuild, the `set_dsp` shape), else stored
    /// for the next load. Only affects a spoken-word item with Smart Speed on.
    pub fn set_smart_speed_level(&mut self, level: SmartSpeedLevel) {
        self.smart_speed_level = level;
        let _ = self.rebuild_af();
    }

    /// Apply spoken-word settings (speed / Smart Speed / Voice Boost) to the item
    /// playing now (Phase 6c): update the stored profile so the af rebuild reflects
    /// them, set mpv `speed` live, and rebuild the `af` chain for the `@ss` / `@vb`
    /// stages. A no-op with nothing loaded, so it never leaks a podcast's speed
    /// onto the next music track (that item resolves its own profile at load).
    pub fn set_spoken(&mut self, speed: f64, smart_speed: bool, voice_boost: bool) {
        let Some(profile) = self.current_profile.as_mut() else {
            return;
        };
        profile.speed = speed;
        profile.smart_speed = smart_speed;
        profile.voice_boost = voice_boost;
        let _ = self.mpv.set_property("speed", speed);
        let _ = self.rebuild_af();
    }

    /// Set one EQ band's gain (Phase 5.5b-ii). The common case (the `@eq` stage
    /// already present, staying non-flat) mutates that band **live and gap-free**
    /// via `af-command`. Crossing the flat↔non-flat boundary (the `@eq` stage
    /// must appear or disappear) does a one-time structural rebuild. Not playing:
    /// just store, applied at the next load.
    pub fn set_eq_band(&mut self, index: usize, gain: f64) -> Result<()> {
        if index >= self.eq.bands.len() {
            return Ok(());
        }
        let was_flat = self.eq.is_flat();
        self.eq.bands[index] = gain;
        let now_flat = self.eq.is_flat();
        if self.current_profile.is_none() {
            return Ok(());
        }
        if was_flat != now_flat {
            self.rebuild_af()
        } else if !now_flat {
            let (label, cmd, arg, target) = crate::player::chain::eq_band_command(index, gain);
            self.af_command(label, cmd, &arg, &target)
        } else {
            Ok(())
        }
    }

    /// Send a runtime command to a labelled `af` filter (Phase 5.5b-ii). libmpv2's
    /// `command` joins the args into one string, so this becomes
    /// `af-command <label> <cmd> <arg> <target>`; `target` selects the filter
    /// instance within the `@<label>` lavfi graph (e.g. an EQ band `b3`).
    pub fn af_command(&self, label: &str, cmd: &str, arg: &str, target: &str) -> Result<()> {
        self.mpv
            .command("af-command", &[label, cmd, arg, target])
            .map_err(|e| Error::Player(format!("af-command {label} {cmd}: {e}")))
    }

    /// Re-set the `af` property from the current item's profile + the active EQ
    /// (the structural path). A no-op when nothing is loaded.
    fn rebuild_af(&self) -> Result<()> {
        let Some(profile) = self.current_profile else {
            return Ok(());
        };
        let af = crate::player::chain::build_af_chain(
            &profile,
            &self.eq,
            &self.dsp,
            self.smart_speed_level,
        );
        self.mpv
            .set_property("af", af.as_str())
            .map_err(|e| Error::Player(format!("rebuilding af chain: {e}")))
    }

    /// Apply `profile` and start playing `path`. The profile properties are set
    /// before the load so they take effect for this item (spec §6.1: the engine
    /// applies the item's profile before playing).
    pub fn load(&mut self, path: &str, profile: &MusicProfile) -> Result<()> {
        // Gapless: `weak` preserves the source rate across a mixed-rate library
        // (spec §6.2); `no` for single items (episodes).
        self.mpv
            .set_property("gapless-audio", if profile.gapless { "weak" } else { "no" })
            .map_err(|e| Error::Player(format!("setting gapless-audio: {e}")))?;
        // Per-show variable speed (Phase 6b-ii-c-3-a): keep pitch constant via
        // scaletempo2 so faster speech stays natural. 1.0 / off is a no-op for
        // music, so the track path is unchanged.
        self.mpv
            .set_property("audio-pitch-correction", profile.pitch_correction)
            .map_err(|e| Error::Player(format!("setting audio-pitch-correction: {e}")))?;
        self.mpv
            .set_property("speed", profile.speed)
            .map_err(|e| Error::Player(format!("setting speed: {e}")))?;
        // Re-assert the resampler quality (Phase 5.5c-ii): cheap, defensive (an AO
        // reinit could in principle reset `audio-resample-*`), and gap-free. The
        // output backend is deliberately NOT re-asserted here: it needs `ao-reload`,
        // which would click the audio on every track.
        let _ = self.set_resampler(self.resampler);
        // The labelled `af` chain (Phase 5.5a): built fresh from this item's
        // profile, so ReplayGain (the `@rg` head `volume`) is recomputed per
        // track — the fix for mpv #8267, where the built-in `--replaygain` (now
        // dropped) sat after the chain and inherited the first track's gain. An
        // empty string clears any prior chain.
        let af = crate::player::chain::build_af_chain(
            profile,
            &self.eq,
            &self.dsp,
            self.smart_speed_level,
        );
        self.mpv
            .set_property("af", af.as_str())
            .map_err(|e| Error::Player(format!("setting af chain: {e}")))?;
        // libmpv2's `command` builds a single command *string* (no array form),
        // so an unescaped path with spaces would split into multiple args. mpv's
        // command parser reads a double-quoted token (with backslash escapes) as
        // one literal argument, which is how we pass an arbitrary path through
        // command_string safely.
        self.mpv
            .command("loadfile", &[&quote_arg(path)])
            .map_err(|e| Error::Player(format!("loadfile: {e}")))?;
        // Remember the profile so a live EQ change can rebuild the chain.
        self.current_profile = Some(*profile);
        Ok(())
    }

    /// Stop playback and unload the current file (mpv `stop`). Used when the
    /// queue is cleared; the host stays alive for the next load.
    pub fn stop(&mut self) -> Result<()> {
        self.current_profile = None;
        self.mpv
            .command("stop", &[])
            .map_err(|e| Error::Player(format!("stop: {e}")))
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

    /// The audio output devices mpv can play to (spec §6.5). mpv always lists an
    /// `auto` pseudo-device; real PipeWire/Pulse/ALSA sinks follow.
    pub fn audio_devices(&self) -> Result<Vec<AudioDevice>> {
        let node: MpvNode = self
            .mpv
            .get_property("audio-device-list")
            .map_err(|e| Error::Player(format!("audio-device-list: {e}")))?;
        let mut out = Vec::new();
        if let Some(array) = node.array() {
            for entry in array {
                let (mut name, mut description) = (String::new(), String::new());
                if let Some(map) = entry.map() {
                    for (key, value) in map {
                        match key.as_str() {
                            "name" => name = value.str().unwrap_or_default().to_string(),
                            "description" => {
                                description = value.str().unwrap_or_default().to_string()
                            }
                            _ => {}
                        }
                    }
                }
                if !name.is_empty() {
                    out.push(AudioDevice { name, description });
                }
            }
        }
        Ok(out)
    }

    /// Switch the output device (mpv `audio-device`; `auto` is the default).
    pub fn set_audio_device(&mut self, name: &str) -> Result<()> {
        self.mpv
            .set_property("audio-device", name)
            .map_err(|e| Error::Player(format!("setting audio-device: {e}")))
    }

    /// Switch the output **backend** (Phase 5.5c-ii, spec §6.5): mpv's `ao` driver,
    /// distinct from the device (`audio-device`, the 4c-ii picker). `auto` maps to
    /// an empty `ao` (mpv's own driver autoprobe); a named backend pins the driver.
    /// The change is applied immediately by reloading the audio output (`ao-reload`,
    /// gap-acceptable, the `set_dsp` structural-rebuild precedent), so an in-session
    /// switch takes effect without waiting for the next item.
    pub fn set_output_backend(&mut self, backend: &str) -> Result<()> {
        self.output_backend = backend.to_string();
        // mpv has no `auto` driver name; the autoprobe is an empty list.
        let ao = if backend == "auto" { "" } else { backend };
        self.mpv
            .set_property("ao", ao)
            .map_err(|e| Error::Player(format!("setting ao: {e}")))?;
        self.mpv
            .command("ao-reload", &[])
            .map_err(|e| Error::Player(format!("ao-reload: {e}")))
    }

    /// Set the resampler quality (Phase 5.5c-ii, spec §6.5). `High` raises the
    /// `audio-resample-*` knobs for the unavoidable-resample case; `Default`
    /// restores mpv's defaults so a toggle-back reverts. Avoid-resample stays the
    /// default either way (`audio-samplerate` / `audio-format` are left unset, so a
    /// same-rate file is untouched). `filter-size` is the authoritative integer
    /// knob; `cutoff` is best-effort (its accepted range varies by libswresample).
    pub fn set_resampler(&mut self, quality: ResamplerQuality) -> Result<()> {
        self.resampler = quality;
        let (filter_size, cutoff) = match quality {
            ResamplerQuality::High => (32, 0.95),
            ResamplerQuality::Default => (16, 0.0),
        };
        self.mpv
            .set_property("audio-resample-filter-size", filter_size)
            .map_err(|e| Error::Player(format!("setting audio-resample-filter-size: {e}")))?;
        let _ = self.mpv.set_property("audio-resample-cutoff", cutoff);
        Ok(())
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

    /// The current playback rate (mpv `speed`), 1.0 = native. Set by `load` from
    /// the item's profile (Phase 6b-ii-c-3-a per-show speed).
    pub fn speed(&self) -> Option<f64> {
        self.mpv.get_property::<f64>("speed").ok()
    }

    /// The decoded channel count of the current item (Phase 11b status bar), from
    /// mpv's `audio-params/channel-count`. `None` before the first frame / when
    /// nothing is loaded. Sourced at runtime because `channels` is not a stored
    /// column (the 11a deferral): the status bar shows it only while playing,
    /// which is exactly when this property is available.
    pub fn channels(&self) -> Option<i64> {
        self.mpv
            .get_property::<i64>("audio-params/channel-count")
            .ok()
    }

    /// Whether mpv's core is idle while it should be producing audio, i.e. it is
    /// waiting on the network/cache (v0.0.38). mpv's `core-idle` is true whenever
    /// no audio is being output, which includes a streamed item still buffering
    /// its first packets. The engine only treats this as "buffering" when the
    /// engine itself is not paused/ended, so a deliberately idle player does not
    /// read as stalled. A missing property (no item loaded) reads as not idle.
    pub fn is_buffering(&self) -> bool {
        self.mpv.get_property::<bool>("core-idle").unwrap_or(false)
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
