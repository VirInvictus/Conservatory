# Audio test fixtures

Tiny tagged audio files (sub-second silence, a few KB each) exercising the
Phase 1c tag reader (`conservatory-core/src/tags.rs`) across formats. These are
the **first committed binary fixtures** in the workspace; tag reading cannot be
tested without real container bytes, and committing them keeps CI hermetic (no
ffmpeg needed at test time). The cover/accent tests synthesize images in memory
and commit nothing.

Each file carries an identical known tag set: title "Test Title", artist
"Test Artist", album artist "Test Album Artist", album "Test Album",
track 3/12, disc 1/1, year 2021, genres ["Electronic", "Ambient"],
ReplayGain track -7.50 dB / album -7.20 dB, and an embedded solid-red 32x32 PNG
cover. Format quirks the tests pin: ID3v2 (mp3) collapses to a single genre;
Opus always reports 48 kHz.

## Regenerating

```sh
cargo run -p conservatory-core --example gen_audio_fixtures
```

Requires `ffmpeg` on PATH (with `libmp3lame`, `libopus`, and the `aac` encoder).
The example synthesizes the silent containers via ffmpeg, then writes the tags
and cover with lofty. Commit the regenerated files.
