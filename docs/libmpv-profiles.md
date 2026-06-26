# libmpv Profile Reference

> **Status: partly implemented; the DSP layer is Phase 5.5.** The music profile and libmpv host landed at Phase 4a–4c (`conservatory-core/src/player/`): a single libmpv instance, gapless + ReplayGain (read), the unified queue + Now-bar, MPRIS/inhibitor, the output-device picker. Phase 5c added in-app ReplayGain scanning (rsgain, §16.7 settled). Still ahead: the **Phase 5.5 audio engine** (the labelled `af`-chain builder, EQ, DSP modules, head-staged ReplayGain, output backend/resampler — §16.6, now resolved) and the podcast/audiobook spoken-word chains (6c/7c), which are **presets on that engine**. This expands spec §6 and is the contract the engine builds against. The current code still sets the flat `gapless`/`replaygain` properties (`player/host.rs`); 5.5a is the refactor to the chain.

## One engine, one queue, two profiles

A single `libmpv` instance (via the `libmpv2` binding) is kept alive across items, using the property API plus the filter graph. The queue (spec §4.3) interleaves tracks, episodes, and audiobooks freely. On advance, the engine resolves the next item's **playback profile** and applies it (the right `af` filter chain, ReplayGain, gapless behaviour) before playing.

```rust
struct PlayableItem {
    path_or_url: Source,     // local managed file, or stream URL for an undownloaded episode
    kind: MediaKind,         // Track | Episode | Audiobook
    profile: PlaybackProfile // resolved per-kind + per-show/per-album/per-book overrides
}
```

This single abstraction is what lets one queue, one Now-bar, one MPRIS surface, and one set of media keys serve all three media types.

## DSP chain discipline (build once, mutate via `af-command`)

mpv routes every libavfilter filter through one `lavfi` wrapper, and `af-command` reaches only the filters whose ffmpeg implementation supports runtime commands. The rules that fall out (Phase 5.5):

- Build the full chain **once** with labelled stages (`@rg`, `@eq`, `@comp`, `@boost`); change *parameters* via `af-command` (gap-free).
- Structural mutation (`af add` / `af set` / `af remove` / reorder) reinitializes the graph and **gaps the audio** — reserve it for explicit settings changes, never for a slider drag.
- Order is signal flow: ReplayGain → EQ → compressor/limiter → leveler. Don't hand-place a resampler; mpv auto-inserts format conversion, and the speed/tempo filter (`scaletempo2`) is auto-inserted on `--speed`.
- **Tarpits:** `superequalizer` / `firequalizer` (no runtime command, so every EQ change rebuilds the graph); `loudnorm` live (its accurate mode is two-pass/offline — use `dynaudnorm` for live leveling, reserve `loudnorm` for an optional import-time pass); `rubberband` in the chain *and* mpv `--speed` (two time-stretchers fight over the tempo factor — drive speed with `--speed` + `audio-pitch-correction` only); a libmpv-PCM visualizer (no audio callback exists in libmpv; a spectrum would need a separate PipeWire monitor tap).

## Music profile (spec §6.2)

Resolved into a **labelled `af` chain**, built once per item and tuned via `af-command` (the discipline above).

- **ReplayGain (head stage):** an explicit `volume=<dB>` at the *head* of the chain, from `tracks.replaygain_track` / `_album` (scanned in-app via rsgain, §16.7 settled), with a preamp (`playback.replaygain_preamp`) and clip-prevention (`playback.replaygain_clip`). **Recomputed and reset on every track change.** This replaces mpv's built-in `--replaygain`, which sits *after* the `af` chain (a boosting EQ defeats clip-prevention) and is not re-applied per track across a gapless boundary (mpv bug #8267: the whole queue inherits track 1's gain). Modes off / track / album.
- **Equalizer:** a graphic EQ as a stack of `equalizer` peaking bands at fixed ISO centres, gains moved live via `af-command`; a parametric option via `anequalizer` (live `change` per band). Named presets, `flat` is a no-op chain. **Settled (5.5b): 10-band ISO octave** — 31 / 62 / 125 / 250 / 500 / 1k / 2k / 4k / 8k / 16k Hz, each rendered `equalizer@b<n>=f=<centre>:t=o:w=1:g=<dB>` under one `@eq` lavfi label (the per-band name is the `af-command` target). 5.5b-i builds the chain + persists presets (applied at load); 5.5b-ii adds the live `af-command` mutation + the GTK Sound dialog; the parametric `anequalizer` is a later follow-up.
- **DSP modules (Phase 5.5c-i, implemented):** optional ordered stages after the EQ — `@comp` (`acompressor`), `@limit` (a brick-wall `alimiter`, `level=disabled` so it is a transparent peak catcher and the ReplayGain clip safety net), `@boost` (`dynaudnorm`, single-pass leveler) — each independently toggleable, built by `player/dsp.rs` and persisted in the `audio_state` singleton. A settings change does a structural `af` rebuild (an explicit change, gap-acceptable; no per-slider live path is needed for DSP, unlike the EQ).
- **Gapless:** `--gapless-audio=weak` (preserves source rate on a mixed-rate library; `audio-samplerate` / `audio-format` left unset to avoid needless resampling).
- **Crossfade: dropped.** Impossible in a single libmpv instance (one decoder, one playlist entry at a time; `acrossfade` is two-input and cannot span entries) and maintainer-rejected. Gapless-only; the old `crossfade_seconds` config key is removed.

## Output (spec §6.5, Phase 5.5c)

- **Backend** via `--ao=pipewire|pulse|alsa|jack`; **device** via the 4c-ii picker. High-quality resampler knobs (`audio-resample-*`) for the unavoidable-resample case; no resampling otherwise.
- **Realized mapping (Phase 5.5c-ii-a).** Backend = the mpv `ao` property; `auto` is an *empty* `ao` (mpv's driver autoprobe), a named backend pins the driver. A switch sets `ao` and issues `ao-reload` so it takes effect mid-session (gap-acceptable); the backend is **not** re-set per load (that would `ao-reload` every track). Resampler: `High` raises `audio-resample-filter-size` (32) + `audio-resample-cutoff` (0.95); `Default` restores mpv's (16 / its own), re-asserted per load (cheap, defensive against an AO-reinit reset). `audio-samplerate` / `audio-format` stay unset, so a same-rate file is never resampled regardless of the quality knob. Both persist in the singleton `audio_state` row.
- **Deferred (recorded, not built):** exclusive/bit-perfect (`--ao=alsa` `hw:` + `--audio-exclusive`) is bare-install-only and fights the Flatpak sandbox; LADSPA / raw-`af` hosting needs the `org.freedesktop.LinuxAudio.Plugins` extension + ffmpeg `--enable-ladspa`; native `crossfeed` is a cheap future headphone module.

## Podcast profile (spec §6.3, ported from Belfry §5.1–5.3)

The filter graph that forces the project's GPL-3-or-later license (the `rubberband` link, spec §15).

- **Smart Speed:** silence-skip via the ffmpeg `silenceremove` filter (negative `stop_periods` for mid-stream removal; `stop_threshold` / `stop_duration` tuned to trim dead air without clipping natural pauses). It changes stream duration on the fly, so seek / scrobble / position math must account for the non-linear timeline. Includes the time-saved session accounting (wall-clock the silence-skip saved), surfaced in stats. **Variable speed** is mpv's `--speed` + `audio-pitch-correction` (the built-in `scaletempo2`, WSOLA, strong at the 1.2x–2x range podcasts use), **not** a chained `rubberband`: running both stacks two time-stretchers at speed≠1.
- **Voice Boost:** dynamic-range compression (`acompressor`) + voice-band EQ + **live loudness leveling via `dynaudnorm`** (single-pass, real-time; `gausssize` to tame pumping), tuned to make quiet/uneven spoken audio intelligible at low volume. `loudnorm` is *not* used live (its accurate mode is two-pass/offline); reserve it for an optional import-time normalization pass.
- **Per-show overrides:** speed, Smart Speed on/off, Voice Boost on/off, skip intro/outro, as in Belfry. Resolved into the `PlaybackProfile` when the episode is queued. **Speed shipped at Phase 6b-ii-c-3-a:** `resolve_episode_profile` reads the show's `playback_speed` (clamped to `[0.25, 4.0]`) into the profile, and `MpvHost::load` applies mpv `speed` + `audio-pitch-correction` (scaletempo2) before `loadfile`. **Smart Speed / Voice Boost shipped at Phase 6c-i** (see the realized chain below); the `smart_speed` / `voice_boost` flags ride the same settings.
- **Realized chain (Phase 6c-i, `player/spoken.rs`):** the stages append after the music stages of `build_af_chain`, Smart Speed before Voice Boost. Smart Speed = `@ss:lavfi=[silenceremove=stop_periods=-1:stop_duration=1:stop_threshold=-40dB:stop_silence=0.3]` (mid-stream removal, leaves a 0.3 s beat). Voice Boost = three labelled stages: `@vbcomp` (`acompressor=threshold=<-24 dB linear>:ratio=4:attack=5:release=150:makeup=2`), `@vbeq` (`highpass=f=80,equalizer=f=2500:t=o:w=1:g=3`, a low-cut + presence lift), `@vbnorm` (`dynaudnorm=g=15:p=0.9`, a tighter window than the music leveler's `g=31`). Params are fixed (only the on/off flags are per-show). A show with no saved settings resolves both flags off (configured-shows-only). Time-saved accounting is 6c-ii: `smart_speed_saved = max(0, audio_seconds/speed − real_seconds)`.

Required ffmpeg filters (the `mpv-libs` build must carry them): `silenceremove`, `acompressor`, `equalizer` / `anequalizer`, `highpass`, `dynaudnorm`, `volume`. Variable speed uses mpv's built-in `scaletempo2` (no extra filter). **As of Phase 6c-i `rubberband` is not actually used** (speed is scaletempo2, Smart Speed is `silenceremove`): the "GPL-3 forced by librubberband" rationale in spec §15 / ATTRIBUTIONS is therefore due a re-confirmation. That is a license-wording decision left to Brandon; this doc does not change §15, and GPL-3 stands regardless (project choice, plus the GPL ffmpeg build). On Fedora the full set means RPM Fusion's `ffmpeg-libs`, not `ffmpeg-free-libs`.

## Audiobook profile (spec §6.3, Phase 7c)

Audiobooks are long-form speech and **reuse the podcast filter graph**: variable speed, Smart Speed, Voice Boost. No new chain is introduced; the only differences are where overrides come from and how the item is structured:

- **Overrides** resolve from `book_playback` (per-book speed / Smart Speed / Voice Boost, spec §4.5) instead of per-show settings.
- **A book is one `PlayableItem`** spanning its ordered chapters (spec §4.5). Chapter advance is *internal* to the item: a seek across per-chapter files or within a single M4B, gapless, with no queue advance. The queue advances to the next item only when the book finishes. This is the same boundary problem as the track↔episode swap below, one level down (chapter↔chapter inside a book).
- **First-class resume:** the engine seeks to `book_playback.position` (absolute seconds across the whole book) on load and writes it back on the insurance interval (spec §6.4).

## Profile switching mid-queue (the prototyping risk, spec §16.9)

Swapping the `af` filter graph between a music track and a podcast episode mid-queue is the part that needs a prototype. Gapless *within* an album plus a clean profile switch *at album/kind boundaries* is the target. The plan (roadmap Phase 4b) is to prototype the swap with the music profile alone first, so the machinery exists before episodes arrive at Phase 6c. The risk is audible glitching or latency on the boundary swap; if it proves costly, the fallback is a brief, deliberate gap at kind boundaries (never within an album).

## Streaming vs downloaded episodes (spec §5.3)

If a local episode file is absent and a URL is present, libmpv streams it with HTTP range requests; the same profile applies. Downloaded episodes play from the managed `Podcasts/` tree. Music is always local (the library owns the files).

## State persistence (spec §6.4)

Position is written on pause, on seek (debounced), on item end, on app quit, and every 30 s during playback (the Belfry insurance interval). A few seconds of resume offset is applied on long items for context. Music play counts and `last_played` update on completion; episode listening sessions are append-only; audiobook position is the absolute offset in `book_playback.position` with `finished` set on completion (spec §4.5, §6.4).

## System integration (spec §6.5)

MPRIS2 (`org.mpris.MediaPlayer2`) via `zbus`, full metadata for the current item regardless of kind, play/pause/next/previous/seek exposed to GNOME's media overlay and lock screen. PipeWire output-sink picker. Suspend inhibitor held during active playback, released on stop.
