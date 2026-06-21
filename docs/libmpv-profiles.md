# libmpv Profile Reference

> **Status: partly implemented (Phase 4a).** The music profile and the libmpv host landed at Phase 4a (`conservatory-core/src/player/`): a single libmpv instance, gapless + ReplayGain, the `playback_state` cursor, play-count-on-completion. The unified queue + profile switching (4b), MPRIS/inhibitor (4c), and the podcast/audiobook spoken-word chains (6c/7c) are still ahead. 4a settled two open questions by deferral: ReplayGain is read-only (no in-app scan, §16.7) and there is no EQ/DSP (§16.6). This expands spec §6 and is the contract the engine builds against. The podcast filter chains are ported verbatim from Belfry §5.

## One engine, one queue, two profiles

A single `libmpv` instance (via the `libmpv2` binding) is kept alive across items, using the property API plus the filter graph. The queue (spec §4.3) interleaves tracks, episodes, and audiobooks freely. On advance, the engine resolves the next item's **playback profile** and applies it (the right `af` filter chain, ReplayGain mode, gapless/crossfade behaviour) before playing.

```rust
struct PlayableItem {
    path_or_url: Source,     // local managed file, or stream URL for an undownloaded episode
    kind: MediaKind,         // Track | Episode | Audiobook
    profile: PlaybackProfile // resolved per-kind + per-show/per-album/per-book overrides
}
```

This single abstraction is what lets one queue, one Now-bar, one MPRIS surface, and one set of media keys serve all three media types.

## Music profile (spec §6.2)

- **Gapless:** `--gapless-audio` within an album.
- **ReplayGain:** track and album modes, read from `tracks.replaygain_track` / `tracks.replaygain_album`. Config `playback.replaygain = off | track | album`. Whether Conservatory also *scans* ReplayGain for untagged files or only reads existing tags is **OPEN** (spec §16.7).
- **Crossfade:** between non-gapless tracks, user-configurable duration, off by default (`playback.crossfade_seconds = 0`).
- **EQ / DSP:** depth is **OPEN** (spec §16.6): none, a simple EQ, or a deadbeef-class chain. A full DSP chain is its own project; do not assume it.

## Podcast profile (spec §6.3, ported from Belfry §5.1–5.3)

The filter graph that forces the project's GPL-3-or-later license (the `rubberband` link, spec §15).

- **Smart Speed:** silence-skip via the ffmpeg `silenceremove` filter, with pitch-preserving time-stretch via `rubberband`. The combination shortens dead air without chipmunking speech. Includes the time-saved session accounting (how much wall-clock the silence-skip saved), surfaced in stats.
- **Voice Boost:** dynamic-range compression (`acompressor`) + voice-band `equalizer` + loudness normalization (`loudnorm`), tuned to make quiet/uneven spoken audio intelligible at low volume.
- **Per-show overrides:** speed, Smart Speed on/off, Voice Boost on/off, skip intro/outro, as in Belfry. Resolved into the `PlaybackProfile` when the episode is queued.

Required ffmpeg filters (the `mpv-libs` build must carry them): `silenceremove`, `rubberband`, `acompressor`, `equalizer`, `loudnorm`. On Fedora this means RPM Fusion's `ffmpeg-libs`, not `ffmpeg-free-libs` (rubberband is the one that is absent from the free build).

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
