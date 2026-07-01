# Patch Notes

## v0.1.8

Turn tabs off, and a header that follows the tab.

- **Enable or disable the Music, Podcasts, and Audiobooks tabs.** Preferences → General has a new Sections group. Turn a section off and, on the next launch, its tab is gone and none of its machinery starts (a disabled Podcasts section never spins up its feed workers, for instance), so the app carries no weight for parts you do not use. Music stays as a fallback so the window is never empty. This is separate from the build-time plugin switch; it is a per-launch preference over what is built in.
- **The toolbar follows the active tab.** The buttons that only make sense for music (Edit, Embed tags, and the track Properties panel) now hide themselves when you switch to the Podcasts or Audiobooks tab, and come back on Music. The playback, preferences, output, and menu controls stay put everywhere.

## v0.1.7

Playlists come to the window.

- **A Playlists section in the sidebar.** The left sidebar now has two labelled parts: Perspectives on top (saved searches, unchanged; the save button there saves your current filter as one) and Playlists underneath. Playlists list your static and smart playlists; clicking one plays it, filling the queue with its tracks in order.
- **Create playlists from the app.** A `+` button offers New Static Playlist (a hand-curated list) and New Smart Playlist. The smart-playlist dialog pre-fills its query from whatever is in the filter bar, so it doubles as "save this search as a playlist", and lets you set an optional size cap and a prioritisation order (newest, highest-rated, least-recently-played, title, or artist). A trash button deletes the selected playlist.
- **Add to Playlist, by right-click.** The track right-click menu gains an "Add to Playlist" submenu listing your static playlists; pick one to append the selected tracks. (Smart playlists are rules, so they are not shown there.)

## v0.1.6

Playlists arrive, from the command line first.

- **Static and smart playlists (CLI).** A new `playlist` command family creates and manages two new kinds of playlist. A **static** playlist is a frozen, hand-ordered list; a **smart** playlist is a live rule (a search expression, with an optional size cap and a prioritisation order) that resolves fresh every time. These are deliberately distinct from Perspectives, which stay saved *browse* queries: a Perspective filters what you are looking at; a smart playlist is a queue source with its own limit and order. Order keys are added, rating, least-recently-played, title, and artist (a shuffle order arrives with the Phase 17 shuffle work). Use `playlist create-static`, `create-smart`, `add`, `list`, `show`, and `delete`. The graphical Playlists surface (a sidebar section and a visual rule builder) follows next.
- Under the hood: a new database migration adds the playlist tables (core-owned, so they exist in every build), with the storage and a fast SQL order/limit primitive in the engine and the query evaluation in the CLI, keeping the core engine free of the search grammar by design. Covered by new tests.

## v0.1.5

The bulk metadata editor shows what you are editing.

- **"Multiple values" in the bulk editor.** Editing several tracks at once, each field now pre-fills the value the selection shares, and reads "multiple values" when they differ, so you can see the current state instead of a row of blank boxes. Each field has a checkbox: only ticked fields are written (and editing a field ticks it for you), so a shared value is never silently rewritten and you choose exactly what to change. Path-affecting edits still go through the same dry-run move preview.

## v0.1.4

Click a star to rate.

- **Click-to-rate in the track list.** The rating column's stars are now interactive: click where you want the rating to land and it fills to there (one through five), and clicking the star it already fills to clears the rating back to zero. The rating is written immediately, and only that one row repaints, so it stays instant on a large library. The properties inspector's rating follows along when it is open. The context-menu Rating submenu from v0.1.3 still works for whole selections.

## v0.1.3

Right-click context menus arrive, with Play Next and Remove from Library.

- **Context menus everywhere you browse.** Right-clicking (or long-pressing) a row now opens a menu on all five surfaces: the music track list (Play, Play Next, Add to Queue, Edit…, a Rating submenu, Reveal in Files, Remove from Library), the facet panes (Play / Play Next / Add to Queue over the facet's set), the queue drawer (Remove from Queue, Clear Queue), the podcast episode list (Play, Add to Queue, Mark Played/Unplayed, Star/Unstar, Archive), and the audiobook shelf (Play, Add to Queue, Edit…). Right-clicking a row that isn't already selected selects just it first, the familiar file-manager behaviour, and every verb reuses an existing keyboard or button path, so nothing is menu-only.
- **Play Next.** A new verb that drops the selection into the queue just after whatever is playing, rather than at the tail, so you can cue something up without disturbing the rest of the queue. The live player and the on-disk queue are inserted at the same spot, so they stay in step.
- **Remove from Library.** Removes tracks from the library from the track-list menu, behind a confirm. It is a database-only unlink: the files stay on disk (so the tracks are re-importable), and the removal cleans up after itself; the queue entries and the playback cursor drop away through the schema's cascades, and search stays in sync.
- Under the hood: a reusable right-click gesture for list rows, a new engine queue-insert command with a unit-tested index-adjust helper, and a matching database queue-insert plus a track-delete, all covered by new tests. Both the full and music-only builds are green.

## v0.1.2

Smart Speed gains a level.

- **Smart Speed level (Gentle / Balanced / Aggressive).** A new control on the Sound settings sets how aggressively Smart Speed trims dead air, so you can trade gentle cuts for more trimming. Gentle is the v0.1.1 tuning and stays the default; Balanced and Aggressive act on shorter, quieter pauses. Measured on a real episode with ffmpeg, the tiers remove roughly 0.3% / 1% / 3.5% of a tightly-produced podcast (more on looser, chattier shows). The per-show and per-book Smart Speed on/off is unchanged; this is the one global aggressiveness applied wherever Smart Speed is on, for podcasts and audiobooks alike. The level applies to the current episode live (the audio-filter chain rebuilds without a reload), persists in the audio settings (a new `audio_state` column, migration 0016), and is settable headless with `conservatory-cli dsp smart-speed <gentle|balanced|aggressive>`.

## v0.1.1

A polish and bug-fix pass over the player surface, plus spoken-word playback fixes.

- **The Now Playing drawer is a full-bleed spectrum.** The expanded drawer (Ctrl+I) drops the redundant second seekbar and the metadata grid; the visualizer now fills it edge to edge as the hero, with a minimal cover / title / artist chip over it. The analyzer is much denser (one thin bar per band, 320 bands) with gradient bars, slow-falling peak caps, and a soft reflection, and it updates live as the queue advances (item-change detection now keys on the queue slot, not just the track id, so it no longer goes stale between songs).
- **The spectrum taps only Conservatory.** It captured the whole output device before, so any other app's audio moved the bars. It now targets our own mpv output node by name (WirePlumber fans the node out to both the speakers and the capture), so it reacts to Conservatory alone; the tap runs only while playing, so it never falls back to the microphone.
- **Side panels give their space back.** Closing the Track Properties inspector or the queue, and the Now Playing drawer collapsing, now return the freed area to the browse, which fills both axes (a revealer was expanding when it should not have, so the browse sat parked in the top-left). The inspector no longer opens empty by default, and both side panels are a touch narrower.
- **The CLI exits cleanly.** `conservatory-cli play` and `audiobook play` now respond to Ctrl-C and exit promptly, instead of ignoring the signal (libmpv sets it to ignore) and then hanging in libmpv / libpipewire teardown.
- **Audiobook chapters are playable.** Activating a chapter row in the book detail pane now plays that book from the chapter's position; it was display-only before.
- **Podcast Smart Speed / Voice Boost / speed by the transport.** A per-show playback control sits next to the transport whenever a podcast episode is playing, so speed, Smart Speed, and Voice Boost are reachable where you reach for play/pause. Changes apply to the current episode live: mpv speed and the audio-filter chain rebuild without a reload, not only from the next episode.
- **Smart Speed actually trims dead air now.** Its silence gate was tuned to near-digital-silence (-40 dB over 1 s) and was measured (ffmpeg, real episodes) to remove nothing from produced podcasts; it is now -30 dB over 0.5 s, which triggers on real speech pauses. Modern loudness-normalized podcasts still have little removable silence, so a per-show aggressiveness level and a dynamic speed-up approach are planned.

## v0.1.0

The first tagged milestone. Conservatory is a daily-driver music player, a full podcast client, and an audiobook library in one native GNOME app, with a database-owned library and a trust-critical file mover (dry-run, undo, crash-safe replay).

This tag promotes the v0.0.91 readiness-gate work to the 0.1.0 release. No code changes since v0.0.91; what changed is the disposition of the gate. Flatpak/Meson packaging is not part of this tag (a later concern), and the real-library verification passes (idle memory at 50k tracks, a full-library move-safety run) are post-0.1.0 items rather than blockers. The synthetic gate passed and the one move-safety bug it found is fixed.

Known follow-ups: the spec §13 idle-memory budget is confirmed at 12k tracks but not yet at the 50k target; if a real-library measurement comes in over budget it becomes a 0.1.x fix.

## v0.0.91

Phase 15 work begins: the 0.1.0 readiness gate. This release is a verification pass (quality, move safety, memory budget) plus the one fix it turned up. No new features.

- **Undo now restores the cover too.** Verifying the move-safety undo path turned up a real gap: undoing an `organize` move put the track files back but left the album's `cover.jpg` stranded in the old destination, with the database's `cover_path` pointing at the wrong folder. Undo now re-syncs covers the same way an apply does, so an undo is a byte-identical restoration of the whole album folder. A regression test (`cover_resyncs_back_on_undo`) locks it in.
- **Move safety, end to end.** Against a real two-album working copy: the dry-run preview matched what `--apply` actually moved, the undo round-trip restored the tree byte-for-byte, and the crash-mid-move roll-forward path stays covered by its existing test.
- **Memory, measured.** A release build was sampled via `--debug`: idle on a 12,000-track library sits at about 196 MB, and active playback holds around 187 MB (playback adds no measurable resident memory over the warm floor). Both are within the spec §13 budgets at this scale. The 50,000-track idle target is not yet confirmed (the synthetic fixture tops out at 12k) and is tracked as a pre-1.0 check.
- **Housekeeping.** Removed a dead `id3` entry from the workspace dependency catalog (it was never wired into any crate). The quality gate (format, clippy on both the default and music-only builds, the full test suite) is green.

## v0.0.90

Phase 14b: the `--debug` mode now sees file IO and network, completing the four channels.

- **`conservatory::io`** lights up across every place the app touches the filesystem: the file mover (rename, cross-device copy + fsync + rename, revert), cover writes, tag write-back, APE stripping, the import scan, podcast downloads and retention deletes, and playlist / OPML export. A `--debug` import now prints a line per file moved, with byte counts.
- **`conservatory::net`** carries every HTTP request (there is no other network): feed fetches (GET / 304 / response), episode downloads, and chapter fetches, all on one filterable channel.
- New [`docs/debugging.md`](docs/debugging.md) documents the flag, the four channels, and `RUST_LOG` narrowing; the README points at it.

## v0.0.89

Phase 14a: a debug mode. Run either program with `--debug` for a verbose diagnostic stream on stderr.

- **`conservatory --debug`** or **`conservatory-cli --debug <command>`** now print every SQL statement with its execution time, plus periodic resident-memory samples. The CLI gained the `--debug` / `-d` flag it was missing.
- **Four channels.** Output is tagged `conservatory::sql`, `conservatory::io`, `conservatory::net`, and `conservatory::mem`, so you can narrow it with `RUST_LOG` (for example `RUST_LOG=conservatory::sql=debug` for SQL alone). The IO and network channels fill in next release.
- **Zero cost when off.** Without `--debug`, none of the hooks are installed and the programs behave exactly as before.

(Internal: the one timing-sensitive sleep-timer integration test, flaky under heavy build load, is now run on demand rather than in the default suite; its behaviour stays covered by deterministic unit tests.)

## v0.0.88

Phase 13e-iii: a keyboard shortcuts reference, closing Phase 13.

- **Press F1** (or open "Keyboard Shortcuts" from the header menu) for a grouped, scrollable list of every shortcut that actually works: playback, browse and queue, panels and view. The list is curated to match what is wired, so it will not promise a key that does nothing.
- The keymap documentation is brought in line with reality: the playback keys are marked live, and the handful still on the drawing board (arrow-key seek, save-as-Perspective, remove-from-library) are flagged as such rather than implied to work.

## v0.0.87

Phase 13e-ii: keyboard shortcuts for playback. The app is now keyboard-drivable for daily listening without reaching for the mouse.

- **Space** plays / pauses, everywhere except when you are typing in the filter box.
- **Ctrl+Right / Ctrl+Left** skip to the next / previous track.
- **Ctrl+Up / Ctrl+Down** nudge the volume; **Ctrl+0** mutes and unmutes.
- **Ctrl+L** clears the filter; **Ctrl+Q** quits.

Two keys were deliberately left out: bare-arrow seek (the arrow keys navigate the browse columns, so binding them to seek would break navigation) and a library-delete shortcut (deleting files needs a confirmation step, not a bare keypress).

## v0.0.86

Phase 13e-i: double-click a genre or artist to play it.

- **Activate-to-play in the browse columns.** Double-clicking (or pressing Enter on) a value in any browse column now plays that filtered set, the way deadbeef-cui and foobar2000 do. Double-click "Ambient" and it plays your ambient tracks; double-click the `[All]` row and it plays everything under your other column selections. The track list already played on double-click; this brings the facet columns in line.

## v0.0.85

Phase 13d: typography. Conservatory now ships its own fonts and uses the right one for each part of the interface.

- **Inter for the interface.** The body, menus, and the dense track list and facet panes are set in Inter, a screen-optimized typeface with aligned (tabular) numbers, so the duration, year, and bitrate columns line up cleanly.
- **Fraunces for headers.** Titles and section headings are set in Fraunces, a warm display serif, giving the headings a bit of character against the neutral body.
- **IBM Plex Mono for technical text.** File paths, the status-bar codec and bitrate line, and MusicBrainz ids render in a monospace, so the technical bits read as technical and line up.
- **Bundled, never assumed.** All three fonts are open-licensed (SIL OFL) and ship with the app; nothing has to be installed on your system. If a font ever cannot be loaded, the text falls back to a sane default rather than breaking.

No behavior change beyond the look; the full test suite, clippy, and the music-only build are green.

## v0.0.84

Phase 13c: an internal tidy. No behavior change, no new features; the app does exactly what it did at v0.0.83.

- **One accent helper.** The Now Playing drawer and the Audiobooks shelf each carried their own copy of the "swap a display-wide CSS provider" dance for per-item accent colour. Both now route through the single shared helper the inspector and the Now-bar already use, so there is one implementation instead of three.
- **One home for the metadata projections.** The pure functions that turn a track, episode, or audiobook into labelled display rows (and the small `push` helper that was duplicated between two files) moved into a single `ui/fields.rs` module, with their unit tests. The inspector, the Now Playing drawer, and the browse all read from the same place.
- **A clean bill on the rest.** A code audit found the codebase otherwise in good shape (no dead code, no stale comments to speak of, consistent error handling), so this release does not churn it for its own sake.

The full test suite, clippy, and the music-only build are green; the moved field projections keep their headless tests as the guardrail.

## v0.0.83

Phase 13b: the structural part of the sleekness pass. Empty states, a grouped header, and toast confirmations.

- **Empty states that say something useful.** The browse area no longer shows a blank grid when there is nothing to show. An empty library invites you to import; a filter with no matches says so plainly. The Now Playing drawer shows a tidy "Nothing playing" panel when idle instead of a lone line of text.
- **A grouped header.** The header buttons are organized into clusters: the panel toggles (queue, properties, Now Playing) read as one segmented group, the edit and embed actions as another, with preferences, output, and the menu set apart. Less of a flat undifferentiated row.
- **Toast confirmations.** Actions that used to complete silently or behind a modal "Done" dialog now give a brief, non-modal toast: embedding tags into files and saving track edits both confirm themselves and get out of the way.

No data or playback behavior changed; this is presentation and feedback.

## v0.0.82

Phase 13a: the layout space-reclaim fix plus a round of low-risk polish. The first of a UI/UX sleekness pass.

- **Closing a side panel gives the space back.** Closing the track-properties inspector (or the queue) now flows the freed width back into the browse area instead of leaving a dead gap. The browse body is the horizontal expand-sink; before, no widget claimed the space a collapsed panel left behind.
- **Smoother motion.** The queue, inspector, and Now Playing drawers slide with a consistent, intentional duration rather than the default snap.
- **Hover feedback.** Track rows, sortable column headers, chapter rows, and the sleep-timer menu now respond to the pointer with a subtle, animated highlight.
- **Breathing room.** The track-properties and Now Playing detail grids, the chapter list, and the up-next list got their cramped 2px spacing opened up so label/value pairs read cleanly.
- **Depth and accent.** Album covers carry a softer layered drop shadow; text selection and the keyboard focus ring now use the Kanagawa accent; scrollbar sliders are gently rounded.

No behavior change beyond the layout fix; the dense browse table stays dense by design.

## v0.0.81

- **Finer spectrum.** The visualizer now draws as a dense field of thin vertical lines (192 of them) instead of chunky bars, with a finer FFT for the resolution to back it. A fine analyzer look rather than blocks.

## v0.0.80

Phase 12d: a real-time spectrum visualizer in the Now Playing drawer, closing Phase 12 (the visual overhaul).

- **Frequency spectrum.** Accent-coloured frequency bars that move with the music, the deadbeef spectrum brought across. They live in the Now Playing drawer (`Ctrl+I`); open it while something plays and the bars dance.
- **How it works.** libmpv exposes no way to read the audio it plays, so the visualizer taps the system audio at PipeWire (the default output's monitor) and runs its own FFT. The maths (windowing, the transform, the log-spaced bands, the fall-off smoothing) is in the engine core and unit-tested; the capture and the drawing are in the app.
- **Smooth.** The bars are drawn on the display's frame clock and smoothed with a fast rise and a slow fall, so they animate fluidly rather than at the player's slower polling rate. Capture runs only while the drawer is open, so it costs nothing when closed.
- **Caveat.** Because it taps the output device, the visualizer reacts to whatever your system is playing, not only Conservatory. If a browser tab is making noise, the bars will show it too.
- **Tests.** The band maths and the smoother are pure and unit-tested (a 1 kHz tone lands in the right band; silence is flat; the smoother rises fast and decays slow). The capture and the widget are verified by running. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. New dependencies: `realfft` (the FFT) and `pipewire` (the capture), both recorded in ATTRIBUTIONS.md; no schema change.

This closes Phase 12 (Visual identity & album-art-forward UI): the Kanagawa Dragon theme, album art across the browse, an enriched playback bar, and now a live spectrum.

## v0.0.79

Phase 12c: a richer Now-bar, the answer to "the playback bar should give more information, not just be a bigger play bar."

- **Bigger, accent-framed cover.** The Now-bar cover grows from 40 to 56px and sits in a frame tinted with the album's accent, the same colour the rest of the app draws from the artwork.
- **Artist and album.** The secondary line now reads `artist · album`, folding the duplicate for a podcast (whose artist and album are both the show), so it never reads "Show · Show".
- **Accent seek.** The filled portion of the seek slider takes the album's accent, so the progress bar belongs to what is playing.
- **Now Playing drawer.** The drawer's cover grows (132→160px) and its spacing opens up for a more composed feel, now that artwork and the Dragon palette are in place.
- **Tests.** The secondary-line folding is pure and unit-tested; the widgets are verified by building and by launching. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. No new dependency, no schema change (the playing item's accent rides the existing now-playing metadata read).

## v0.0.78

Phase 12b: album art in the music browse, the "more covers" half of the Phase 12 overhaul.

- **Cover thumbnail per track.** The track list gains a leading album-cover column, the deadbeef album-art-per-row look. Covers load lazily (only visible rows) and are cached downscaled, so scrolling a large library does not re-decode full-resolution artwork.
- **A large cover panel.** The properties inspector (the right-docked `coverart` + properties column) now opens by default with a bigger 300px cover, so album art is on screen from first launch. `Ctrl+P` still toggles it.
- **Read layer.** The leaf query now projects each album's cover and accent onto its tracks; a single change point feeds both the fast SQL path and the in-memory filter path.
- **Tests.** A core read test asserts the cover/accent projection; the column, cache, and panel are verified by building and by launching. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. No new dependency (the downscaler is the GTK stack's own gdk-pixbuf), no schema change.

## v0.0.77

Phase 12a: a visual identity. The app now ships the Kanagawa Dragon theme instead of inheriting the flat grey system look, the first step of the Phase 12 UI overhaul.

- **Kanagawa Dragon palette.** The Dragon variant mapped onto libadwaita's named colours, with the dark scheme forced. Warm-dark chrome (`#181616` ground, `#c5c9c5` text) and the dragonRed accent (`#c4746e`) on the rating stars, the playing row, and selection, in place of the system grey. The full palette-to-libadwaita mapping is documented in `docs/theme.md`.
- **Lifted cover cards.** Album, inspector, Now Playing, book, and Now-bar covers get a rounded card with an Amberol-style drop shadow, so artwork reads as a raised object rather than a flat thumbnail. The previously styleless Now-bar cover class is filled in.
- **Centralized accent.** A shared `ui/accent.rs` helper replaces the copy-pasted per-module accent providers with one technique: a 2px per-album accent ring layered over the cover's drop shadow. The properties inspector is migrated onto it; the browse covers, Now-bar, and Now Playing surfaces adopt it in the following sub-phases.
- **Tests.** The accent helper's pure parts (the class name and the ring CSS) are unit-tested; the visual result is verified by building and by launching. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. No new dependency, no schema change.

This starts Phase 12 (Visual identity & album-art-forward UI). Still to come: album art in the music browse (a cover column and a large cover panel), an enriched playback bar, and a spectrum visualizer.

## v0.0.76

Phase 11d: transport conveniences, completing the Phase 11 Columns UI parity pass.

- **Stop after current (`Ctrl+M`).** Arm it and the player finishes the current track / episode / book, then pauses at the boundary instead of playing on; it disarms itself once it fires. Also in the new header menu, with a checkmark when armed.
- **Jump to current track (`Ctrl+J`).** Select and scroll the browse list to the playing track. A no-op when the playing item is a podcast / audiobook or has been filtered out of the view. Also in the header menu.
- **Header menu.** A primary menu in the header bar hosts both, so the conveniences are visible and not keyboard-only (spec §3.1).
- **Tests.** Stop-after-current is covered by an engine integration test (the queue pauses at the boundary, the next item never plays), beside the existing end-of-item sleep test; the jump's row-resolution is a pure unit. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. No new dependency, no schema change.

This completes Phase 11 (Browse & player polish). The music surface now matches the deadbeef daily-driver layout: configurable facet panes, a properties inspector, a status bar with the play-status glyph, an enriched Now Playing drawer, and the expected transport conveniences.

## v0.0.75

Phase 11c: a richer Now Playing drawer, the spec §3.6 surface filled out.

- **Full-bleed cover.** The Now Playing drawer (click the playback bar, or `Ctrl+I`) now leads with a large album / show / book cover, tinted with the item's accent colour, the bigger sibling of the small playback-bar thumbnail.
- **Accent scrubber.** An accent-tinted seek slider with a position / duration label sits under the title, draggable to seek, distinct from the playback bar's own scrubber.
- **Up next.** A short peek at the next few items in the queue, each badged with its kind, so you can see what is coming without opening the queue drawer. It stays in step with queue edits.
- **Audio-engine line.** For a playing track, a one-line readout of the active EQ preset, the enabled DSP modules, and the gapless state. (The drawer already showed chapters, the Smart Speed indicator, and the sleep timer, which arrived with the podcast engine.)
- **Tests.** The audio-engine line and the queue-tail slice are pure and unit-tested; the cover, scrubber, and up-next widgets are verified by building and by hand. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. No new dependency, no schema change.

## v0.0.74

Phase 11b: a status bar footer and the play-status glyph column, the next slice of the Columns UI parity pass.

- **Status bar.** A thin footer above the playback bar. On the left, the playing track's technical line (format, sample rate, channels, bitrate); on the right, the active view's "N tracks · total playtime", which switches to the selection's total when you select two or more rows.
- **Play-status glyph.** The leftmost column of the track list now shows a play or pause icon on the row that is the currently playing track, the deadbeef ♫ marker. It follows track changes and pause, and it survives filtering. This is the per-row status glyph owed since Phase 3c, now that playback state exists.
- **Channels.** Read live from the player (mpv's decoded channel count) rather than stored, so it shows while a track plays without a schema change or a re-import.
- **Tests.** The aggregate, the playtime and thousands formatting, the technical line, and the glyph-state selection are pure and unit-tested; the widgets are verified by building and by hand (the established pattern for GUI work). Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. No new dependency, no schema change.

## v0.0.73

Phase 10c: configurable browse panes. The facet columns down the left of the music browser are no longer fixed at Genre, Album Artist, Album. This completes Phase 10 (Configuration & preferences).

- **Pick your browse columns.** Preferences → Library → Browse panes gives five ordered slots; each is a field or "(none)". Choose from Genre, Shelf Genre, Album Artist, Artist, Album, Year, and Format, in any order, one to five panes. The change saves to `config.toml` and takes effect on the next launch.
- **Four new facet fields.** Beyond the original three, the browser can now facet by Shelf Genre, Artist (the track artist, distinct from the album artist), Year, and Format, all from data already in the library.
- **Safe by default.** Unknown field names in the config are ignored, the list is capped at five, and an empty list falls back to the default Genre → Album Artist → Album, so the browser is never left without panes.
- **Tests.** The field parsing, the config resolution (skip unknown / cap / default), and the new facet queries (each partitions the library; the cascade still narrows) are tested; the editor and the config-driven build are verified by building and by hand. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. No new dependency, no schema change.

## v0.0.72

Phase 11a: a track properties inspector and a large cover panel, the first slice of the Columns UI parity pass.

- **Properties inspector.** A collapsible panel on the right of the browse window (a header button, or `Ctrl+P`) showing the selected track's full detail: title, artist, album, year, genre, track and disc, duration, format, bitrate, sample rate, file size, ReplayGain, rating, play count, last played, date added, file location, MusicBrainz ids, and the cover file. Read-only, and it updates as you move the selection. Everything comes straight from the database or a quick file-size check.
- **Large cover.** The album art at a readable size sits above the properties, tinted with the album's accent colour, distinct from the small thumbnail in the playback bar. A placeholder shows when a track has no cover.
- **Notes.** The panel shows the first selected track when several are selected, and it costs nothing while closed. Channels is the one deadbeef field not shown yet: it is not stored and would need decoding the file, so it is deferred.
- **Tests.** The field projection (track and album to the displayed rows) is pure and unit-tested; the panel itself is verified by building and by hand (the established pattern for GUI work). Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. No new dependency, no schema change.

## v0.0.71

Phase 10b: a Preferences window. The settings introduced in 10a are now editable from the GUI instead of by hand-editing the config file.

- **Preferences dialog.** The former Sound dialog (still `Ctrl+,`, now a preferences button in the header) becomes a full Preferences window with three pages: General, Library, and the existing Sound.
- **General page.** Set the library root with a folder chooser, the music path template, the import mode (copy or move), whether edits are embedded back into files, and the fallback genre. Changing the library root takes effect on the next launch (the running session keeps the library it opened with), and the page says so.
- **Library page.** The podcast and audiobook defaults: their library subfolders, the audiobook path template, default playback speed, and the Smart Speed / Voice Boost toggles. The browse pane configuration will join this page in a later release.
- **Where settings live.** The General and Library pages read and write `config.toml`; the Sound page keeps managing the audio engine (equalizer, ReplayGain, dynamics, output) in the database exactly as before. Nothing about the working audio settings changed.
- **Tests.** The import-mode mapping is unit-tested; the dialog itself is verified by building and by hand (the established pattern for GUI work), and it saves through the same config writer covered by the 10a tests. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. No new dependency, no schema change.

## v0.0.70

Phase 10a: Conservatory's first config file. This is the foundation of Phase 10 (Configuration & preferences); it introduces `~/.config/conservatory/config.toml` and makes the GTK app find its library from there instead of requiring a command-line argument.

- **`config.toml`.** A new config file at `$XDG_CONFIG_HOME/conservatory/config.toml` (falling back to `~/.config`). It holds the app and library settings: the library root, the music/audiobook path templates, the import mode (copy or move), the genre fallback, the podcast and audiobook subdirectories and book defaults, and the browse pane layout. Every field has a sensible default, so a missing or partial file just works; only a genuinely malformed file is reported as an error rather than silently reset.
- **Library root from config.** Launching the GTK app with no arguments now reads the library root from `config.toml`. A path given on the command line still wins (handy for development), but it is no longer required.
- **What stays in the database.** The audio engine settings (ReplayGain, equalizer, dynamics, output) deliberately stay in the database where the Sound dialog already manages them live; the config file does not duplicate them. This keeps the working audio settings untouched.
- **`config` CLI verb.** `conservatory-cli config path` prints the file location, `config show` prints the effective settings as TOML, and `config init` writes a default file (without ever overwriting an existing one).
- **Still to come in Phase 10:** a Preferences page to edit these settings in the GUI (10b), and configurable browse panes (10c).
- **Tests.** The config load/save/merge paths are unit-tested (default round-trip, partial-file merge, missing-file defaults, malformed-file error, XDG path resolution) along with the GTK root-precedence helper, and verified end to end via the `config` verb. No new dependency (the `toml` crate was already in the workspace); no schema change. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green.

## v0.0.69

Phase 8d: playlist export and import as portable `.m3u` / `.m3u8` files. This is the last slice of Phase 8, so library maintenance (verify, duplicates, audit, stats, APE strip, playlists) is now complete.

- **`playlist export`.** `conservatory-cli playlist export <db> '<expr>' <out.m3u>` writes the tracks matching a search expression to an extended-M3U playlist (with `#EXTINF` duration and `Artist - Title` lines), in album order. The selector is the full search grammar, and it accepts `vl:NAME` to export a saved Perspective. Paths are library-root-relative by default (portable when the `.m3u` sits at the library root); `--absolute` (with `--root`) writes full paths instead.
- **`playlist import`.** `conservatory-cli playlist import <db> <in.m3u>` reads a playlist, resolves each path back to a managed track, and loads the play queue (appends by default; `--replace` replaces it). Paths that match no track in the library are reported and skipped, never fatal, so a playlist that points partly outside the library still imports what it can. Pass `--root` to map absolute playlist paths back to the library.
- **`vl:NAME` everywhere.** Wiring the Perspective resolver into the shared selector path means `vl:NAME` now works not just in `playlist export` but in every selector-taking verb (`search`, `verify`, `duplicates`, `audit`), where it previously fell back to a plain text search.
- **Tests.** The `.m3u` build/parse round-trip is unit-tested; verified end to end by exporting `format:mp3` to a playlist (relative and absolute) and re-importing it to the exact same eight tracks, plus a `vl:NAME` export and a missing-path import. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green. No new dependency, no schema change.

## v0.0.68

Phase 8c-iii (part 2 of 2): the `apestrip` command removes the stray APEv2 tags that `audit --tier ape` finds. This is the byte-level fix deferred since Phase 5b (lofty cannot remove an APE tag from an MP3). It mutates your files, so it is built with the full set of safeguards: dry-run by default, crash-safe writes, and a reversible undo. With this, Phase 8c (library health) is complete.

- **`apestrip` CLI verb.** `conservatory-cli apestrip <db> --root <root>` previews which MP3s carry a stray APE and changes nothing (dry-run). `--apply` strips them; `--undo` puts them back. A stray APE shadows the file's ID3 in foobar2000 / DeaDBeeF, so removing it lets your curated tags win.
- **Safe by construction.** Every strip writes a sibling temp file, fsyncs it, decode-checks it (the result must still parse as valid audio), then atomically renames it over the original; a failure leaves the original untouched. Before any file is touched, the exact bytes being removed are recorded in the database, so `--undo` restores the file precisely. Undo refuses a file that changed since the strip, rather than clobbering it.
- **Migration deferred.** Optionally migrating APE fields into ID3 before stripping is left for a follow-up; Conservatory's database is the source of truth and already writes canonical ID3, so removing the shadow is the win here.
- **Tests.** The byte-level splice and the full strip-then-undo roundtrip on a real MP3 are tested, plus a database migration (0015) for the undo journal. Verified end to end (dry-run, apply to a byte-identical result that still decodes, and undo to the exact original). Full workspace suite + clippy `-D warnings` + fmt + the music-only build green.

## v0.0.67

Phase 8c-iii (part 1 of 2): the audit can now detect MP3s carrying a stray APEv2 tag. An APE tag sitting on an MP3 shadows the file's ID3 in players like foobar2000 and DeaDBeeF, silently defeating tag edits. This release reports them; the next will strip them.

- **`audit --tier ape`.** A new audit tier (`conservatory-cli audit <db> --tier ape --root <root>`) flags every MP3 that carries a stray APEv2 tag. Like the cover-art checks it needs `--root` (it reads each file); it scans only the last 128 KB of each MP3, so it is cheap.
- **A careful byte parser.** The detection is a hand-rolled APEv2 parser that anchors on the tag's footer and validates it (the spec-mandated reserved-zero bytes, and a header-consistency check), so a stray "APETAGEX" sequence inside the audio is not mistaken for a real tag.
- **Tests.** The parser is unit-tested against synthesized tags (with and without a header, with and without a trailing ID3v1, and the false-positive guard), plus a filesystem test for the audit tier. Verified end to end (a clean library reports none; a crafted APE tag is flagged). Full workspace suite + clippy `-D warnings` + fmt + the music-only build green.

## v0.0.66

Phase 8c-ii: a `stats` command that summarizes the library, the second slice of the Phase 8c maintenance work. It is a port of Lattice's statistics report, run over Conservatory's database. Read-only. No new dependency, no migration. With this, only the stray-APE detect + strip (8c-iii) remains in Phase 8c.

- **What it reports.** An overview (track / album / artist totals, total size, total duration, and the share of fully-tagged tracks), a per-format breakdown with sizes and percentages, a bitrate summary (average, range, and how many files fall below 192 kbps), the rating distribution as a small histogram, the genre distribution with a per-genre rating tally, and the top artists by track count.
- **File sizes need `--root`.** The database stores everything except the size of each file on disk, so the size figures come from a quick pass over the files and need `--root`; without it the report prints sizes as "n/a" and still computes everything else. Rating 0 is treated as unrated (Conservatory's default), with 1 to 5 as stars.
- **`stats` CLI verb.** `conservatory-cli stats <db> --root <root>` prints the full report; `--top N` sets how many genres and artists to list (default 15), and `--format tsv|json|human` (human default) picks the output.
- **Tests.** The aggregation is unit-tested over planted rows (format counts, bitrate average / range / below-floor, the rating tally, genre and artist breakdowns, the fully-tagged predicate), with a filesystem test for the size pass. Verified end to end on real albums (14 tracks across two albums: 94.6 MB, 1h 5m, correct format split and bitrate range). Full workspace suite + clippy `-D warnings` + fmt + the music-only build green.

## v0.0.65

Phase 8c-i: an `audit` command that reports library health problems, the third Phase 8 audit (after integrity and duplicates). It is a faithful port of Lattice's tag / bitrate / ReplayGain / cover-art audits. Read-only: it reports, it never changes a file. No new dependency, no migration. Phase 8c is large, so it is sliced: this is the audits; library statistics (8c-ii) and the stray-APE detect + strip (8c-iii) follow.

- **Five health checks.** (1) Missing critical tags: title, artist, track number, or genre. (2) Low bitrate: lossy files below a floor (default 192 kbps); lossless formats are never flagged. (3) ReplayGain coverage per album: missing, partial, no-album-gain, or ok. (4) Missing cover art. (5) Low-resolution cover art below a pixel floor (default 500×500), read from the cover file's header.
- **Opus R128 is understood.** Opus files often carry loudness as the `R128_*` convention rather than the standard ReplayGain tags. When you pass `--root`, the audit reads those tags directly for any Opus track the database does not already know a gain for, so a properly tagged Opus album is not falsely reported as missing ReplayGain.
- **`audit` CLI verb.** `conservatory-cli audit <db> --root <root>` prints a sectioned report; `--tier tags|bitrate|replaygain|art|artres` limits which checks run, `--bitrate-floor` and `--min-art-px` tune the thresholds, and `--format tsv|json|human` (human default) picks the output. The cover-file, art-resolution, and Opus R128 checks need `--root`; without it the audit says so rather than silently skipping. Always exits 0 (a deficiency is a report, not an error).
- **Tests.** Each check is unit-tested against a planted set, with a filesystem test for the cover-art path using generated images. Verified end to end on real albums (it correctly caught an Opus track one kbps under the floor and recognized the R128 ReplayGain on the same album). Full workspace suite + clippy `-D warnings` + fmt + the music-only build green.

## v0.0.64

Phase 8b: a `duplicates` command that finds redundant copies in your library, the second Phase 8 audit. It is a faithful port of Lattice's duplicate detector. Read-only: it reports, it never deletes (cleanup goes through `organize`, with its dry-run and undo). One new dependency (`unicode-normalization`, for matching names that differ only in unicode form); no migration.

- **Four kinds of duplicate.** (1) The same album sitting in two folders. (2) The same track ripped to several formats in one album (flac + mp3). (3) Near-miss album names by the same artist, e.g. "Domestica" vs "Domestica (Deluxe Edition)", via a fuzzy match. (4) The same recording (artist + title) living in multiple albums, grouped by duration so a studio take and a live take of the same song are reported separately, not lumped together.
- **Robust name matching.** Names are normalized the way Lattice does before comparison: unicode NFKC folding, curly quotes and dashes folded to ASCII, whitespace and case collapsed; the fuzzy tier additionally strips "feat./ft." clauses and trailing parentheticals. The fuzzy similarity is a hand-rolled port of Python's difflib ratio, so the 0.85 threshold behaves exactly as it does in Lattice.
- **`duplicates` CLI verb.** `conservatory-cli duplicates <db>` prints a four-section report; `--tier exact|multiformat|similar|tracks` limits which sections run, and `--format tsv|json|human` (human default) picks the output. Always exits 0 (a duplicate is not an error).
- **Tests.** Each tier is unit-tested against a planted set, plus the normalization vectors and the difflib-ratio parity. Verified end to end on real albums (a clean library reports nothing). Full workspace suite + clippy `-D warnings` + fmt + the music-only build green.

## v0.0.63

Phase 8a: a `verify` command that decode-checks your files for corruption. This is the first piece of Phase 8 (library maintenance and audits), modeled on Lattice's integrity tests. One database migration (0014); no new third-party Rust dependency (it shells out to `flac` and `ffmpeg`, which need to be on your PATH).

- **Decode-verify with a four-tier verdict.** Each file is test-decoded and sorted into OK, METADATA (audio fine, only a container/tag note), SUSPECT (decoded to the end but the tool flagged something), or CORRUPT (the decoder errored, or a FLAC came up short, i.e. truncation or bit-rot). FLAC goes through `flac -t`, which MD5-verifies the decoded stream and so catches rot a lenient player would skip; every other format goes through a strict `ffmpeg` decode. A benign-note allowlist keeps a clean mp3's routine "estimating duration" warning from flagging it.
- **`verify` CLI verb.** `conservatory-cli verify <db> '<search>' --root <root>` checks the matched tracks in parallel and prints a per-tier summary; `--verbose` lists each flagged file with the tool's message. The process exits non-zero only when CORRUPT files exist, so it drops straight into a cron or pre-backup hook.
- **Cached, so re-runs are cheap.** Each verdict is stored keyed by path plus the file's size and modification time; a re-run skips files that have not changed (pass `--force` to re-check everything). The cache is path-keyed, so podcasts and audiobooks can be folded into the same verifier later with no schema change.
- **Migration 0014** adds the `verify_results` table.
- **Tests.** Pure classifier units for both tools (including the benign-note regression), an availability-gated integration test (a clean FLAC fixture verifies OK, a truncated copy verifies CORRUPT), and the cache round-trip through the worker. Verified end to end against real albums. Full workspace suite + clippy `-D warnings` + fmt + the music-only build green.

## v0.0.62

Phase 7c-iii, and with it **Phase 7 is complete**: audiobooks are now a full third media type. You can play a book from the shelf, see it on the Now Playing surfaces, control it from GNOME's media keys, and tune its playback per book. This is the last of three audiobook-playback commits; no migration, no new third-party dependency.

- **Play from the shelf.** Double-click (or press Enter on) a cover and the book, plus the rest of the shelf below it, starts playing in the one unified queue alongside music and podcasts. Ctrl+Enter appends the selected books to the queue tail instead.
- **The book on the Now Playing surfaces.** The Now-bar and the Now Playing drawer now show a book's title, author, series, duration, and cover, with a clickable chapter list that jumps anywhere in the book (across its files) and highlights the current chapter as it plays. The queue drawer shows books too, and a book left playing when you quit resumes on next launch.
- **Media keys and the lock screen.** MPRIS now reports the right metadata for whatever is playing, books included, so GNOME's media overlay and the keyboard media keys drive a book the same as a song.
- **Per-book playback settings.** A gear in the book's detail pane opens a small dialog for that book's speed, Smart Speed, and Voice Boost. Changing them never disturbs your saved place in the book.
- **Tests.** A pure `book_fields` projection (the Now Playing field list) is unit-tested for author / series / format / decimal series index; the play, resume, and profile logic is covered by the 7c-i / 7c-ii engine tests. Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

## v0.0.61

Phase 7c-ii: a book now resumes exactly where you left off, honors its own playback settings, and skips chapters across files. This is the second of three audiobook-playback commits; the GTK surface, MPRIS, and a per-book settings dialog are 7c-iii. One database migration (0013); no new third-party dependency.

- **One timeline across a book's files.** The engine now speaks a "book-absolute" position: it maps each file's own clock onto a single timeline that runs from the start of the book to its end. Everything that reports or uses position (the seek slider, the resume write, the chapter highlight, the time-saved accounting) now speaks that one timeline, so a multi-file book behaves like a single long item.
- **First-class resume.** A book's absolute position is written on pause, on seek, and on the periodic insurance interval, and the transport cursor records which book was playing. `audiobook play --resume` reopens a book and seeks straight back to where you stopped, to the second; a slider seek or a chapter skip that crosses a file boundary loads the right file and lands at the right offset.
- **Per-book playback settings.** Audiobooks share the podcast spoken-word engine (variable speed, Smart Speed, Voice Boost), now resolved from each book's own overrides rather than a show's. `audiobook settings <db> <id> --speed 1.5 --smart-speed true --voice-boost true` sets them; playback applies them.
- **Time saved counts for books too.** The listening-session history table was rebuilt so a session can belong to a book as well as an episode (exactly one of the two), so a book's Smart Speed time-saved feeds the same `stats` totals.
- **Migration 0013.** Adds the book cursor column and rebuilds `listening_sessions` with a nullable book/episode owner and an exactly-one check. Existing episode history is copied forward unchanged.
- **Tests.** Six integration tests now drive the real engine over a null audio output: the resume position and cursor persist mid-book, a cross-file seek reaches the third file, the chapter readout progresses across all three files, and a completed book writes exactly one book-keyed session, plus the `resolve_book_profile` defaults/overrides/clamp units. Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

## v0.0.60

Phase 7c begins: audiobook playback. This first commit (7c-i) is the headless engine that makes a book play as one item in the unified queue. The novel piece is that a book's chapters can live in one M4B or across a folder of one-file-per-chapter, and either way the book is a single queue entry the engine plays through internally. No new third-party dependency, no schema change.

- **A book is one playable item with a segment plan.** A new pure module plans a book's chapters into ordered per-file "segments", each tagged with its cumulative offset across the whole book, and lifts the chapter marks onto one absolute book timeline. That plan is what lets the engine speak a single position from 0 to the end of the book regardless of how many files back it.
- **The engine advances file to file internally.** When one file ends, the engine loads the next file of the same book without advancing the queue, and only marks the book finished (clearing its resume position) at the last file's end. A multi-file book that reports "finished" is therefore proof every file played. An M4B is the one-file case: its chapters are seeks inside the single file, fully gapless.
- **The gapless tradeoff, stated plainly.** Multi-file books advance with one fresh load per file, so there is a brief gap at each file boundary (a natural chapter pause); M4B books have none. True cross-file gapless is a later refinement, recorded in the roadmap.
- **CLI `audiobook play <db> <id> --root`** plays a book headlessly through the libmpv engine, printing each chapter as it advances, with the shared `--sleep` timer. The position-persistence resume and per-book speed / Smart Speed / Voice Boost land in 7c-ii; the shelf play button, Now Playing surface, and MPRIS in 7c-iii.
- **Tests.** The segment math is unit-tested (single-M4B, multi-file, multi-file-with-internal-chapters, missing-duration degrade, position-to-segment locate). Two integration tests drive the real engine through a null audio output: a three-file book advances through every file and completes, and a single-file book with chapter marks completes. Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

## v0.0.59

Phase 7b-iii-b completes Phase 7b: the Audiobooks tab gets a bulk-edit dialog, the GTK front end for the editing engine that landed in 7b-iii-a. Select one or more books on the shelf, press `Ctrl+E` (or the pencil button), edit their fields, and a path-affecting change re-shelves the files behind a preview-and-confirm. No new third-party dependency.

- **The shelf is multi-select now.** A plain click still selects a single book (so the detail browse is unchanged, and the detail pane follows the first selected book), but Ctrl-click and Shift-click extend the selection for a bulk edit.
- **The bulk-edit dialog.** A pencil button on the filter bar and `Ctrl+E` (scoped to the Audiobooks tab, so it never fights the Music tab's shortcuts) open an editor over the selection: author(s) and narrator(s) (semicolon-separated, replacing the credited set), series, series index, title, year, shelf genre, and rating, plus a "Standalone (no series)" checkbox. Blank fields are left unchanged; one bad value rejects the whole edit rather than applying it half-done (the Calibre / music-surface model).
- **Series clearing is a checkbox.** Since blank means "unchanged", making a book standalone is the explicit "Standalone (no series)" toggle, which moves it to `Audiobooks/<author>/Standalone/…`.
- **Path-affecting edits move files behind a confirm.** After writing the metadata, the dialog aggregates the planned moves across every edited book into a single "Move N files?" prompt; confirm and each book re-shelves through the same journaled mover (dry-run preview, undo journal, crash-safe replay) the headless half built, then the shelf reloads. A non-path edit (rating, narrator, shelf genre) just reloads, no move.
- **Tests.** The edit and move logic is covered by the 7b-iii-a suite (the resolver units and the reorganize round-trip on real files); the dialog itself is verified by build plus manual launch, the documented GTK-view precedent. Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

Phase 7b is complete: the audiobook library is browsable, filterable, and editable. Next is Phase 7c, the last of Phase 7: audiobook playback on the unified queue, with chapter navigation, variable speed, the sleep timer, and first-class resume.

## v0.0.58

Phase 7b-iii-a: the headless half of audiobook bulk edit. You can now edit a book's metadata and, when a path-affecting field changes (author, series, series index, title, year), have its files re-shelved into the new folder through the same trust-critical, journaled mover that owns the music library. This sub-phase is the engine; the GTK multi-select dialog (7b-iii-b) is next. No new third-party dependency, no schema change.

- **The book reorganize path.** Until now the only way a book's files moved was the initial import. This adds the audiobook analogue of music's `organize`: re-render the book's folder from its current (edited) database state and move the files there. The mover needed no audiobook-specific change. A move operation carrying a book id already rewrites every chapter row of that book and updates its folder, the same under a re-shelve as under an import, so a single M4B that backs many chapters still moves once and all its chapters follow. The dry-run preview, the undo journal, and crash-safe replay all apply unchanged (a move bug damages a real library; this is the headline risk).
- **Typed, shared edit logic.** A pure `BookEdit` resolver classifies which fields are path-affecting (author, series, series index, title, year, the ones in the folder template) versus not (narrator, shelf genre, rating, starred). It is the single source the CLI and the coming GTK dialog both build their edits from, and it is unit-tested with no database.
- **Series can be cleared to standalone.** Setting a series files the book under it; clearing the series moves the book to `Audiobooks/<author>/Standalone/…`. Blank still means "leave unchanged", so clearing is an explicit action (`--series ""` on the CLI).
- **CLI `audiobook set` grew teeth.** It now takes `--title`, `--year`, `--author`, `--narrator`, `--series`, and `--series-index` alongside the existing rating/starred/shelf-genre. A path-affecting edit needs `--root`; without `--apply` it is a dry-run that shows the current and would-be folders and writes nothing. With `--apply` it writes the metadata and re-shelves. Undo is the existing `organize --undo <job>`.
- **The cover follows.** When a book moves, its `cover.jpg` is carried into the new folder and the path updated (best-effort: covers re-derive and the accent already lives on the row, so a cover hiccup never fails the move).
- **Tests.** The resolver's path-affecting matrix and value parsing; a reorganize round-trip suite against real files on disk (a multi-file book, a single-M4B book whose three chapters all follow the one moved file, undo restoring the tree and the rows, an in-place no-op, and a destination conflict being refused with nothing moved). Verified end-to-end against the committed import fixture. Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

Next: Phase 7b-iii-b, the GTK side — a multi-select shelf and a bulk-edit dialog (`Ctrl+E`) whose path-affecting edits re-shelve behind a move preview-and-confirm.

## v0.0.57

Phase 7b-ii: the Audiobooks shelf gains a filter bar. The same Calibre-style search grammar that drives the Music tab now sits above the cover shelf, with the audiobook fields wired in. This is the first non-music consumer of the shared grammar (the podcast tab never had a filter bar), so the four book fields joined the shared field set rather than forking it: one grammar, all three surfaces, as the spec intends. No new third-party dependency.

- **The filter bar:** an always-on search entry above the shelf, no separate search mode (the Music-surface idiom). Type `author:sanderson`, `series:"The Stormlight Archive"`, `narrator:kramer`, or `is:finished` and the shelf narrows in place, keeping its in-progress-first order. `is:starred` and the shared numeric grammar (`rating:>=4`, `year:2010`, `duration:`) work on books too. `Ctrl+F` focuses the bar; it is scoped to the Audiobooks tab so it does not fight the window's global music `Ctrl+F`. A malformed expression tints the bar yellow rather than erroring (the forgiving parser).
- **`is:finished`:** a new state, the same shape as `is:played` and `is:starred`. To find unfinished books, write `NOT is:finished`. (The spec table's `is:finished false` example predates the actual `is:` mechanics, where states take no value; `NOT` is the negation everywhere.)
- **Shared grammar, eval-only for books:** `author:` / `narrator:` / `series:` / `is:finished` are now known on every surface, so a book field typed into the music bar simply matches nothing instead of being an unknown field. They never translate to the music `tracks` SQL: the translator returns "can't express this," which forces the whole query onto the in-memory path, where the audiobook shelf (small, already loaded whole) is matched directly. No SQL change, no schema change.
- **Headless and CLI-testable:** the filter logic lives in a pure `book_query::filter_books` (unit-tested: fielded matches, `is:finished`, bare-text author search, forgiving degrade), and the `audiobook list` CLI verb gained an optional filter expression (`conservatory-cli audiobook list <db> "author:sanderson AND NOT is:finished"`), the headless twin of the shelf filter.
- **Tests:** the grammar extension (the new fields parse, round-trip, eval correctly, and are excluded from SQL translation, keeping the music SQL path safe); the `book_query` filter model (five cases); the CLI mapping; and the existing music search-parity suite still green (the shared `SearchItem` change is additive). Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. The widget wiring is verified by build plus manual launch (the documented GTK-view precedent).

Next: Phase 7b-iii (bulk edit across selected books, path-affecting edits through the mover), then 7c (playback, chapter navigation, and resume on the unified queue).

## v0.0.56

Phase 7b-i: the Audiobooks tab arrives, the third media surface. The shelf is a cover grid (the app's first `GridView`, every other browse being a column table) beside a detail pane, and it is the first place the median-cut cover accent is used in the GUI. This sub-phase is read-only browse; a book becomes playable at Phase 7c, so there is no play action yet (the same browse-before-playback split podcasts took). Filtering and bulk edit follow in 7b-ii/iii.

- **The shelf:** every audiobook as an accent-tinted cover tile, ordered in-progress first (then new, then finished), with the title and author beneath. Click a tile and the detail pane on the right fills with the cover, the title, an author · narrator · series · year line, a progress bar with the derived state, and the book's chapter list. A side-by-side `gtk::Paned`, matching the Podcasts and Music tabs; the cover loads from the managed `cover.jpg`, falling back to a placeholder.
- **State derivation:** New / In progress / Finished is derived (not stored) from the `book_playback` resume row, and the shelf surfaces in-progress books first (most recently played first). The derivation and the ordering are pure, tested functions in core, so the GUI shelf and the new `audiobook list` CLI verb share one source of truth.
- **One core read for the shelf:** `list_book_rows` returns a denormalized row per book (author and narrator credits, series, summed chapter duration, and resume state) in a single query, the podcast `EpisodeListRow` precedent, so the grid never does an N+1 of per-book lookups.
- **CLI:** `conservatory-cli audiobook list <db>` prints the shelf rows with their derived state, the headless view of the GUI shelf (the every-surface-CLI-testable rule).
- **Plumbing:** the multi-view chrome (the header switcher, the adaptive bottom bar, the narrow breakpoint) moved out of the podcasts attach into a shared step, so podcasts and audiobooks are now independent compile-time plugins: either one alone still gets the switcher, and a music-only build is unchanged. The shell already reserved the Alt+3 slot; the Audiobooks page is built lazily on first view.
- **Tests:** the shelf read and ordering (denormalized credits / series / duration, and in-progress-first ordering beating alphabetical) and the `BookState` derivation in core; the tile row formatting (progress fraction, the "Author · Read by Narrator · Series · Year" line, the state label, decimal series numbers) as `BookRow` unit tests. The widget tree itself is verified by build plus manual launch (the documented GTK-view precedent). Manually checked against the two real test books (the Rothfuss series book and the Gaiman full-cast standalone) imported into a shelf. Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build (and the audiobooks-without-podcasts build) green.

Next: Phase 7b-ii, the filter bar and bucket sidebar wired to `conservatory-search` (`author:` / `narrator:` / `series:` / `is:finished`), then 7b-iii (bulk edit) and 7c (playback, chapters, resume).

## v0.0.55

Phase 7a-iii closes Phase 7a: the audiobook import pipeline. The 7a-ii reader turned files into a `BookDraft`; this sub-phase resolves that draft into database rows and moves the book's files into the managed `Audiobooks/` tree through the same trust-critical, journaled file mover that owns the music library. Point the CLI at a folder or a single `.m4b` and you get an organized, database-owned audiobook with ordered chapters, headless. No new third-party dependency.

- **The audiobook path template:** `Audiobooks/{author}/{series}/{series_index:02}. {title} ({year})`, with four new tokens (`{author}`, `{narrator}`, `{series}`, `{series_index}`) added to the Phase 2a engine. A standalone book renders under the literal `Standalone` folder, so every author folder is two levels deep; the index/year groups collapse when absent (a standalone drops the `NN. ` prefix). The series index is decimal-aware: an integral `1.0` zero-pads to `01`, a fractional `1.5` renders as `1.5`. The render loop is now shared between music and books through a small internal trait, so music paths are byte-for-byte unchanged (the existing path tests are the guard).
- **The mover learns about books:** books are owned and moved like music, not ephemeral like podcasts, so the move runs through the journaled mover with its dry-run preview, undo, and crash-safe roll-forward (a move bug damages a real library; this is the headline risk). The journal gains a `book_id` column (migration `0012`). One subtlety drove the design: a single M4B can back many chapters, so move operations are built per unique physical file, not per chapter, and completing a book op rewrites every chapter row of the book whose path matches the moved file. A per-chapter-file book and a single-M4B book both land correctly.
- **Import:** `import_book` (in the `conservatory-audiobooks` plugin, calling the core mover and worker) runs the music importer's two-pass shape. A pure resolve pass renders the folder and pre-checks the move for conflicts; only if the plan is clear does the persist pass create the `book` / `book_people` / `series` / `book_chapters` rows and run the move. A conflict (the folder already exists, or two files collide) refuses the whole import with nothing written. Scope is one book per call; a whole-`Author/*`-tree batch is a later add.
- **Cover and accent:** the median-cut accent (the Hermitage path) into `books.accent_rgb`, and the cover written into the moved book folder via the existing folder-based cover sync. A book with embedded cover art in its chapter files gets a `cover.jpg` written beside them.
- **CLI:** `conservatory-cli audiobook import <db> <source> <root>` (copy by default, `--move` to consume; a conflict exits nonzero with nothing written) and `audiobook set <db> <book_id>` (rating, starred, shelf genre; path-affecting edits are deferred to the 7b bulk-edit surface). Undo is the existing media-agnostic `organize --undo <job>`.
- **Tests:** the book path render (the default template, the standalone fallback, integer and decimal indices, collapsing groups, per-component sanitization, and the unchanged music regression set); the mover book round-trip suite (multi-file and the single-M4B many-chapters case, undo restoring both tree and DB, and crash replay); and the end-to-end import of the committed multi-file fixture (the resolved rows, the moved files under the rendered folder, and a second import refused with no partial writes). Manually verified against the real Rothfuss series book: 94 chapters land under `Audiobooks/Rothfuss, Patrick/The Kingkiller Chronicle/01. The Name of the Wind (2009)/` with the embedded cover extracted, and copy mode leaves the source intact. Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

Next: Phase 7b, the Audiobooks GTK tab (the third view-stack page, a cover-grid shelf plus a book detail pane), then 7c (playback, chapter navigation, and resume on the unified queue).

## v0.0.54

Phase 7a-ii: the audiobook reader. The `conservatory-audiobooks` plugin crate, an empty stub until now, gets the piece that turns files on disk into a structured book: the tag and Audiobookshelf-sidecar reader and the chapter resolver. The output is a `BookDraft` (metadata plus an ordered chapter list); the database writes, file moves, and the `audiobook import` verb are the next sub-phase (7a-iii). No new third-party dependency.

- **The reader (`read_book`):** point it at a book folder or a single audio file and it produces a draft. Metadata comes from three sources, merged per field by precedence **sidecar > embedded tags > folder structure**: the explicit Audiobookshelf sidecars win, the embedded tags are the common case, and the on-disk layout is a last-resort fallback. Every field is best-effort, so an untagged folder still yields a draft with its chapters.
- **Embedded tags (lofty):** the audiobook convention, validated against real books. The book title is the album tag, the author is the album artist, the narrator is the composer (plus a custom `NARRATOR` frame), and series and sequence come from custom `SERIES` / `SERIES-PART` frames. Custom frames are read across both the ID3v2 `TXXX` and MP4 freeform spellings, so one path covers mp3 and m4b. People are split out of a packed credit string (a full-cast book lists every narrator in one field) and sorted last-name-first ("Rothfuss, Patrick"), the Calibre author-sort convention.
- **Sidecars:** `metadata.opf` (Dublin Core plus Calibre series tags, via `quick-xml`), `desc.txt`, and `reader.txt`, the Audiobookshelf conventions.
- **Folder inference:** `Author/Title` is a standalone book, `Author/Series/NN - Title` a series entry. Recognised conservatively, so a numerically-titled standalone (`1984`) is not mistaken for a series entry.
- **Chapters:** a chapter addresses either a standalone per-chapter file or a span inside one M4B. One file with embedded markers reads them via an `ffprobe -show_chapters` shell-out (lofty cannot read MP4 chapter atoms; this avoids a Rust MP4 dependency, the rsgain precedent and the m4b-tool technique); one file without markers is a single whole-file chapter; a multi-file folder is one chapter per file, ordered by the part tag (a multi-part M4B numbers `1/11 .. 11/11`, so a lexical filename sort would put "Part 10" before "Part 2"). `ffprobe` is best-effort: a missing binary degrades to a whole-file chapter, never an error.
- **The artifact:** `conservatory-cli audiobook debug-read <path>` prints a draft (title, authors and narrators with sort names, series and sequence, and the ordered chapter list). It reads the two real test books correctly: the Rothfuss series book (94 one-file-per-chapter parts, narrator Nick Podehl) and the Gaiman full-cast M4B standalone (11 parts ordered by tag, seven narrators).
- **Tests:** the pure logic is unit-tested (the people splitter, the person sort name, the raw-chapter-to-draft mapping, the part ordering, the folder inference, the `.opf` parse, the precedence merge); three fixture-backed integration tests cover `read_book` end to end (a multi-file book, a single M4B with embedded chapters gated on `ffprobe`, and a sidecar overriding the embedded tags), with the small fixtures generated by `cargo run -p conservatory-audiobooks --example gen_fixtures`. CI gains a best-effort ffmpeg install (the rsgain pattern) so the gated test runs there. Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

Next: 7a-iii, the audiobook path-template tokens (`{author}` / `{series}` / `{series_index}`), the import pipeline that resolves a draft into rows and moves the book into the managed tree (the file mover with dry-run and undo), cover and accent, and the `audiobook import` / `audiobook set` CLI verbs.

## v0.0.53

Phase 7 begins: audiobooks, the third media type. This first sub-phase (7a-i) is the headless schema foundation, with no user-facing surface yet. It lands the audiobook tables in the core migration ledger so the reader, import, and browse work that follows has a database to write to. Nothing about music or podcasts changes.

- **Migration `0011` (spec §4.5):** seven tables modeled on Audiobookshelf's relational shape and Cozy's Book → Chapter → file model. `book_people` (authors and narrators share one table, role-tagged by which link table credits them), `series`, `books`, the `book_authors` / `book_narrators` link tables, `book_chapters` (each row addresses either a standalone per-chapter file or a span inside one M4B, via `file_path` + `file_offset`), and `book_playback` (one row per book; first-class resume holding the absolute position, finished flag, and per-book speed / Smart Speed / Voice Boost overrides). The schema is core-owned even though audiobooks are a plugin: the boundary is code and dependencies, not the database (spec §2.2), so a music-only build still migrates to it with empty tables.
- **`book_fts` (spec §4.4):** full-text search over title, author, narrator, and series. Unlike the podcast `episode_fts`, the author / narrator / series columns denormalize from the link tables rather than a single row, so the index is maintained by triggers on `book_authors` / `book_narrators` (a space-joined `group_concat` re-aggregated as links change) plus rename-propagation triggers on `book_people` and `series` (the music `artists_au` precedent).
- **The unified queue gains its last foreign key.** Migration `0005` parked `queue.book_id` as a plain column because SQLite refuses a foreign key to a table that does not exist yet; `0011` rebuilds `queue` to add the `book_id REFERENCES books(id) ON DELETE CASCADE` now that `books` is real (the same treatment `0006` gave `episode_id`). All three queue id columns now carry their cascade.
- **Core CRUD, headless and CLI-testable:** the `Book` / `BookPerson` / `Series` / `BookChapter` / `BookPlayback` models, the single-writer write path (get-or-create people and series by their unique sort-name / name, insert a book, link authors and narrators idempotently, replace an ordered chapter set, upsert resume state, and the position / completion writes the Phase 7c player will use), and the read helpers (a book, its authors / narrators / chapters / series / resume row, and the book list).
- **Tests:** a worker round-trip (people, series, a book, role links, ordered chapters, and resume state insert through the writer and read back through the pool intact, with sort-name / name dedup and idempotent links); `book_fts` denormalization from the link tables, including a re-aggregation on a second author and a rename propagating back into the index; the position / completion writes (resume un-finishes a finished book); and the queue `book_id` foreign key enforcing (a queue row pointing at a missing book is rejected). Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new dependency.

Next: 7a-ii, the tag and Audiobookshelf-sidecar reader plus the chapter resolver. That sub-phase carries the one real design decision of the phase: how to read embedded M4B chapter markers (an `ffprobe` shell-out versus a Rust MP4 crate).

## v0.0.52

Phase 6c-iii-d shipped: the sleep timer, the last piece of podcast parity. Set it to a duration or a boundary and playback pauses when you fall asleep; a fired duration timer keeps Castro's tap-to-extend trick alive. With this in, Phase 6c is complete and the absorbed Belfry podcast engine is at parity.

- **The timer (`conservatory-core/src/player/sleep.rs`, pure):** a `SleepClock` the engine ticks each loop turn, in three modes. A duration timer (15 / 30 / 45 / 60 minutes, or any value from the CLI) counts down only while actually playing, so a manual pause holds it rather than letting it expire silently; when it elapses, playback pauses. "End of item" pauses cued on the next item at the current item's end instead of advancing. "End of queue" lets the queue play out and disarms when it ends. The clock is transient per-session state, exactly like Castro / Overcast, so there is no database column and no migration.
- **Tap-to-extend (Belfry §3.6):** when a duration timer fires it opens a 30 second window; pressing play within it re-arms the same interval instead of merely resuming. Outside the window, play just resumes.
- **Available for any playing item, not episodes alone.** Falling asleep to an album is a real use case and the engine is media-agnostic, so the timer is offered for music tracks too (a small broadening of spec §3.6, which scoped it to episodes). The menu's boundary row adapts to what is playing: "End of track", "End of episode", or "End of book".
- **Where it lives:** a moon menu button on the Now-bar (the output-menu popover idiom), shown whenever something is loaded, with the remaining `M:SS` beside the icon for a duration timer and an accent tint while armed. The Now Playing drawer carries a "Sleep · …" line that mirrors the state and invites the tap-to-extend once a timer has fired. **`S`** pops the menu (a window-local shortcut, so the bare letter does not fire while the filter bar has focus). Headless, `conservatory-cli play <db> <root> --sleep <15|30|45|60|episode|queue>` arms it and the run exits when a duration timer elapses.
- **Tests:** the clock's unit tests (counts down only while playing, fires at zero, tap-to-extend re-arms inside the 30 second window and refuses outside it, the boundary modes carry no countdown); an engine null-output run where a duration timer fires and pauses mid-queue then tap-extends, and an "end of item" timer pauses at the first item's boundary without ever playing the second; the GUI label helpers (`fmt_sleep_remaining` rounds the clock up, `sleep_boundary_label` follows the kind, `sleep_drawer_text`). Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new dependency, no new migration.

Next: with podcast parity reached, **Belfry retires** (archive the repo, update the `~/.gitrepos` project map, spec §16.8), then Phase 7 (the Audiobooks tab) or Phase 8 (library audits).

## v0.0.51

The equalizer ships with built-in presets. Until now the only seeded preset was Flat, so the Sound dialog's preset dropdown was empty until you built your own. Migration 0010 stocks it with 16 starter curves that appear automatically in the dropdown and the `eq preset` CLI.

- **The set:** utility (Bass Boost, Bass Reducer, Treble Boost, Treble Reducer, Loudness, Vocal Boost), spoken-word (Spoken Word, Small Speakers, which matter for the podcast and audiobook tabs), and genre (Acoustic, Classical, Jazz, Rock, Pop, Electronic, Hip-Hop, Dance). Curves follow the classic iTunes/Winamp shapes, adapted to the peaking-band EQ and kept conservative (mostly within 6 dB) so octave-overlapping boosts do not stack into clipping. Your band layout (31 / 62 / 125 / 250 / 500 / 1k / 2k / 4k / 8k / 16k Hz) matches Apple's 10-band EQ, so the shapes map straight across.
- **Respectful seeding:** the built-ins are inserted with `INSERT OR IGNORE`, a one-shot at migration time. A preset you already saved under one of these names keeps its values, and a built-in you delete does not come back on the next launch. Only Flat stays protected from deletion, as before.
- **No code change:** the list / load / apply paths and the Sound dialog dropdown already existed; this is purely the seed migration. Tests: the built-ins load with the expected curve shapes and stay within a sane dB range; the seed-count assertions were updated. Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` build green.

## v0.0.50

Bugfix: podcast show notes were missing for feeds that leave `<description>` empty and put the real notes in `<content:encoded>` (Cortex, and others on the same setup). The parser preferred the RSS summary and only fell back to content when the summary was absent, but feed-rs reports an empty `<description/>` as `Some("")`, so the fallback never fired and a blank string was stored. On a live Cortex pull this left the 33 newest episodes with no notes.

- **Parse fallback (`conservatory-podcasts/src/parse.rs`):** an empty or whitespace-only summary is now treated as absent, so the notes fall through to `<content:encoded>`; a blank result either way collapses to `None` (the ingest sanitize then no-ops). All 180 Cortex episodes now carry notes.
- **Readability (`conservatory-podcasts/src/notes.rs`):** the ingest sanitize now also breaks headings and list items (`</h1>`–`</h6>`, `</li>`, `</ul>`, `</div>`, …) into newlines, not just `</p>` / `<br>`, so a heading no longer runs into the paragraph that follows it.
- **Tests:** an empty-`<description>`-with-`<content:encoded>` parse case and a heading/list-break sanitize case. Full podcasts suite + clippy `-D warnings` + fmt green. No new dependency, no migration, no schema change.

Note: chapters are unaffected. Cortex publishes no `podcast:chapters`, so its episodes have no chapter list (embedded ID3 chapters are a later fold-in).

## v0.0.49

Phase 6c-iii-c shipped: the Now Playing drawer is now a real episode surface. Open it on a podcast and you get a clickable chapter list that highlights the chapter you are in and follows the playhead, a Smart Speed line whose saved time ticks up as you listen, and show notes cleaned to readable text. This is the last of the surfacing work before the sleep timer (6c-iii-d) and Belfry's retirement.

- **Show notes cleaned at ingest (`conservatory-podcasts/src/notes.rs`):** feed descriptions are HTML; they are now run through `ammonia` to plain readable text when an episode is stored, so the triage pane, the Now Playing drawer, and the CLI all read clean notes with no per-render cost. `ammonia` does the load-bearing work (it drops `<script>` / `<style>` bodies and copes with malformed markup); a small pass turns paragraph breaks into newlines and decodes the structural entities. Existing episodes clean on their next refresh.
- **Smart Speed reaches the UI:** the player snapshot now carries `smart_speed_active` (the current item's profile flag) and `smart_speed_saved` (the live saved seconds from the open listening session). The drawer shows a "Smart Speed · saved m:ss" line for a smart-speed show, with the full figure in a tooltip; it is hidden for music and shows without it.
- **Chapter list (`now_playing_panel.rs`):** the drawer grows a "Chapters" list for a chaptered episode. Each row is the start time plus the title (or "Chapter N" when untitled); clicking a row seeks to it; the playing chapter is highlighted and re-highlights as the playhead crosses a boundary. A track or chapter-less episode hides the section. The current-chapter highlight and the Smart Speed line update from the same 250 ms poll that drives the Now-bar, so the per-tick cost is a class toggle, not a rebuild.
- **Tests:** the `sanitize_notes` cases (tag stripping, entity decoding, paragraph breaks, dropped `<script>` bodies, malformed HTML, blank-line collapse); an engine assertion that the snapshot reports Smart Speed active for an episode whose profile has it on. Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new migration; `ammonia` (workspace-approved, spec §11) activated in the podcasts crate.

Manual-launch check still owed (display-bound): open the drawer on a chaptered, smart-speed show and confirm the chapter list highlights and seeks, the saved time ticks up, and a track hides both.

Next: Phase 6c-iii-d (the sleep timer), then podcast parity is complete and Belfry retires.

## v0.0.48

Phase 6c-iii-b shipped: chapter navigation. Episodes with chapters can now be skipped chapter by chapter, from the Now-bar or the keyboard, and the engine swaps cleanly between a music track's filter chain and an episode's spoken-word profile mid-queue. The mechanism is built generic in the core player so the audiobook engine reuses it at Phase 7.

- **The navigation math (`conservatory-core/src/player/chapters.rs`, pure):** a lightweight `ChapterMark` (start time + title) and two helpers, `current_chapter_at` (which chapter the playhead is in) and `neighbour_chapter` (the absolute time to seek to for a next / previous skip). Forward stops at the last chapter (a no-op past the end); back restarts the current chapter when more than three seconds in, else steps to the previous one, clamped at the first. Unit-tested for every case.
- **The item carries its chapters:** `PlayableItem` gained a `chapters` field, resolved at queue-build time (an episode's stored chapters, an audiobook's at Phase 7), so the engine navigates them without ever reading the database. The GUI attaches them after building a queue (`attach_episode_chapters`); a music-only queue simply has none.
- **Engine + transport:** a new `SkipChapter` command seeks to the neighbouring mark (and, like a user seek, is excluded from the Smart Speed time-saved accounting). The player snapshot now reports the current chapter and the chapter count. The Now-bar grows previous / next-chapter buttons that appear only for a chaptered item, and **`Ctrl+Shift+→` / `Ctrl+Shift+←`** skip chapters from anywhere.
- **CLI:** the `play` verb now attaches an episode's chapters to its queue item, so chapter-skip works headless too.
- **Tests:** the `chapters.rs` helpers; an engine run that skips forward to a chapter boundary and back to the start (paused, so the fixture cannot end under it); and the roadmap-named filter-graph swap, a queue interleaving a music track and a podcast episode that plays both to completion (proving the `af`-chain profile switch at the kind boundary, spec §16.9). Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new dependency, no new migration.

Manual-launch check still owed (display-bound): confirm the Now-bar chapter buttons appear for a chaptered episode and the keybinding jumps chapters, and that a chapter-less item hides them.

Next: Phase 6c-iii-c/d (the Now Playing episode surface, then the sleep timer), the last of podcast parity before Belfry retires.

## v0.0.47

Phase 6c-iii-a shipped: podcast chapters are now fetched and stored. A feed's
`podcast:chapters` URL (parsed since 6a-ii, but dropped) is fetched on first sight
of an episode, parsed, and written to the `chapters` table, so chaptered shows
arrive with their chapter set. Headless; navigation and the Now Playing chapter
surface are 6c-iii-b and -c.

- **Chapter fetch + parse (`conservatory-podcasts/src/chapters.rs`):** `parse_chapters_json`
  (pure) reads the Podcast Index chapters JSON (`{ "chapters": [ {startTime, title,
  endTime, url, img}, … ] }`) into core `Chapter` rows; `fetch_chapters` is the thin
  GET wrapper, sharing the refresh fetcher's connection pool. Tolerant: a chapter-less
  or `{}` document is an empty set; blank optional strings become `None`; the chapter
  image URL is kept verbatim (downloading it is deferred).
- **Wired into refresh (`refresh.rs::apply_feed`):** when a genuinely-new episode
  carries a chapters URL, its chapters are fetched and stored via the existing
  `replace_chapters` worker command. Best-effort: a fetch or parse failure is logged
  and never fails the refresh, and only new episodes fetch (a re-refresh does not
  re-hit every URL). The storage plumbing (table, write, `list_chapters` read) already
  existed; only the fetch + parse + wiring is new.
- **CLI:** `podcast chapters <db> <episode_id>` lists an episode's stored chapters
  (index, start time, title), read-only.
- **Tests:** the JSON parser (a full document, empty / missing-array, blank strings,
  malformed); a wiremock refresh that serves a feed plus its chapters JSON and asserts
  the chapters land. `serde` / `serde_json` activated in the podcasts crate (both
  already workspace dependencies). Full suite + clippy `-D warnings` + fmt + the
  `--no-default-features` music-only build green. No new migration.

Next: Phase 6c-iii-b (chapter navigation: the shared skip-to-next/prev engine
mechanism the audiobook engine reuses at 7c).

## v0.0.46

Phase 6c-ii shipped: time-saved accounting. The player now records an append-only listening session for every episode it plays, with the wall-clock time Smart Speed saved, and a CLI surfaces the running totals. Headless; the Now Playing readout is part of the 6c follow-on.

- **The accounting math (`conservatory-core/src/player/session.rs`, pure):** a `SessionAccumulator` samples real (wall-clock) vs audio time each engine tick and reports `smart_speed_saved = max(0, audio_seconds / speed − real_seconds)`. The subtle part is `silenceremove`'s non-linear timeline: when it drops dead air the playhead leaps forward, and those jumps are counted as covered audio (the listener got that audio for free), which is exactly what produces a positive saved figure. Variable speed alone nets to zero (at 2× the playhead advances twice as fast, so the two terms cancel); a user seek is excluded (it is a jump, not audio played); a pause accrues nothing. All five cases are unit-tested with synthetic tick sequences.
- **Engine wiring (`player/engine.rs`):** a session starts when an episode loads, samples once per ~10 Hz loop (resyncing rather than accruing while paused or ended, so idle time never inflates it), excludes user seeks (including the launch-resume jump to the saved offset), and closes by appending one row at every episode boundary: item change, end-of-file, stop, queue clear/replace, the removal of the playing item, and a mid-episode shutdown. The write is blocking, like the resume cursor, so the ledger row is guaranteed to land.
- **Storage:** the `listening_sessions` table (seeded back in migration `0006`, never written until now) gets its insert (`insert_listening_session`, append-only) and an aggregate read (`listening_totals`: session count + summed real / audio / saved seconds). No new migration, no new dependency.
- **CLI:** `podcast stats <db>` prints the totals, formatted as `H:MM:SS`: sessions, time listened, audio covered, and the wall-clock Smart Speed saved.
- **Tests:** the `session.rs` accumulator math (no-silence ≈ 0, a silence jump saves, variable-speed nets zero, a seek is excluded, a pause accrues nothing, a non-positive speed cannot divide by zero); a `listening_sessions` append round-trip through the worker (empty totals are zero, three appended sessions sum); an engine null-host run that plays an episode to EOF and lands exactly one session row with sane non-negative totals. Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

Next: Phase 6c-iii+ (chapters + the Now Playing episode additions), the last of podcast parity before Belfry retires.

## v0.0.45

Phase 6c-i shipped: Smart Speed and Voice Boost are real. The per-show toggles (saved since v0.0.37 but inert) now drive spoken-word stages on the shared 5.5 `af`-chain, so a show's episodes play through silence-trimming and voice-leveling. Headless; the time-saved accounting is 6c-ii.

- **Spoken-word stages (`conservatory-core/src/player/spoken.rs`, pure, sibling to `dsp.rs`):** Smart Speed is `@ss:lavfi=[silenceremove=…]` (mid-stream dead-air removal, `stop_periods=-1`, tuned to leave a natural beat); Voice Boost is a fixed three-stage preset, `@vbcomp` (a gentle `acompressor` with make-up gain) → `@vbeq` (low-cut + presence lift) → `@vbnorm` (live `dynaudnorm`, a tighter window than the music leveler). Both are presets on the existing chain, not a parallel path; `build_af_chain` appends them after the music stages, Smart Speed before Voice Boost so the compressor does not raise the noise floor before silence detection.
- **The profile carries the flags:** `MusicProfile` gained `smart_speed` / `voice_boost`. `resolve_episode_profile` reads them from the show's settings; `resolve_music_profile` leaves them false, so the music chain is byte-for-byte unchanged (the no-regression guard). A show with no saved settings resolves both to false (the feature applies to shows you have configured; the settings dialog defaults Smart Speed on, so saving opts in).
- **CLI:** `podcast debug-chain <db> <episode_id>` prints an episode's resolved `af` chain (its spoken-word profile composed with the persisted EQ + DSP), so the `@ss` / `@vb*` stages are inspectable headless.
- **GUI:** the per-show settings dialog drops its "audio processing arrives later" caption; Smart Speed / Voice Boost now describe what they do.
- **Tests:** the `spoken.rs` builders (on → the expected `silenceremove` / `acompressor` / `dynaudnorm` strings, off → none); `build_af_chain` appends the spoken stages for an episode profile and emits the unchanged chain for music; the libmpv `ao=null` EOF run now sets both flags, proving mpv accepts the `silenceremove` + Voice Boost syntax and still decodes to end. Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new dependency, no new migration.

Next: Phase 6c-ii (time-saved accounting via append-only `listening_sessions`).

## v0.0.44

Phase 5.5c-ii-b shipped, completing Phase 5.5c-ii and the music audio engine: the "Sound" preferences dialog (the equalizer's home since 5.5b) now holds the whole chain, and the persisted config finally drives playback. The music daily-driver feels complete; podcasts (Phase 6) are next.

- **The consolidated "Sound" page (`conservatory/src/ui/window.rs`, `ui/sound.rs`):** three new groups join the equalizer. **ReplayGain** (mode / preamp / clip-guard); **Dynamics** (the compressor / limiter / leveler, each an `adw::ExpanderRow` whose enable switch toggles the module and whose child rows tune it, the app's first use of `ExpanderRow`); **Output** (backend / device / resampler / gapless `ComboRow`s + a switch). DSP and output changes drive the engine live (DSP is a structural `af` rebuild, gap-acceptable; the backend reloads via `ao-reload`); ReplayGain / gapless changes are resolved per-queue, so they take effect on the next built queue. The whole `audio_state` persists on dialog close (the EQ slider precedent).
- **The output device picker lives in two places now:** the header `MenuButton` (Phase 4c-ii, the quick-switch) and a write-through `ComboRow` in the Sound page's Output group, both populated from the engine's queried device list and both sending `set_audio_device`.
- **Persisted config drives playback (`apply_persisted_audio`):** at startup the GUI pushes the stored DSP modules + output backend + resampler into the engine (which also fixes that 5.5c-i's DSP was stored but never applied in the GUI), mirroring `apply_persisted_eq`. The queue builders (`build_play_queue` / `build_mixed_queue`) now read the persisted playback defaults via `PlaybackConfig::from_audio_state` instead of `PlaybackConfig::default()`, so a saved ReplayGain mode / preamp / clip / gapless choice shapes the next queue.
- **Deferred and recorded (unchanged from the spec, not built):** exclusive/bit-perfect output (ALSA `hw:` + `--audio-exclusive`), LADSPA / raw-`af` hosting, native `crossfeed`, the parametric `anequalizer`, peak-aware ReplayGain attenuation.
- **Tests:** the pure picker-mapping helpers in `ui/sound.rs` (`option_index` / `option_value` / `option_labels` for the ReplayGain-mode, backend, and resampler tables, with the forgiving fallback and round-trip). The dialog widgets are verified by build + manual launch (the 3b/3c/5.5b-ii precedent). Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new dependency, no new migration.

Next: Phase 6 (podcasts) continues; the shared `af`-chain engine is now ready for the Phase 6c spoken-word presets (Smart Speed / Voice Boost).

## v0.0.43

Phase 5.5c-ii-a shipped: the output backend and resampler are finally applied to the player, with a CLI to drive them (headless). Migration `0009` seeded `output_backend` and `resampler_quality` back at 5.5c-i but nothing consumed them; this closes that gap. The consolidated GTK "Sound" page is the next step (5.5c-ii-b).

- **Output backend (`conservatory-core/src/player/host.rs`):** `MpvHost::set_output_backend` sets mpv's `ao` driver and reloads the output (`ao-reload`) so an in-session switch takes effect without waiting for the next track (gap-acceptable, the `set_dsp` structural-rebuild precedent). `auto` maps to an empty `ao` (mpv's own driver autoprobe); a named backend (`pipewire` / `pulse` / `alsa` / `jack`) pins the driver. This is distinct from the device picker (`audio-device`, Phase 4c-ii): backend is the driver, device is the sink.
- **Resampler quality:** `MpvHost::set_resampler` raises the `audio-resample-*` knobs (`filter-size`, `cutoff`) for `High` and restores mpv's defaults for `Default`, re-asserted on each load. Avoid-resample stays the default either way: `audio-samplerate` / `audio-format` are left unset, so a same-rate file is never resampled.
- **Engine:** `SetOutputBackend` / `SetResamplerQuality` player commands + the matching `PlayerHandle` methods (the `SetDsp` shape). `PlaybackConfig::from_audio_state` maps the persisted playback defaults (ReplayGain mode / preamp / clip, gapless) into the profile resolver; it lives in `player/profile.rs` so the db layer stays free of the player's `ReplayGain` enum (the one place the stored `replaygain_mode` string becomes the enum, forgiving on an unknown value).
- **CLI `output` verb:** `output show` (the active backend + resampler), `output backend <auto|pipewire|pulse|alsa|jack>`, and `output resampler <default|high>` (read `get_audio_state`, write through the worker, the `dsp` verb precedent). `debug-dsp` now prints the resolved backend + resampler line.
- **Tests:** the host null-AO integration sets the backend to `null` (exercising the `ao` + `ao-reload` path hermetically, since a real driver might fail to init in CI) and toggles the resampler, and the EOF smoke run now re-asserts a `High` resampler through `load`; the `PlaybackConfig::from_audio_state` mapping (each mode plus the forgiving fallback); the CLI verbs verified end-to-end against a fixture DB. Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new dependency, no new migration.

Next: Phase 5.5c-ii-b (the consolidated GTK "Sound" preferences page).

## v0.0.42

Phase 5.5c-i shipped: the DSP modules, in the chain and the CLI (headless). The compressor, brick-wall limiter, and volume leveler join the `af` chain as toggleable stages; their settings persist; the output backend / resampler control and the consolidated GTK "Sound" page are the next step (5.5c-ii).

- **DSP stages (`conservatory-core/src/player/dsp.rs`):** `build_af_chain` now also renders the dynamics modules after the EQ, in signal-flow order: `@comp` (`acompressor`), `@limit` (`alimiter`), `@boost` (`dynaudnorm`). Each contributes a stage only when its module is enabled (all off is the no-op chain). The limiter runs `level=disabled`, so it is a transparent peak catcher rather than a normalizer, which makes it a safe ReplayGain clip net (a positive net gain can never clip with the limiter on). User-facing dB knobs (the compressor threshold, the limiter ceiling) are converted to the ffmpeg filters' linear forms in the one place the stage is built. `dynaudnorm` is used for live leveling rather than `loudnorm`, whose accurate mode is two-pass / offline.
- **Modules keep their settings while off.** Each module is an `enabled` flag plus its parameters, stored independently, so toggling a tuned compressor off and back on restores it rather than resetting to defaults.
- **Persistence (migration `0009`, the `eq_state` precedent):** a singleton `audio_state` row holds the whole audio config: the playback defaults (ReplayGain mode / preamp / clip, gapless), the three DSP modules, and the output backend / resampler. The `AudioState` model round-trips through the single-writer worker; the read is forgiving (a bad stored value degrades to the default). The playback and output halves ship here but are consumed at 5.5c-ii, so that sub-phase needs no second migration.
- **Engine:** a new `SetDsp` player command. The host holds the active DSP modules and applies them into the chain on the next load (a settings change rebuilds the chain structurally, which is fine for an explicit change; DSP has no per-slider live path like the EQ).
- **CLI `dsp` verb:** `dsp show` (each module's state and the resolved `@comp` / `@limit` / `@boost` chain), `dsp comp on|off [--threshold --ratio --attack --release]`, `dsp limiter on|off [--ceiling]`, and `dsp leveler on|off [--target --gausssize]`. `debug-dsp` now prints the DSP breakdown, and `play` applies the persisted modules.
- **Tests:** the stage builders (off → no stage; on → the expected lavfi string with the correct dB-to-linear conversion); the full chain order (`@rg` → `@eq` → `@comp` → `@limit` → `@boost`); the `audio_state` round-trip and the params-survive-an-off-toggle guard through the worker; the migration table-exists; and the libmpv EOF run now sets a real `@comp` / `@limit` / `@boost` chain, proving the `acompressor` / `alimiter` / `dynaudnorm` mpv syntax decodes. Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new dependency (the filters ride the linked libmpv/ffmpeg); one schema migration.

Next: Phase 5.5c-ii (the output backend / resampler control and the consolidated GTK "Sound" preferences page).

## v0.0.41

Phase 5.5b-ii shipped: the equalizer is now interactive. Open the new Sound preferences (the speaker-card button in the header, or Ctrl+,), drag a band, and hear it change live; presets are a click away. This completes Phase 5.5b.

- **Live, gap-free per-band changes (`conservatory-core`):** dragging a band sends mpv an `af-command` to the named `equalizer@b<n>` filter (ffmpeg's `equalizer` supports a runtime `gain` command), so the change is instant with no click or gap in the audio. A structural chain rebuild is reserved for the moments that need it: switching presets, or crossing the flat↔non-flat boundary where the `@eq` stage appears or disappears. The host now remembers the playing item's profile so it can rebuild mid-track. New engine command `SetEqBand` + `PlayerHandle::set_eq_band`; the `af-command` mapping is a pure, unit-tested helper.
- **The app's first preferences dialog (`conservatory/src/ui/sound.rs`):** a "Sound" page in an `adw::PreferencesDialog` (Phase 10's config work builds on this surface). An Equalizer group of 10 vertical sliders (−12 to +12 dB, with a detent at 0) under their ISO centre-frequency labels, plus a preset dropdown (your saved presets and a "Custom" marker) and Save as… / Delete / Reset. Dragging a slider applies live and marks the EQ "Custom"; choosing a preset or Reset moves the sliders and the sound together. Edits persist when you close the dialog; preset actions persist immediately.
- **The persisted EQ is now active from launch.** The GUI never pushed the stored EQ to the engine at startup; it does now, so your equalizer applies from the first track, not only after you open the dialog.
- **Opens via** the header speaker-card button or **Ctrl+,**.
- **Tests:** the `eq_band_command` mapping (a band change maps to `af-command @eq gain <dB> b<n>`, no chain rebuild); a null-output engine run that mutates bands live mid-playback and still reaches end-of-file (the real mpv `af-command` path); the `match_preset` projection. Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new dependency; no schema change.

Next: Phase 5.5c (the DSP modules — compressor, brick-wall limiter, `dynaudnorm` leveler — plus output backend / resampler control, and peak-aware ReplayGain clip-prevention), then Phase 6c (the spoken-word Smart Speed / Voice Boost presets on this engine).

## v0.0.40

Phase 5.5b-i shipped: the graphic equalizer, in the chain and the CLI (headless). A 10-band ISO-octave EQ now joins the `af` chain as the `@eq` stage; presets persist; the live sliders and the GTK "Sound" dialog are the next step (5.5b-ii).

- **`@eq` chain stage (`conservatory-core/src/player/chain.rs`):** `build_af_chain` now also renders the equalizer: `@eq:lavfi=[equalizer@b0=f=31:t=o:w=1:g=…, … equalizer@b9=f=16000:…]`, a stack of named `equalizer` peaking bands at the ISO octave centres (31 / 62 / 125 / 250 / 500 / 1k / 2k / 4k / 8k / 16k Hz). A flat EQ contributes **no** stage (the no-op chain). Each band is named `equalizer@b<n>` so the next sub-phase can mutate it live via `af-command` without rebuilding the graph. The libmpv EOF test confirms the syntax decodes.
- **Persistence (migration `0008`, the `perspectives` precedent):** `eq_presets` (named, seeded with `Flat`) and the singleton `eq_state` (the live band values + the selected preset). The `EqState` model carries `bands: [f64; 10]` + the preset name; reads and worker writes round-trip through the single-writer worker. CSV-backed, forgiving (a bad stored row reads as flat rather than breaking playback).
- **Engine:** a new `SetEq` player command. The host holds the active EQ and applies it into the chain on the next loaded track. (Instant, gap-free per-band changes are 5.5b-ii.)
- **CLI `eq` verb:** `eq show` (each band's centre + gain, the active preset, and the resolved `@eq` chain), `eq set <band 0-9> <gain dB>` (clamped to ±24, marks the EQ a custom edit), and `eq preset list | save <name> | load <name> | delete <name>` (`Flat` is undeletable). `play` applies the persisted EQ.
- **Tests:** the `@eq` builder (flat → no stage; a non-flat EQ → the named bands at the ISO centres; `@rg` precedes `@eq`); the EqState + preset round-trips and the forgiving CSV parse; the migration table-exists; the libmpv EOF run now sets a real `@eq` chain. Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new dependency (the EQ rides the linked libmpv/ffmpeg `equalizer` filter); one schema migration.

Next: Phase 5.5b-ii (live `af-command` slider mutation + the app's first GTK "Sound" preferences dialog).

## v0.0.39

Phase 5.5a shipped: the chain foundation and correct head-staged ReplayGain. This begins Phase 5.5 (the music DSP engine) and fixes a real ReplayGain bug, headless/core. It is the substrate the Phase 6c spoken-word chain (Smart Speed / Voice Boost) is built on.

- **Labelled `af`-chain builder (`conservatory-core/src/player/chain.rs`):** the flat profile-to-properties application becomes a real mpv `af` filter chain, built once per item with labelled stages. 5.5a emits the `@rg` ReplayGain head stage; the `@eq` / `@comp` / `@boost` slots (equalizer, compressor/limiter, leveler) join in 5.5b/c. `build_af_chain` is pure and unit-tested.
- **ReplayGain moves to the chain head and is recomputed per track, fixing mpv #8267.** ReplayGain is now an explicit `@rg:lavfi=[volume=<dB>]` at the *head* of the chain, computed from the DB's `replaygain_track` / `_album` (read from tags at import or written by the rsgain scan). mpv's built-in `--replaygain` is dropped: it sat *after* the `af` chain (a boosting EQ would defeat clip-prevention) and was not re-applied per track across a gapless boundary, so a whole queue inherited the first track's gain (mpv bug #8267). Because the host rebuilds the chain from each item's profile on load, every track gets its own gain.
- **Preamp + clip-prevention.** A `replaygain_preamp` offset, and a clip guard: with no peak data stored, the safe default (`replaygain_clip`) clamps the net gain to attenuate-only (≤ 0 dB), which can never push a sample over full scale. The real brick-wall limiter and peak-aware attenuation arrive in 5.5c.
- **Gapless: `--gapless-audio=weak`** when gapless (preserves the source rate across a mixed-rate library), `no` for single items.
- **Crossfade removed.** The unused `crossfade_seconds` field and config key are gone (crossfade is impossible in a single libmpv instance and maintainer-rejected; the codebase never set it non-zero). Gapless-only, the path real mpv-based players take.
- **CLI `debug-dsp <db> [track_id]`:** prints the resolved `af` chain plus the ReplayGain breakdown (mode, raw track/album gains, preamp, clip, net dB), gapless, and speed. The every-surface-CLI-testable rule; verified against the real `testdata/` albums.
- **Tests:** the chain builder (the `@rg` string; empty when off; **different gains produce different chains**, the per-track #8267 guard); profile resolution (mode downgrade, preamp, the clip clamp, off → none, episode → none); and the libmpv EOF test now plays a fixture with a real `@rg` chain set, proving the `af` syntax is valid and does not break decode. Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green. No new dependency (every filter rides the already-linked libmpv/ffmpeg); no schema change.

Also corrected a stale phasing claim: spec §17 / the roadmap said Phase 5.5 "lands before podcasts," but 6a/6b (the podcast manager and triage) are independent of the audio engine and shipped first. The wording now says 5.5 lands before the **Phase 6c spoken-word chain** specifically, which is the piece that actually depends on it.

Next: Phase 5.5b (the graphic + parametric equalizer, the first GTK "Sound" preferences surface, and live `af-command` parameter mutation).

## v0.0.38

Playback feedback, diagnostics, and a Now Playing drawer. The player no longer runs silently: it tells you what it is doing, the two bugs from hands-on use are fixed, you can turn on logs, and clicking the Now-bar opens a details panel.

**Bug fixes:**
- **The Now-bar no longer shows a stale song while a podcast plays.** The engine snapshot now carries the current item's media kind, so the Now-bar reads the right metadata: a playing episode shows its title and show (and the show's art, or the placeholder, never the previous track's cover). The root cause was a kind-blind snapshot that always did a track-only lookup, which returned nothing for an episode and left the old cover up. A new `episode_metadata` read resolves an episode to the same shape as a track.
- **A podcast episode now starts on the first press, no pause-then-play nudge.** `engine.load_current` now syncs mpv's pause state to "playing" after loading. mpv inherits the prior pause state across a `loadfile`, so an item loaded after a paused one (notably the launch-resume queue, which loads paused) came up paused while the UI thought it was playing. A regression test guards it.

**Feedback the player was missing:**
- **Buffering indicator.** A streamed episode can take many seconds to start while it buffers; the Now-bar now shows a spinner during that wait (mpv `core-idle`), so the silence is explained.
- **Streaming vs downloaded.** The Now-bar shows a streaming glyph for an undownloaded episode, and the Podcasts episode list gained a column marking each episode downloaded vs stream-only. ("Downloading" is not shown yet; there is no GUI-triggered download.)
- **Podcast double-click plays just that episode.** Double-clicking an episode used to dump the entire visible list into the queue (the album idiom, wrong for a 180-episode feed). It now plays only the episode you clicked; the queue is still built deliberately via triage or `Ctrl+Enter`.

**Diagnostics:**
- **Logging.** Both binaries install a tracing subscriber, so the events wired through the worker, player engine, and podcast fetch/refresh actually surface. The GUI defaults to `info` and takes a `--debug` flag (raises our crates to `debug`: the player load / advance / buffering transitions); the CLI honours `RUST_LOG`. Without this, all of it was a silent no-op, which is why the app gave no output. New player-transition log lines make "what is it doing" answerable.

**New surface:**
- **Now Playing drawer.** Click the Now-bar cover/title (or press `Ctrl+I`) and a panel slides up from the bottom, the horizontal twin of the right-side queue drawer, showing the current item's full metadata: for a track, format / bitrate / sample rate / ReplayGain / path / rating / plays / album / year; for an episode, show / date / runtime / size / stream-or-local / notes. It updates as the queue advances. This is the lighter realization of the spec's Phase 11c Now Playing surface, and its content area is the intended home for the future spectrum visualizer.

**Tests:** the pause-sync regression (a fresh queue plays after a pause, via a null-output engine run); `episode_metadata` resolves the show title + cover and a missing episode reads as `None`; the Now Playing field projections for a track and an episode (pure, headless); the `streaming` flag through the queue builders. Full suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

## v0.0.37

Phase 6b-ii-c-3-c shipped: per-show podcast settings in the GUI. Select a show in the Podcasts tab, click the gear in the detail pane, and set its playback speed, Smart Speed and Voice Boost, intro/outro skip, and what happens to new episodes (inbox, queue, or archive). This completes the per-show overrides and Phase 6b-ii-c.

- **Settings gear (`conservatory/src/ui/podcasts.rs`):** when a show is the selected sidebar source, a gear button appears in the detail pane (hidden for the triage buckets and tags, where a single show does not apply). The detail header now shows the show's title when no episode is selected.
- **Settings dialog:** the gear opens an `adw::AlertDialog` whose content is an `adw::PreferencesGroup` of rows, pre-populated from the show's stored overrides (or the schema defaults when it has none): a speed spin row (0.25x–4.0x, the same bounds the player clamps to), Smart Speed and Voice Boost switches, intro/outro skip spin rows, and a "New episodes" dropdown (Add to Inbox / Add to Queue / Archive, the inbox policy that drives v0.0.36's routing). Save writes through the single-writer worker (`upsert_show_settings`), the same path the `podcast settings` CLI uses. The dialog reuses the bulk-edit dialog idiom; the existing episode-detail and triage flows are untouched.
- **Honest about 6c:** Smart Speed and Voice Boost are saved per show, but their audio processing (the `af`-chain filters) lands at Phase 6c; the dialog says so, so the toggles persist intent without overstating what plays today.
- **No clobber:** the panel does not expose the global-inherit `skip_forward` / `skip_back` fields, so a save preserves whatever those were rather than resetting them.
- **Tests:** the pure form mapping is unit-tested headless (the inbox-policy index round-trip and out-of-range degrade-to-inbox; `settings_from_form` applies the edited fields and preserves the skip-forward/back inherits; the default skeleton matches the schema). The dialog widget tree is build + manual (the 3b/3c GUI precedent). First use of the libadwaita preference-row widgets (`SpinRow` / `SwitchRow` / `ComboRow`); no new dependency. The `--no-default-features` music-only build stays green.

With this, **Phase 6b-ii-c is complete** (episode playback, resume, and per-show overrides), and so is **Phase 6b-ii** (the Podcasts triage panes). What remains for podcast parity is Phase 6c (the Smart Speed / Voice Boost `af`-chain, built on the Phase 5.5 engine, plus chapters and the now-playing additions).

Next: Phase 5.5 (the DSP `af`-chain engine that 6c builds on) or Phase 6c (the spoken-word profile), per the spec §17 ordering.

## v0.0.36

Phase 6b-ii-c-3-b shipped: inbox-policy routing and retention, the management half of per-show overrides. A show set to auto-queue drops its new episodes straight into the queue; one set to auto-archive keeps them out of your inbox; and a show with a keep limit prunes its oldest downloads so they do not pile up on disk.

- **Inbox-policy routing (`conservatory-podcasts`, on refresh):** `refresh::apply_feed` now reads each show's `inbox_policy` once (the schema default, Inbox, when a show has no stored settings) and routes every **genuinely-new** episode through it: `AlwaysQueue` enqueues it into the unified queue, `AlwaysArchive` marks it `ArchivedUnlistened`, `Inbox` does nothing (the §4.2 derivation already puts a row-less, un-queued episode in the Inbox bucket). Only new episodes route, so a re-refresh never re-queues an episode you have since removed from the queue or un-archives one you archived by hand.
- **Retention (`conservatory-podcasts/src/retention.rs`):** prune downloaded episodes beyond a show's `keep_count` (0 = keep all). The oldest downloads lose their audio file and their `audio_path`, reverting to stream-only; the row, triage state, and resume position survive, only the bytes go. It is a separate **root-aware** pass (it deletes files under the library root), split `plan` → `apply` in the mover's dry-run-then-apply shape, and only ever touches files you actually downloaded (`auto_download` is off by default). The empty episode dir is removed best-effort.
- **Core:** a new `clear_episode_audio_path` worker command (the counterpart to `set_episode_audio_path`, which can only set) reverts an episode to stream-only after its file is pruned.
- **CLI:** `podcast prune <db> [show_id] --root <root> [--apply]` (one show or all). Dry-run by default: it lists the downloads it would delete; `--apply` does the deletion. Routing needs no new verb (it rides `podcast refresh`).
- **Tests:** a new episode routes per each of the three policies and an already-seen one does not re-route (`refresh.rs`); retention prunes the oldest downloads, keeps the newest, is a no-op at `keep_count = 0`, and never counts stream-only episodes toward the cap (`retention.rs`). The `--no-default-features` music-only build stays green.

This is the second of the per-show overrides (c-3 split a/b/c): **a** was speed, **b** is this (inbox routing + retention), **c** is the GUI per-show settings panel. The Smart Speed / Voice Boost flags are stored but their filters remain Phase 6c.

Next: Phase 6b-ii-c-3-c (the GUI per-show settings panel), which surfaces speed, the Smart Speed / Voice Boost toggles, skip intro/outro, and the inbox policy in the Podcasts detail pane.

## v0.0.35

Phase 6b-ii-c-3-a shipped: per-show podcast playback speed. Set a show to play at 1.5x and its episodes play at that rate, with pitch held constant so faster speech still sounds natural, in the one unified queue alongside music.

- **Profile carries speed (`conservatory-core`):** the per-item playback profile gains `speed` + `pitch_correction`. `resolve_episode_profile` now reads the show's `playback_speed` (clamped to a sane `[0.25, 4.0]`, pitch correction on); music resolves to native speed. `MpvHost::load` applies mpv's `speed` + `audio-pitch-correction` (the built-in scaletempo2) before loading, so 1.0 / off is a no-op for tracks (the music path is unchanged).
- **Threaded through the queue builders:** each episode's show settings reach the profile at enqueue. `EpisodeSource` / `MixedQueueRow` / `QueueDisplayRow` carry `show_id`; a new core `show_settings_map` batch-reads the per-show overrides; `build_episode_queue`, `build_mixed_queue` (resume), and the CLI `resolve_queue_items` resolve each episode's speed from them.
- **CLI:** `podcast settings <db> <show_id> [--speed N]` views a show's settings, or sets the playback speed (preserving the other fields). The headless surface for the feature; the in-detail-pane GUI editor is c-3-c.
- **Tests:** profile speed resolution + clamp; the queue builders apply per-show speed (and default to 1.0 without settings); a host integration test asserts `load` leaves mpv's `speed` at the profile value; the `--no-default-features` music-only build stays green.
- This is the first of the per-show overrides (Phase 6b-ii-c-3 split into a/b/c): **a** is speed (this); **b** is inbox-policy routing + retention pruning on refresh; **c** is the GUI settings panel. The `smart_speed` / `voice_boost` flags are stored and ride the settings, but their filters are Phase 6c (the Smart Speed / Voice Boost `af` chain).

Next: Phase 6b-ii-c-3-b (inbox-policy routing + retention) or c-3-c (the GUI per-show settings panel).

## v0.0.34

Phase 6b-ii-c-2 shipped: podcast episodes now resume. Close the app mid-episode and reopen it, and you pick up where you left off, to the second, downloaded or streamed, in the one unified queue alongside music. An episode's progress and played state persist on their own, so finishing or part-listening to an episode is reflected in the triage list across restarts.

- **Per-kind transport cursor (migration `0007`):** the singleton `playback_state` cursor gains `kind` + `episode_id`, so it records *what kind* of item was last playing. A restart reopens an episode (not just the last track) and seeks it to the saved position. The column adds are additive (`kind` defaults to `'track'`, the episode FK defaults NULL), so existing libraries migrate with no rewrite. `docs/schema.md` updated.
- **Episode persistence (`conservatory-core`):** two partial-upsert worker writes mirror the triage actions: `set_episode_position` (records the resume position + marks InProgress, preserving starred / play_count) on a playback tick, and `complete_episode` (marks PlayedFully, bumps play_count, rewinds position) on a natural end-of-file. The per-episode state lives in the podcast `playback` table, so it survives the queue moving on to other items, distinct from the singleton cursor.
- **Engine per-kind dispatch (`player/engine.rs`):** the three persistence sites (tick flush, end-of-file, terminal end-of-queue) now branch on `MediaKind`. A track writes the music cursor + `tracks.play_count` as before; an episode writes the podcast `playback` row + the episode-kinded cursor, and never an id into the music tables. The episode position write is synchronous and guarded on `!ended`, so it can never reorder past or clobber the terminal completion (both touch `playback.played`).
- **Mixed-queue resume (CLI + GUI):** the saved queue is no longer track-only on resume. `conservatory-cli play` (`resolve_queue_items`) and the GUI `resume_saved_queue` rebuild an interleaved track + episode queue (`build_mixed_queue`) and resume at the cursor's `(kind, id)` + position; `load_queue_display` now carries each episode's audio source so the GUI rebuilds without extra reads.
- **Refactor:** the cursor write is bundled into a `PlaybackCursor` struct (the codebase's struct-passing idiom) rather than a seven-argument call.
- **Tests:** episode persistence + cursor round-trip through the worker; an episode plays to EOF writing the podcast `playback` row (PlayedFully + play_count) while leaving the music cursor and a colliding track untouched (the per-kind regression guard, strengthened from the c-1 guard test); `build_mixed_queue` interleave / source-skip / cursor re-index; the `--no-default-features` music-only build stays green.

Next: Phase 6b-ii-c-3 (per-show overrides: speed, Smart Speed, Voice Boost, skip, retention, inbox policy).

## v0.0.33

A cleanup pass over the podcast subsystem: a real correctness fix, a robustness fix, and doc/comment tidying. No new features.

- **Fix (the episode-id leak):** the per-kind playback persistence guard from 6b-ii-c-1 was **incomplete**. The terminal cursor write in the engine's `advance_after_end` (reached when the last queue item ends naturally) was unguarded, so finishing an episode-tailed queue wrote the episode id into the music `playback_state.track_id`. Because that column has an FK to `tracks(id)`, the write was silently dropped when no track shared the id, but in a populated library where an episode id collides with a track id, it persisted and would **resume the wrong music track** on the next launch. The terminal write now carries the same `kind == Track` guard as the tick path. The engine null-sink test was strengthened to import a real track first (so the FK is satisfiable), which makes it actually exercise the leak path rather than masking it with an empty table.
- **Fix (429 cooldown):** a `429 Too Many Requests` with a missing or non-numeric `Retry-After` (the HTTP-date form) recorded no cooldown, so the next refresh cycle immediately re-hit the throttling host. It now falls back to a default backoff, so a throttling host is always cooled down.
- **Default change: `auto_download` is now off.** A new subscription no longer flags itself for auto-download (the schema default and the `add` / `import-opml` paths now set it `false`). On a large subscription list, auto-downloading every episode fills the disk fast; downloads stay user-chosen (the `podcast download` verb, spec §5.3). It is a stored per-show flag with no auto-download loop wired yet, so this only sets the future default; existing subscriptions keep their value.
- **Docs / comments:** the Podcasts GUI module doc now describes the playback path (it still said playback was unshipped); the unified-queue write block comment and the `enqueue_episodes` / `replace_queue_with_episodes` docs were corrected (a doc comment had been misattached); the `episodes_in_bucket` doc now states that Queue and Played can overlap by design (only Inbox is exclusive); and a misleading "char boundary" note in `slugify` was reworded (the slug is ASCII-only).
- Verified the OPML round-trip on a real 72-feed Overcast export (import + export preserve every subscription and `applePodcastsID`) and a single-feed refresh (215 episodes).

Known follow-ups (flagged, not done here): a few small refactors (a `Show::skeleton` constructor to dedupe the ~18-field skeleton built in three places; a shared `element_name` helper in `namespace.rs`; an `EpisodeRow → EpisodeSource` helper), and URL-decoding the download filename (kept percent-encoded for now, which is path-safe).

## v0.0.32

Phase 6b-ii-c-1 shipped: podcast episodes now play. Double-click an episode in the Podcasts list and it plays through the same libmpv engine, Now-bar, and queue drawer as music, streamed if it is not downloaded, from the local file if it is. The one unified queue now genuinely interleaves tracks and episodes.

- **Episode queue writes (`conservatory-core`):** `enqueue_episodes` / `replace_queue_with_episodes` (mirroring the track variants; the queue schema already carries `episode_id`). `load_queue_display` now joins episodes, so a queued episode shows its title and its show in the drawer (`QueueDisplayRow` gains `episode_id`).
- **Queue builder (`conservatory/src/playqueue.rs`):** `build_episode_queue` turns episodes into `PlayableItem`s, using the downloaded file (`root` + `audio_path`) when present, else the enclosure URL (libmpv's `loadfile` streams a URL as-is). Source-less episodes are skipped. A basic `resolve_episode_profile` (no ReplayGain, no gapless; Smart Speed / Voice Boost is 6c).
- **Per-kind persistence guard (the load-bearing engine change, `player/engine.rs`):** the engine persists position + play counts only for `MediaKind::Track`. An episode plays to its end but does not yet write the music `playback_state` cursor or bump `tracks.play_count`, so an episode id can never leak into the music tables. Episode resume + per-kind persistence are 6b-ii-c-2. Music playback and resume are unchanged.
- **GUI:** the Podcasts episode list gets double-click / Enter to play the visible list from that row, and `Ctrl+Enter` to append, exactly the music leaf idiom. `EpisodeListRow` (and the `EpisodeRow` GObject) carry `audio_path` / `audio_url` so the view builds sources without a second read.
- **Tests:** the episode queue write + display join (core); `build_episode_queue` local-vs-stream-vs-skip (unit); and an engine **null-sink** test that plays an episode to EOF and asserts the guard held (no music cursor written). The existing track-playback test still passes, which is the music-regression check for the guard.

Deferred to 6b-ii-c-2: episode resume and per-kind persistence (write the podcast `playback` table on episode tick/EOF); to 6b-ii-c-3: per-show overrides.

Next: Phase 6b-ii-c-2 (episode resume + per-kind persistence).

## v0.0.31

Phase 6b-ii-b shipped: the Podcasts inbox is now actionable. Select an episode and mark it played, unplayed, or archived, or star it; the list's state glyph and the triage buckets update live. A Tags section joins the sidebar.

- **Triage writes (`conservatory-core`):** two **partial** playback upserts so an action never clobbers its siblings: `set_episode_played(episode_id, state, when)` (preserves starred / play_count; marking unplayed rewinds the resume position) and `set_episode_starred(episode_id, starred)` (preserves played / position). New worker commands in the single-writer ledger.
- **Tag reads:** `list_all_tags` (every tag, for the sidebar) and `episodes_for_tag` (episodes of shows carrying a tag, with triage state), reusing the 6b-ii-a episode-list projection.
- **CLI:** `podcast mark <db> <episode_id> <played|unplayed|archived>` and `podcast star <db> <episode_id> [--off]`, the headless surface. Verified end to end against a live feed (mark an episode played, watch it move to the Played bucket; star preserved across the mark).
- **GUI (`conservatory/src/ui/podcasts.rs`):** the detail pane gains a triage action bar (Mark played / unplayed, Archive, Star), enabled when an episode is selected, that writes through the single-writer worker (the music edit path's `rt.block_on(worker.*)` idiom) and re-loads the current source so the glyph and bucket counts refresh. The sidebar gains a Tags section.
- **Feature-gated** as before; the `--no-default-features` music-only build is unchanged and green.
- **Tests:** the partial writes (mark-played keeps starred and vice-versa; mark-unplayed rewinds position; archived maps to ArchivedUnlistened) and the tag-filter read, as a core integration test; the bucket reads reflect the new state.

Deferred to 6b-ii-c: episode playback in the unified queue (stream or downloaded) and per-show overrides (speed / Smart Speed / Voice Boost / inbox policy). Those touch the player engine (per-kind persistence, a generalised queue item, resume), so they land as their own commit.

Next: Phase 6b-ii-c (episode playback + the unified queue + per-show overrides).

## v0.0.30

Phase 6b-ii-a shipped: the Podcasts tab can now browse your subscriptions. The empty placeholder from 6b-i is replaced with a three-pane triage browse (read-only); the triage actions and episode playback are the next step (6b-ii-b).

- **Triage reads (`conservatory-core`):** a new `EpisodeListRow` (episode display fields + show title + the triage state joined from `playback`, defaulting to Unplayed, plus an `in_queue` flag) and two reads: `episodes_for_show` (a show's episodes, newest first) and `episodes_in_bucket` for the Inbox / Queue / Played buckets across every subscription. The buckets are derived, not stored (spec §4.2): Queue is unified-queue membership, Played is `played >= PlayedFully`, Inbox is the rest.
- **CLI:** `podcast episodes <db> [--show <id> | --bucket inbox|queue|played]` (read-only; `--tsv/--json/--human`), the headless surface for the same data. Behind the `podcasts` feature.
- **Podcasts view (`conservatory/src/ui/podcasts.rs`, GTK):** a sidebar of the triage buckets and subscribed shows; an episode list (`ColumnView`) with a played-state glyph, title, date, and length; and a detail pane with the show notes. Built lazily on the page's first `::map` (the 6b-i slot) over the read pool. Notes are rendered as raw feed text for now (the `ammonia` sanitize is deferred). The three panes are nested `gtk::Paned` (matching the music browse); an adaptive `AdwNavigationSplitView` is a later refinement.
- **Feature-gated:** the new `EpisodeRow` GObject and the whole `podcasts` UI module compile only with the `podcasts` feature; the `--no-default-features` music-only build is unchanged and stays green.
- **Tests:** the bucket derivation is covered by a core integration test (an episode in the queue is Queue, a played one is Played, an untouched one is Inbox); `EpisodeRow`'s formatting is unit-tested; and the GTK view's construction is exercised by a display-guarded build test (runs under a session, skips on a headless CI).

Deferred to 6b-ii-b: the triage actions (mark played / unplayed / archived / starred), per-show overrides, episode-into-the-unified-queue insertion, and streaming-or-local episode playback. A Tags sidebar section also lands then (it needs a tag-filtered episode read).

Next: Phase 6b-ii-b (triage actions + episode playback).

## v0.0.29

Phase 6b-i shipped: the multi-view window shell. The GUI is no longer a single music window; it now has a top-level view switcher, with Music as the first tab and a Podcasts tab alongside it. This is the structural groundwork for the Podcasts UI (the triage panes land in 6b-ii); the Podcasts tab is an empty placeholder for now.

- **View stack (`conservatory/src/ui/window.rs`):** the music browse (facet panes, filter bar, track list, queue drawer) is now one page of an `AdwViewStack`. The header carries an `AdwViewSwitcher` (`policy = wide`, the libadwaita 1.4+ idiom; the deprecated `AdwViewSwitcherTitle` is not used). The always-on filter bar moved from a global top bar into the Music page, so a music filter no longer shows over the Podcasts tab; its behaviour (`Ctrl+F`, the grammar, the debounce) is unchanged.
- **Adaptive collapse:** an `AdwBreakpoint` (max-width 550sp) hides the header switcher and reveals a bottom `AdwViewSwitcherBar` on narrow widths. The Now-bar stays the stable innermost bottom bar; the switcher bar reveals *beneath* it (the spec §2.3 stacking call, no GNOME precedent to copy).
- **Lazy + feature-gated:** the Podcasts page builds its child on first `::map`, not at startup (it retains state once built). The whole multi-view chrome (switcher, breakpoint, bottom bar, Podcasts page) is behind `#[cfg(feature = "podcasts")]` (the binary's first feature gates); a `--no-default-features` music-only build keeps a single-page stack with **no switcher chrome**, visually identical to before.
- **Keyboard:** `Alt+1` / `Alt+2` switch views (a global `ShortcutController`, the `AdwTabView` `Alt+N` convention; `Ctrl+1/2/3` stays free for the 6b-ii triage lists). `Alt+3` is wired but inert until the Audiobooks tab (7b). `docs/keymap.md` updated.
- **Tests:** the `Alt+N` → page-name mapping is a pure unit test; the widgets are verified by build + a launch smoke (the window constructs and runs cleanly with the new tree), the 3b/3c/4b precedent. The music-only build stays green.

No new dependencies; no schema, engine, or core changes (a pure GTK restructure). Next: Phase 6b-ii (the Podcasts triage panes: sidebar of triage lists / shows / tags, episode list, detail pane, with episodes flowing into the unified queue).

## v0.0.28

Phase 6a-iii-b shipped: private-feed credentials and episode download. This completes Phase 6a, the headless podcast subsystem; what remains for podcasts is the GUI (the Podcasts tab and triage, Phase 6b).

- **Credential store (`conservatory-podcasts/src/credentials.rs`):** a `CredentialStore` enum with a libsecret backend (via `oo7`) and an in-memory backend for tests and headless environments. The password lives in the secret service; the database stores only `shows.auth_user` and an opaque `shows.auth_pass_ref` lookup key, never the password inline (spec §8). The enum (rather than a `dyn` trait) keeps the async methods simple.
- **HTTP Basic auth wiring:** the fetcher gained `fetch_authed`, and `refresh_show` / `refresh_all` now resolve a show's stored credential and attach `Authorization: Basic` for a private feed. The anonymous path is unchanged; a missing secret service just leaves private feeds anonymous (a 401 then surfaces as that show's `Failed` outcome).
- **Episode download (`conservatory-podcasts/src/download.rs`):** `download_episode` streams an episode's audio into `<root>/<folder_path>/<filename>` (the managed layout, spec §5.3) and records the relative `audio_path`. The write is crash-safe in the `mover::fsops` shape: stream to a sibling `.part` file, fsync, then rename into place, so a partial download is never mistaken for a complete one. It reuses the fetcher's connection pool and carries the show's Basic-auth credentials.
- **Core:** a new `set_episode_audio_path` worker command records the download path (`upsert_episode` deliberately preserves `audio_path` across a re-fetch, so it cannot set it), plus a `get_episode` read by id.
- **CLI:** `podcast download <db> <episode_id> --root <root>` (resolves the episode + its show's credentials, downloads, records the path), behind the `podcasts` feature; the music-only build stays green.
- **Dependency:** `oo7` activated (signed off, spec §11; ATTRIBUTIONS.md), with `default-features = false` + the `tokio` runtime (its default pulls async-std, which clashes with the workspace's tokio) and `native_crypto` (pure-Rust file backend, no system OpenSSL).
- **Tests:** credential round-trip + resolve rules (in-memory backend); a Basic-auth-gated download (401 without the credential, 200 with it flowing through the store); the download writes the file and sets `audio_path`; filename derivation (URL basename else MIME-typed fallback). All hermetic (wiremock + a temp-DB worker).

Next: Phase 6b (the Podcasts tab: the window shell with an adaptive view switcher, then Belfry's Inbox → Queue → Played triage).

## v0.0.27

Phase 6a-iii-a shipped: OPML import and export. A subscription list round-trips, preserving tags and the Apple show id, so you can move in from another podcast app or back up your subscriptions.

- **OPML module (`conservatory-podcasts/src/opml.rs`):** `parse_opml` reads every `<outline>` carrying an `xmlUrl` (folder hierarchy is flattened, the Belfry tag-round-trip stance), pulling the feed URL, the title (`title`, else `text`, else the URL), the Pocket Casts `category="a,b"` tags, and `applePodcastsID`. `write_opml` emits an OPML 2.0 document with XML-escaped attributes. The parser is forgiving in the house style: a malformed or foreign OPML yields whatever outlines parsed cleanly rather than erroring.
- **Import is network-free:** `import_opml` creates (or resolves) each subscription's show through the single-writer worker and applies its tags via the `get_or_create_tag` / `set_show_tags` methods from 6a-i; `applePodcastsID` lands in `shows.apple_podcasts_id`. Episodes are not fetched here; a subsequent `podcast refresh` pulls them (so importing dozens of feeds is instant). `export_opml` reads the shows and their tags back out.
- **CLI:** `import-opml <db> <file>` (reports created vs already-subscribed) and `export-opml <db> [--out <file>]` (stdout by default), both behind the `podcasts` feature. The music-only build does not expose them and stays green.
- **Tests:** `opml.rs` unit tests (round-trip with escaping, forgiving parse of nested/foreign outlines, the title fallback chain) and `tests/opml.rs` (import through a real worker creates shows + tag links + the Apple id; a re-import is idempotent; export then re-parse returns the same subscription set). Hand-verified end to end through the CLI.

No new dependencies (`quick-xml` was already pulled at 6a-ii-b). Next: Phase 6a-iii-b (libsecret credentials via `oo7` for HTTP Basic auth, and episode download into the managed tree).

## v0.0.26

Phase 6a-ii-b shipped: feed parsing and the refresh pipeline. A feed URL now becomes a subscribed show with its episodes, entirely headless. This completes the headless fetch-and-parse half of the podcast absorption (6a-ii); OPML, credentials, and downloads are 6a-iii.

- **Parse (`conservatory-podcasts/src/parse.rs`):** `parse_feed` runs the body through `feed-rs` for the RSS/Atom/JSON core and through the hand-rolled namespace pass, merging the two by item position with a guid cross-check. It yields a storage-agnostic `ParsedFeed` (channel metadata + a flat `Vec<ParsedEpisode>`), so it stays a pure, fixture-tested function; the refresh layer maps it into core `Show` / `Episode` rows. Episode identity is `(show_id, guid)` (spec §8): the item-level `<podcast:guid>` when present, else feed-rs's entry id. The enclosure (URL / MIME / size) comes from feed-rs's media objects; `itunes:duration` gives the runtime.
- **Namespace handler (`conservatory-podcasts/src/namespace.rs`):** ported from Belfry's `fetch/namespace.rs` (the `quick-xml` event walker for `<podcast:guid>`, season, episode, and the chapters URL), and **extended** to also read `itunes:season` / `itunes:episode` / `itunes:episodeType`. `feed-rs` surfaces none of those, and real Apple-style feeds carry season/episode/type in the iTunes namespace far more often than in `podcast:`, so without this the columns would almost never populate. `podcast:` values win when both appear, regardless of element order.
- **Slugs (`conservatory-podcasts/src/slug.rs`):** `slugify` and `episode_dir` render the managed `Podcasts/<show-slug>/<YYYY-MM-DD>--<episode-slug>` layout (spec §5.3), so each episode row is download-ready before any byte is fetched.
- **Refresh orchestration (`conservatory-podcasts/src/refresh.rs`):** `add_show` (unconditional fetch → create → upsert), `refresh_show` (conditional GET honouring the stored ETag / Last-Modified; a 304 just bumps `last_fetched`), and `refresh_all` (every subscription concurrently under a `Semaphore`, via a `JoinSet`, aggregating per-show outcomes). A refresh rewrites only the descriptive metadata and the HTTP validators; user-configured fields (priority, keep_count, auto_download, auth, cover/accent) are preserved. Triage (inbox policy, playback rows, queue insertion) is **not** here; that is Phase 6b. Re-adding an existing feed is idempotent (it just refreshes).
- **CLI (`conservatory-cli`, behind `#[cfg(feature = "podcasts")]`):** `podcast add <db> <url>`, `podcast remove <db> <show_id>`, and `podcast refresh <db> [show_id]`, with `--tsv` / `--json` / `--human` output. The music-only build (`--no-default-features`) does not expose them and stays green.
- **Dependencies activated** in `conservatory-podcasts`: `feed-rs` (RSS/Atom/JSON core) and `quick-xml` (the namespace pass), both already in the workspace catalog; plus a path dependency on `conservatory-core` so the plugin can drive the typed worker methods (the §2.2 boundary is code and dependencies, not the schema, and there is no cycle). `ATTRIBUTIONS.md` records the sign-off and the Belfry namespace provenance.
- **Tests:** parse unit tests (channel + episode extraction, guid precedence, enclosure, the podcast-vs-itunes precedence) and `tests/refresh.rs` (wiremock + a real core worker on a temp DB): `add` lands both episodes, a second `refresh` dedups by `(show_id, guid)` and counts only the genuinely-new episode, and the conditional-GET round-trip stores an ETag on `add` then replays it for a 304 that leaves the episode set untouched. Two committed feed fixtures back the wiremock tests.

Next: Phase 6a-iii (OPML round-trip, libsecret credentials via `oo7`, and episode download into the managed tree).

## v0.0.25

Phase 6a-ii-a shipped: the RSS-catching layer. The `conservatory-podcasts` plugin crate gains a real HTTP client and a conditional-GET feed fetcher, both ported from Viaduct. Headless and wiremock-tested; no parsing or CLI yet (that is 6a-ii-b).

- **HTTP client (`conservatory-podcasts/src/http.rs`)**, ported from Viaduct's `network/http.rs` (lineage NetNewsWire): rustls TLS, gzip + brotli, `POOL_MAX_IDLE_PER_HOST = 4`, 30 s idle/request and 10 s connect timeouts, a descriptive `Conservatory/<version> (podcast client; +URL)` User-Agent, and the `ACCEPT_FEED` header. `build_client()`.
- **Conditional-GET fetcher (`conservatory-podcasts/src/fetcher.rs`)**, ported from the network slice of Viaduct's `network/fetcher.rs`. This is the heart of your "use Viaduct's method for RSS catching" steer: Belfry's fetch loop was only ever a stub, so the mature path wins. `fetch(url, etag, last_modified)` sends `If-None-Match` / `If-Modified-Since`, short-circuits a 304 with an empty body, extracts `ETag` / `Last-Modified` / `Cache-Control: max-age` from a 2xx, and keeps a per-host 429 cooldown that honours `Retry-After` (a host in cooldown short-circuits without a network hit). `FetchError` is the typed error.
- **Deliberately simpler than Viaduct:** the broadcast request-coalescing is dropped (each show has a distinct feed URL, so same-URL coalescing rarely helps) and the content-hash re-parse skip is deferred to the refresh orchestration at 6a-ii-b (where the stored hash will live). Documented in the module headers.
- **Dependencies activated** in `conservatory-podcasts`: `reqwest` (rustls-tls + gzip + brotli), `tokio`, `chrono`, `thiserror`, `tracing`, plus `wiremock` as a dev-dep. `bytes` stays deferred (the body is a `Vec<u8>`, so the crate never names `Bytes`). `ATTRIBUTIONS.md` records the Viaduct/NNW provenance and the new deps.
- **Tests (`tests/fetcher.rs`, wiremock, hermetic):** a 200 returns the body and extracts the validators; a conditional request sends `If-None-Match` and the server's 304 is handled; a 429 with `Retry-After` returns `RateLimited` and the cooldown short-circuits the next fetch (asserted by an `expect(1)` mock); an invalid URL is reported; plus `max-age` parse and client/UA unit smoke tests.

The music-only build is unaffected (the plugin is excluded under `--no-default-features`). Next: Phase 6a-ii-b (feed-rs + Belfry's `namespace.rs` parse, the refresh orchestration writing through the 6a-i worker, and the `podcast add|remove|refresh` CLI verbs).

## v0.0.24

Phase 6a-i shipped: the podcast schema and the core worker CRUD that backs it. **Phase 6 (absorb Belfry) begins.** This is the headless DB foundation; no network code yet (that is 6a-ii). The Belfry subsystem is being absorbed table by table into Conservatory's core-owned ledger.

- **Migration `0006` — the eight podcast tables**, ported from Belfry (`shows`, `episodes`, `playback`, `show_settings`, `listening_sessions`, `chapters`, `tags`, `show_tags`), with one deliberate change (spec §4.2): triage Queue state lives in the unified `queue` table, so `playback` drops Belfry's `in_queue` / `queue_position` columns. Inbox / Queue / Played derives from `playback.played` plus `queue` membership. `episode_fts` / `show_fts` join the FTS set as ordinary trigger-synced tables, matching the music FTS style in `0001`.
- **The unified queue gained its `episode_id` foreign key.** Migration `0006` rebuilds `queue` to add the FK that was deferred at `0005` (with `foreign_keys = ON`, SQLite refused a child FK to the then-absent `episodes` table). `book_id` stays plain until `books` lands at Phase 7. The saved playback queue is copied across the rebuild.
- **Core domain models + worker CRUD:** `Show` / `Episode` / `Playback` (+`PlayedState`) / `ShowSettings` (+`InboxPolicy`) / `ListeningSession` / `Chapter` / `Tag` in `db/models.rs`; podcast reads in `db/reads.rs`; and the worker write path (`get_or_create_show`, `update_show` — carrying the conditional-GET state the fetch loop will refresh — `delete_show`, `upsert_episode` by `(show_id, guid)`, `upsert_playback`, `upsert_show_settings`, `replace_chapters`, `get_or_create_tag`, `set_show_tags`). The schema is core-owned (the §2.2 boundary rule); the `conservatory-podcasts` plugin (6a-ii onward) consumes these typed `WorkerHandle` methods. `upsert_episode` deliberately never overwrites a downloaded `audio_path` on a re-fetch.
- **On the Viaduct/Belfry split (settled, lands at 6a-ii):** RSS *catching* (the HTTP client + conditional-GET fetcher) ports from **Viaduct** (`network/http.rs` + `network/fetcher.rs`), the mature, proven path; Belfry's fetch loop was only ever a planned stub. RSS *parsing* stays `feed-rs` plus Belfry's hand-rolled `podcast:` namespace handler (spec §8, §11).
- **Tests:** `tests/podcasts.rs` (9) covers show idempotency, episode upsert/dedup + download-path preservation, FTS sync across edit/delete, playback + settings round-trip, chapter replace, tag round-trip, and the queue `episode_id` FK (via `PRAGMA foreign_key_list`); the migration table-exists check is extended. The music-only build (`--no-default-features`) stays green: core is feature-free and the tables apply in every build.

No new dependencies (6a-i pulls none; the heavy podcast deps land with the fetcher at 6a-ii), so `ATTRIBUTIONS.md` is untouched. Next: Phase 6a-ii (the Viaduct-style fetcher + `feed-rs`/namespace parse + the refresh pipeline).

## v0.0.23

The default music layout gains a top-level **`Music/`** folder, so a library root holds `Music/`, `Audiobooks/`, and `Podcasts/` side by side (spec §5.1).

- `DEFAULT_MUSIC_TEMPLATE` is now `Music/{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}` (was no prefix). New imports land under `Music/`.
- **Existing managed libraries:** running `organize` re-shelves the music tree into `Music/`. The move is journaled and undoable like any other, but it relocates every album, so expect one big move the first time.
- Docs: spec §5.1 + §5.7 + `docs/path-template.md` record the canonical per-media layout (audiobooks put standalone books under a literal `Standalone/` folder; podcasts already used `Podcasts/<show>/<episode>`).

This is a docs-and-default change; no engine behaviour changed beyond the rendered path.

## v0.0.22

Phases 5c (ReplayGain scan) and 5d (cover art to disk) shipped together. **Phase 5 — bulk editing, write-back, ReplayGain, and covers — is complete.**

**5c — ReplayGain scan (`rsgain`):**
- `conservatory-core/src/replaygain.rs` shells `rsgain` (the Lattice invocation: album gain, write tags, clip-protect) to compute ReplayGain 2.0 for an album, then re-reads the written gains and refreshes the DB `replaygain_*` columns the player's profile resolution consults. rsgain was chosen over the `ebur128` Rust crate because the crate only measures decoded PCM and the pure-Rust decoder can't handle Opus (half the library); rsgain decodes every format itself. It is an external tool (ATTRIBUTIONS.md); spec §16.7 is settled.
- CLI: `replaygain scan <db> <selector> --root <root> [--apply] [--target-lufs N]` (per-album; dry-run lists the albums, `--apply` scans and syncs the DB).

**5d — cover art to disk:**
- `conservatory-core/src/covers.rs` writes each album's `cover.jpg`/`.png` into the managed folder and records `albums.cover_path`. Import writes covers; `organize` and path-affecting edits **resync** them (covers follow their album to the new folder, the stale one removed). The trust-critical mover is untouched: covers are derived, so they are synced idempotently rather than journaled.
- CLI `set-cover <db> <album_id> <image> --root` sets an album's cover (and refreshes the accent).
- The deferred **Now-bar cover thumbnail** and MPRIS **`mpris:artUrl`** are now wired (the Now-bar shows the album art; `mpris:artUrl` is `file://<root>/<cover_path>`).
- Tests: `tests/replaygain.rs` (hermetic DB-sync + a skip-if-absent rsgain scan over FLAC + Opus) and `tests/covers.rs` (import writes the cover; an edit moves it).

Still deferred: the APE-strip (Phase 8c byte-level pass), the in-dialog GUI cover field (the CLI `set-cover` covers it), online cover fetch.

## v0.0.21

Phase 5b-ii shipped: the GUI write-back action. Phase 5b (embedded-tag write-back) is complete.

- **"Embed metadata into files" header button** (the save icon): writes the database metadata into the selected files, behind a "Write tags to N file(s)?" confirm and a result dialog. Explicit, not automatic on every edit (the Calibre model); shares the v0.0.20 `write_track_tags` core. Needs the library root (launch as `conservatory <db> <root>`).

Next: Phase 5c (ReplayGain scan, in-process via the `ebur128` crate + lofty), then 5d (cover to disk + cover field).

## v0.0.20

Phase 5b-i shipped: embedded-tag write-back. The curated DB metadata can now be written back into the files, so the managed library is never a roach motel: walk away with the tree and the files describe themselves.

- **Write-back core (`tags::write_track_tags`, lofty):** writes the format's canonical primary tag authoritatively (title, track artist + sort, album, album artist + sort, year, track/disc, raw multi-value genres), creating it if the file had none and dropping the legacy ID3v1. Only the rebuildable descriptive layer is written; the curated layer (rating, shelf genre, play counts, starred) stays DB-only per §5.6. A new `db::writeback_rows` join supplies the per-track data (display + sort names + group-concat genres).
- **CLI `embed-tags <db> <selector> --root <root> [--apply]`:** dry-run by default (shows the per-file field diffs, current tags vs DB); `--apply` writes. No undo journal: write-back is re-derivable from the DB (the source of truth), so re-running fixes any mistake.
- **Tests (`tests/writeback.rs`):** per-format round-trip (edit DB → embed → re-read the file) and the **§5.6 re-import contract** (embed → wipe DB → re-import → the edited album survives). Verified manually against the `testdata/` albums.

**Scope note — APE-strip deferred.** The Lattice `apestrip` hygiene (stripping a stray APEv2 that shadows ID3 on MP3) is not in 5b: lofty reads APE on MPEG but neither writes nor removes it, so a reliable strip needs byte-level surgery (which is why `apestrip.py` is hand-rolled). It is deferred to a byte-level pass paired with the Phase 8c "detect stray APE" audit. `embed-tags` writes the canonical ID3v2 correctly; it just cannot remove a pre-existing APE shadow on MPEG.

Next: Phase 5b-ii (a GUI "Embed metadata into files" action), then 5c (ReplayGain scan).

## v0.0.19

Phase 5a-ii shipped: the GTK bulk-edit dialog. Phase 5a (bulk metadata editing) is complete.

- **Bulk-edit dialog (`ui/window.rs`):** select tracks in the browser and press the header pencil button or `Ctrl+E` to open an edit dialog: one entry per field (album artist, album, year, shelf genre, track artist, title, raw genres, rating), blank means unchanged. Filled fields are parsed through the shared `core::edit` resolver (a bad year/rating rejects the whole set), then applied across the selection through the single-writer worker.
- **Path-affecting edits are confirmed:** changing album / album artist / year / shelf genre writes the values, then shows a "Move N files?" preview (the `mover::plan` dry-run) before relocating the touched albums with the Phase 2c mover (undoable). The browse refreshes after the edit.
- Search-and-replace remains a headless verb (`tag replace`, v0.0.18); the in-dialog replace mode is deferred. Live incremental refresh (the deferred `LibraryChanges` delta) is still a full reload.

The dialog is GUI (build + manual verification); the edit and move logic it drives is covered by the v0.0.18 `tests/edit.rs`.

Next: Phase 5b (embedded-tag write-back) — write the curated DB metadata back into the files.

## v0.0.18

Phase 5a-i shipped: headless metadata editing. The library is no longer read-only after import; you can edit fields across a selection from the CLI, and path-affecting edits re-shelve files safely.

- **Edit commands (`conservatory-core`):** new single-writer commands `update_track` (title / rating / track artist), `update_album` (title / year / shelf genre / album artist), and `set_track_genres` (the raw multi-value side, §5.2). `COALESCE`-guarded so only changed fields move; setting an artist resolves it through get-or-create by derived sort name. The FTS index re-syncs automatically on every edit (the existing triggers), verified by test.
- **Pure resolver (`src/edit.rs`):** parses `field=value`, classifies track-level vs album-level and which edits are path-affecting (album / albumartist / year / shelfgenre), builds the typed edits, splits raw genres, and does literal search-and-replace. Unit-tested; shared by the CLI now and the GTK dialog at 5a-ii.
- **Path-affecting edits reuse the Phase 2c mover:** an album / albumartist / year / shelf-genre change re-renders the touched albums and moves the files, with the same dry-run preview + undo journal as `organize`.
- **CLI:** `tag set <db> <selector> <field=value>...` and `tag replace <db> <selector> <field> <find> <replace>` (selector is a full search expression; `--root` + `--apply` drive the move).
- **Tests:** `tests/edit.rs` covers field updates, FTS-follows-rename, genre relink (replace not append), and a year edit that re-renders, moves, and undoes. CI uses the committed fixtures; the real `testdata/` albums are the manual corpus.

Also settled (recorded in the roadmap, deps added when those phases land): Phase 5c ReplayGain scanning will use the in-process `ebur128` crate + lofty (no external binary); Phase 8a integrity verification will use `flac -t` + the `ffmpeg` CLI.

Next: Phase 5a-ii (the GTK bulk-edit dialog), then 5b (embedded write-back).

## v0.0.17

Phase 4c-ii shipped: the output-device picker. **Phase 4 is complete — a daily-driver music player.**

- **Output devices (`player/host.rs`):** `MpvHost::audio_devices()` parses mpv's `audio-device-list` (a node array of maps) into `AudioDevice { name, description }`, and `set_audio_device()` sets the `audio-device` property. The engine queries the list once at init and carries it (plus the current selection) on the snapshot; a `SetAudioDevice` command applies a switch through the engine thread.
- **Header picker (`ui/window.rs`):** a `MenuButton` whose popover is built fresh on each open from the snapshot — the sinks (plus `auto`), the current one checked; clicking one switches output. No D-Bus; mpv handles the device move live.
- **MSRV:** `rust-version` bumped to 1.88 to match the let-chains already in use (introduced with the MPRIS module at 4c-i); CI builds on stable, so this is a documentation correction, not a behaviour change.
- **Tests:** a host integration test (`audio_devices()` includes `auto`, `set_audio_device("auto")` ok); the menu is verified by build + manual launch.
- **Fix — GUI playback:** the GUI never actually played, because libmpv's `mpv_create()` returns NULL unless `LC_NUMERIC = "C"`, and GTK sets the locale from the environment at startup (the CLI never does, so it was unaffected). `MpvHost::build` now calls `setlocale(LC_NUMERIC, "C")` (via `libc`, signed off) before creating mpv. Also: `scripts/demo.sh` now passes the library root (`conservatory <db> <root>`), without which the GUI can browse but not play, and a missing-root launch logs a hint instead of failing silently.

With this, Phase 4 (libmpv playback, the unified queue, the GUI player + Now-bar + queue drawer, MPRIS2/media keys/inhibitor, and output selection) is done. Deferred polish carried forward: MPRIS `Quit`/`Raise` wired to the app, `mpris:artUrl` + a Now-bar cover (need covers on disk, §7.4), the audible within-album gapless prototype (§16.9), and in-window keyboard playback bindings. Next is **Phase 5 — bulk editing + embedded-tag write-back.**

## v0.0.16

Phase 4c-i shipped: MPRIS2 and a suspend inhibitor. The player is now a desktop citizen — media keys, the GNOME media overlay and lock screen, and don't-suspend-while-playing.

- **MPRIS2 (`conservatory-core/src/mpris.rs`, on `zbus 5`, signed off):** serves `org.mpris.MediaPlayer2` and `…Player` on the session bus. Properties (PlaybackStatus, Metadata, Position, Volume, CanGoNext/Previous, …) and methods (Play/Pause/PlayPause/Next/Previous/Stop/Seek/SetPosition) drive the `PlayerHandle`. `run(player, pool)` polls the engine snapshot (~300 ms), emits `PropertiesChanged` on change, and resolves the current track's metadata via a new `track_metadata` read (the snapshot carries only a track id). The GUI spawns it on its runtime; **media keys and the GNOME overlay/lock screen come for free** (GNOME routes them to MPRIS).
- **Suspend inhibitor:** a logind `Inhibit("sleep", …, "block")` proxy on the system bus, the FD held while playing and released on pause/stop. Best-effort: a missing system bus or logind disables the inhibitor without affecting MPRIS.
- **In core, not the GTK binary** (spec §16.13): the whole surface is `conservatory-core`, spawned by the GUI; no new widgets. The state→D-Bus mapping is pure, unit-tested helpers (PlaybackStatus, CanGoNext/Previous, wants_inhibit, volume/position conversions, metadata); a `track_metadata` worker test covers the join. Live D-Bus is verified manually (`playerctl`, `systemd-inhibit --list`).

Deferred to 4c-ii: the PipeWire output-sink picker (mpv `audio-device` + a header menu). Also deferred: MPRIS `Quit`/`Raise` wired to the app, `mpris:artUrl` (needs covers on disk, §7.4), and the in-window keyboard playback bindings. After 4c-ii, Phase 4 — the daily-driver player — is complete.

## v0.0.15

Phase 4b-ii-c shipped: queue polish. The queue now survives a restart, and you can add to it from the browse list.

- **Launch-resume:** on startup `resume_saved_queue` loads the saved DB queue into the engine **paused at the cursor** (a new `paused` flag on the engine's `SetQueue`, exposed as `PlayerHandle::resume` + a seek to the saved offset), so reopening the app shows the last track in the Now-bar, paused, with the saved queue in the drawer; press play to continue. Opening makes no sound.
- **`Ctrl+Enter` append:** appends the browse selection to the queue, both the DB tail (`enqueue_tracks`) and the live engine tail (the new `AppendItems` command, which starts playing if the queue was idle). Plain Enter / double-click still *replaces* the queue.
- **Tests:** an engine null-host integration test covering append-to-idle (starts playing), a second append (extends the tail, current unchanged), and resume (a fresh engine loads the whole queue paused at the cursor). The GUI wiring is verified by build + manual launch.

Deferred: the Now-bar cover thumbnail (blocked until covers are written to disk, spec §7.4); the audible within-album gapless prototype (§16.9); the `playback_state` explicit queue-entry reference. Phase 4c is the system-integration finish (MPRIS2 + media keys + PipeWire sink picker + suspend inhibitor); the library root moves to config at Phase 10.

## v0.0.14

Phase 4b-ii-b shipped: a drag-and-drop queue drawer. The queue you're playing is now visible, reorderable, and editable, with the playing track highlighted. (Launch-resume, append, and a cover thumbnail are 4b-ii-c.)

- **The drawer (`conservatory/src/ui/queue_panel.rs`):** a right-side slide-in `gtk::Revealer` (header toggle + `Ctrl+U`) holding a `ListView` of the queue, each row a kind icon over title/artist, the playing row accent-highlighted. Rows are **drag-and-drop reorderable** (the Atrium idiom: the `DragSource` carries the row's position, the `DropTarget` computes Above/Below from the cursor Y, both controllers torn down in `unbind` so they don't leak on recycling). Keyboard too: `Alt+↑/↓` reorder, `Delete` removes, `Ctrl+Shift+C` clears.
- **Live engine mutation (`conservatory-core/src/player/`):** the engine gained `MoveItem` / `RemoveItem` / `ClearQueue` so editing the queue never restarts the current track. The `current_index` adjustment is pure and unit-tested (`move_current_index` / `remove_current_index`): the playing item follows a move, a remove-before shifts it down, removing the current item reloads what fell into its slot. `MpvHost::stop` unloads on clear.
- **DB queue is the source of truth (spec §4.3):** double-click now **writes the DB queue through** (`replace_queue_with_tracks`) before playing, and every drawer edit applies the identical `(from, to)` to both `worker.reorder_queue` and `player.move_item`, so the DB position and the engine index stay aligned. New core read `load_queue_display` (queue ⋈ tracks ⋈ artists) backs the drawer; the playing-row highlight follows the engine via the 250 ms snapshot poll.
- **Tests:** the index helpers (8), `drop_target_position` (Above/Below, dragging up/down, end clamp), an engine null-host integration test that moves and removes queue items and asserts `current_index` tracks correctly *without* restarting the current track, and a `load_queue_display` worker test. The widgets are verified by build + manual launch (the 3b/3c precedent).

Deferred to 4b-ii-c: launch-resume (load the saved queue paused at the cursor on startup), `Ctrl+Enter` append, a Now-bar cover thumbnail, the audible within-album gapless prototype (§16.9), and the `playback_state` queue-entry reference. MPRIS2 + media keys + inhibitor are Phase 4c; the library root moves to config at Phase 10.

## v0.0.13

Phase 4b-ii-a shipped: the browse window plays music. The threaded engine stands up in the GUI, a persistent Now-bar transport sits at the bottom, and double-clicking a track plays the list you're looking at. (The visible queue panel and drag-and-drop reorder are 4b-ii-b.)

- **Engine in the GUI (`conservatory/src/ui/window.rs`):** the `Player` is spawned on the window's existing tokio runtime right after the worker; a libmpv init failure leaves it unset and the transport inert (browse is unaffected). The window now also holds the snapshot poll source, the playing queue's track-id → title/artist map, and the library root.
- **Now-bar (`conservatory/src/ui/now_bar.rs`):** a persistent bottom bar (attached with `ToolbarView::add_bottom_bar`) showing title/artist, prev / play-pause / next (symbolic icons, no font assumption), a position label + seek slider, and a volume button. The transport buttons are non-blocking `PlayerHandle` sends; the seek slider drives playback through `change-value` (user drag only), so the 250 ms refresh's programmatic `set_value` never loops back into a seek.
- **Double-click / Enter plays the visible list (spec §3.6, the deadbeef idiom):** the leaf's display order becomes the queue and the activated row is the start. A pure `playqueue::build_play_queue` (headless-tested) turns the ordered ids + a batch `Track` read into resolved `PlayableItem`s, preserving order, joining the library root onto the relative paths, resolving each profile, and re-indexing the start past any track that vanished between the read and the build.
- **Snapshot polling + teardown:** a 250 ms `glib::timeout_add_local` refreshes the Now-bar (position/seek/icon every tick; labels only on track change). On window close the timer is removed first, then the player is shut down and joined (its terminal flush still has a live worker), then the worker/runtime drop — the order that keeps the final position write safe.
- **Core:** one new reusable read, `get_tracks` (a chunked `WHERE id IN (...)` that survives a full-library activation). The GUI takes an optional library root as a second arg (`conservatory <db> [root]`) until Phase 10 config sources it.
- **Tests:** `build_play_queue` (order, root-join, start re-index, missing tracks) and time formatting as pure unit tests; a `get_tracks` cross-chunk worker test. The widgets themselves are verified by build + manual launch (the Phase 3b/3c precedent).

Deferred to 4b-ii-b: the visible queue panel with drag-and-drop reorder (and `Alt+↑/↓` / `Delete` / `Ctrl+Shift+C`), `Ctrl+Enter` append, GUI resume-from-cursor, a Now-bar cover thumbnail, the audible within-album gapless prototype (§16.9), and the library root from config. MPRIS2 + media keys + inhibitor remain Phase 4c.

## v0.0.12

Phase 4b-i shipped: the unified queue and the threaded player engine, headless. The libmpv host moves off the CLI loop onto its own thread behind a cross-thread handle, and a real queue drives it. (The GTK Now-bar and the drag-and-drop queue view are 4b-ii.)

- **Unified queue (migration `0005`, spec §4.3):** the `queue` table lands with its full column set, but only `track_id` carries a foreign key for now. With `foreign_keys = ON` SQLite refuses any DML on a child table whose parent does not exist yet, even for a NULL column, so the `episode_id`/`book_id` foreign keys are added when the `episodes` (Phase 6) and `books` (Phase 7) tables land. Positions stay contiguous (`0..n-1`), renumbered transactionally on the single writer. New worker commands: enqueue, replace, remove, reorder, clear; `load_queue` reads it back in order.
- **Threaded `Player` engine (`conservatory-core/src/player/{engine,handle,item}.rs`):** a dedicated `std::thread` owns the `!Send` `MpvHost` (constructed there via a `make_host` factory, so it never crosses a boundary) behind a `Send + Clone` `PlayerHandle`. Commands (`play_queue` / `toggle_pause` / `next` / `previous` / `seek` / `set_volume` / `stop` / `shutdown`) flow out over a channel; state flows back through a `PlayerSnapshot` the consumer polls. On advance the engine applies the next item's profile before loading (the spec §16.9 boundary switch, music profile); it advances on a natural end-of-file, skips an errored item, and ignores the self-initiated stop its own load emits. Persistence is split (spec §6.4): debounced ticks are fired and forgotten through the runtime, while the terminal writes (pause, seek, stop, shutdown, and the play-count bump + final cursor on end-of-file) block on the worker so they are guaranteed to land.
- **`is:queued` is live (was inert since 3a):** `conservatory-search`'s SQL path emits `tracks.id IN (SELECT track_id FROM queue WHERE kind='track' ...)`; the eval path reads `SearchRow.queued`, an `EXISTS` against the queue computed in `search_rows`.
- **CLI:** `queue add | list | remove | clear`, and `play <db> <root> [track_id]` rewritten to drive the engine through the queue (the root resolves the relative `file_path`s; with a track id it replaces the queue, else it plays the existing queue from the saved cursor), polling the snapshot until the queue ends.
- **Tests:** queue position integrity (enqueue/remove/reorder stay a dense ordered range); `is:queued` membership; and the headline engine test, which imports the committed fixtures into a managed tree, plays the whole queue through a null audio output, and asserts every play count incremented once and the cursor landing on the last item (`tests/queue.rs`).

Deferred to 4b-ii: the persistent Now-bar transport; the drag-and-drop reorderable queue view (with keyboard fallbacks); the audible within-album gapless prototype (mpv playlist append, §16.9); the library root from config (Phase 10) rather than a CLI arg. MPRIS2 + media keys + inhibitor remain Phase 4c.

## v0.0.11

Phase 4a shipped: the libmpv playback host and the music profile. The engine can play a track from the managed library (the first sound Conservatory makes), headless via the CLI, with the position persisted so a restart resumes.

- **libmpv host (`conservatory-core/src/player/host.rs`):** a single `libmpv2` instance kept alive across items (`MpvHost`), with `load` / `set_paused` / `seek_absolute` / `time_pos` and a `pump` that maps libmpv events to a small `HostEvent`. The host is thin glue, kept in core (spec §16.13), so the whole engine stays CLI-driveable. `libmpv2 4.1` was signed off over the alternatives and pulled into core; the system `libmpv` joins GTK/libadwaita in CI. The threaded `Player` handle and command channel are deferred to 4b, where the GTK Now-bar is the second consumer; building that plumbing now, with only the CLI loop to drive it, would be speculative.
- **Music profile (`player/profile.rs`, pure + tested):** `resolve_music_profile` turns a track + the `[playback]` config (spec §10 defaults) into the gapless flag, the ReplayGain mode, and the crossfade duration. ReplayGain uses mpv's native `replaygain` property (mpv reads the same file tags `lofty` stored at import); the DB `replaygain_*` columns drive mode resolution, downgrading album→track→off by what the track actually carries. **Settled for 4a:** read-only ReplayGain (no in-app scan, §16.7) and no EQ/DSP (§16.6); both stay open. Crossfade is carried through but rendered at 4b with the queue.
- **State persistence (`player/state.rs`, pure + tested; migration `0004`):** a new singleton `playback_state` table is the transport cursor (what was playing and where). `StateDebounce` coalesces the steady position stream to one write per 30 s insurance interval while flushing immediately on pause/seek/item-end/quit; `EndReason::counts_as_play` gates the `play_count` + `last_played` bump to a natural end-of-file. Saves go through the single-writer worker (`save_playback_state` / `increment_play_count`).
- **CLI:** `play <db> [track_id]` plays a track (gapless + ReplayGain), persisting position on the interval and on end; with no id it resumes the saved cursor. Read the track through the pool, write state through the worker, all on one current-thread runtime.
- **Tests:** profile resolution + ReplayGain downgrade and the debounce/Eof logic as pure unit tests; `playback_state` round-trip and play-count increment through the worker; an `ao=null` libmpv smoke test that decodes a committed fixture to EOF (`tests/playback.rs`).

Deferred: the threaded `Player` handle + unified queue + Now-bar transport (4b); MPRIS2 + media keys + suspend inhibitor (4c); crossfade rendering (4b); EQ/DSP and ReplayGain scanning (§16.6/§16.7, still open).

## v0.0.10

Phase 3c shipped: the browse window becomes a working library browser. A sortable, multi-select track list; the always-on filter bar wired to the grammar; and Perspectives (named saved searches) in a sidebar, persisted through the single-writer worker (its first appearance in the GUI).

- **Track list (`conservatory/src/ui/track_list.rs`):** the full deadbeef columns (Artist | Album | Genre | Title | Duration | Rating). Click a header to sort; the comparison delegates to `core::cmp_tracks`, so the GTK sort and the headless `sort_tracks` can't drift. Multi-select (Ctrl/Shift) comes from `MultiSelection`; rating renders as accent-tinted symbolic stars (icon-theme glyphs, no font assumption); rows lift on hover. `TrackBrief` gained a name-ordered `genres` roll-up and `rating`.
- **Filter bar (spec §3.4):** an always-on `SearchEntry` under the header; `Ctrl+F` focuses it; no separate search mode. Typing narrows the leaf through the full grammar, debounced, intersected with the facet selection ("the panes filter, the grammar searches, same surface"). Malformed input degrades to substring and tints the bar, never errors. The composition lives in a non-GTK `query.rs` (headless-tested), keeping core runtime-search-free.
- **Perspectives (spec §3.4):** migration `0003` adds the core-owned `perspectives` table (saved searches as text, re-parsed on load). The sidebar lists Default + saved searches; Save names the current filter, clicking a row reloads it, Delete removes it. `vl:NAME` now resolves from storage, so a Perspective can reference another. Saves/deletes go through the single-writer worker (`save_perspective` / `delete_perspective`), which the browse window now stands up on a tokio runtime (the in-GUI writer, pulled forward from Phase 5a to back persistence).
- **Demo:** `scripts/demo.sh`'s headless path now previews the filter-bar grammar (live `search` runs) alongside the facets; the GUI hint mentions sorting, `Ctrl+F`, and Perspectives.

Deferred: live `BatchUpdate` / library deltas (still Phase 5a); user-reconfigurable + persisted pane order (Phase 10); the per-row playing/status glyph (waits for playback state, Phase 4).

## v0.0.9

Phase 3b shipped: the first GTK4/libadwaita code. `conservatory` is now a launching app with the deadbeef-cui "Columns UI" faceted browse (spec §3.3).

- **Facet logic (`conservatory-core/src/db/facets.rs`, headless + tested):** `facet_rows` (distinct values of Genre / Album Artist / Album with `COUNT(DISTINCT track)`, narrowed by upstream selections) and `facet_tracks` (the leaf set). Genre is multi-valued: a track tagged `Electronic; Ambient` counts under both rows (the §5.2 decoupling). The CLAUDE.md hard rule keeps the logic in core; the GTK binary only renders. `debug-facets <db>` exercises it headless.
- **GTK browse window (`conservatory/src/ui/`, programmatic):** an `adw::ApplicationWindow` laid out like deadbeef Columns UI: a row of facet panes on top, the track table below (a draggable split). Each pane is a `ColumnView` with a value column + right-aligned `Count` column, sortable headers, grid lines, and an `[All (N)]` top row; the leaf is a `ColumnView` track table (Artist / Album / Title / Duration). Selecting facet rows narrows the downstream panes and the leaf (the cascade). A small CSS pass tightens the rows; richer track columns (rating, bitrate), sorting, and multi-select land at 3c.
- **Coalescing:** ported Viaduct's `CoalescingQueue` (interval + max-interval flush, dedup) to debounce selection changes into one cascade recompute per multi-select drag, never per row (spec §2.1).
- **CI:** the `libgtk-4-dev` / `libadwaita-1-dev` install lands in both jobs.

Deferred: user-reconfigurable + persisted pane order (Phase 10 config); the sortable track list + filter bar (3c); `BatchUpdate` / live deltas (until an in-GUI writer, 5a).

## v0.0.8

Phase 3a shipped: the `conservatory-search` expression engine and a CLI `search` verb (the first piece of Phase 3, GTK browse).

- **Grammar pipeline (`conservatory-search`):** `lex` → `parse` (typed AST + extracted `sort:` specs) → `eval` (in-memory) + `sql_translate` (all-or-nothing SQL `WHERE`, so the two paths never diverge), with `rank` (bm25 + recency). Structure ported from `atrium-search`, semantics from CalibreQuarry, FTS plumbing from Viaduct; an independent implementation. Storage-agnostic (`SqlValue`, no rusqlite, no core); deps `regex` + `chrono` only.
- **Grammar:** the music field set (`artist`/`albumartist`/`album`/`title`, `genre` vs `shelfgenre`, `year`/`added`, `rating`/`bitrate`/`duration`/`format`, `is:played`/`is:starred`/`is:queued`), match modifiers (substring/`=`/`~`regex/`?`fuzzy), boolean + ranges + date keywords/precision + presence, `sort:` as metadata. The parser is **forgiving** (degrades to substring, never errors). `vl:` perspectives expand at parse time with a cycle guard.
- **CLI:** `search <db> '<expr>' [--format tsv|json|human]` — SQL fast path when the whole expression translates, else the in-memory evaluator; bare-text hits ranked by bm25 + recency. New core reads `search_rows` / `search_track_ids` / `fts_rank` (the consumer maps `SqlValue` → a core `SqlParam`, keeping core search-free).
- **Tests:** parse round-trip, per-field eval, per-node SQL, `vl:` cycle guard, and SQL-vs-eval **parity** over a 2,000-track fixture; hand-verified against the real imported albums.
- **Deferred:** persistent Perspective storage + UI (3c); `is:queued` matches nothing until the queue table lands (4b); podcast/audiobook fields (6/7).

## v0.0.7

Phase 2d shipped: the import pipeline and real CLI verbs. **The manager is usable headless** (the Phase 2 exit): point the CLI at a folder and get an organized, database-owned library.

- **Import pipeline (`src/import/`):** scan a folder → read tags → resolve artists/albums/genres → derive shelf genre + accent → render targets → move/copy into the managed tree. Runs in two passes: an in-memory resolution + conflict pre-check, then (only if clear) the persist + move, so a conflicting import leaves the database untouched. Import inserts at the source path and runs the journaled mover, so it is undoable and crash-safe like organize.
- **Resolver:** album grouping by `(artist, title)`; album artist from the shared album-artist tag, else shared track artist, else Various Artists; artist identity by `sort_name` (embedded `ARTISTSORT` preferred, else a leading-article derivation); album identity `(album_artist_id, title)` so re-imports reuse the album.
- **CLI:** `import <db> <source> <root>` (copies by default; `--move` to consume), `organize` (re-render from the DB; dry-run/`--apply`/`--undo`), `shelf-genre-set`. Output `--tsv` (default) / `--json` / `--human`. The old `debug-organize` is promoted to `organize`.
- **Worker:** `get_or_create_artist`/`get_or_create_album`/`set_album_shelf_genre`. The tag reader now also reads embedded sort-name tags.
- **Tests:** `tests/import.rs` end-to-end (import into a managed tree, copy-vs-move, re-import refusal, shelf-genre-set → organize) plus resolver/scan unit tests; hand-verified against two real albums (mp3 + opus).

## v0.0.6

Phase 2c shipped: the crash-safe file mover. This is the trust-critical, release-blocking subsystem (spec §5.4); moving the user's files is the headline risk.

- **Mover engine (`src/mover/`):** `plan` (pure dry-run preview with conflict detection), `apply` (journal-first, then execute), `undo` (revert a completed job), and `recover` (roll-forward replay of interrupted jobs at startup). The journal is a SQLite ledger (migration `0002`: `move_jobs` + `move_operations`), written before any file is touched and durable via WAL. Recovery rolls forward (completes the move the user asked for); replay is idempotent.
- **Per-file primitive (`mover::fsops`):** same-filesystem `rename` fast path; cross-filesystem copy → fsync → verify → delete (modeled on Atrium's atomic write). Idempotent: a file already at its target is a no-op, which is what makes crash replay safe.
- **Conflict policy:** duplicate targets, missing sources, and existing destinations refuse the whole job; nothing moves. Copy-vs-move is a per-job choice.
- **DB consistency:** completing an operation updates `tracks.file_path` and `albums.folder_path` in the same transaction as marking it done; undo reverts both the tree and the DB.
- **Worker + CLI:** new journal commands on the single writer (file I/O stays off it); `debug-organize <db> <root> [--apply] [--copy] [--undo <id>]`.
- **Tests:** the release-blocking suite (`tests/mover.rs`): move/undo round-trip, simulated mid-move crash rolling forward, conflict refusal, copy mode, tree↔DB consistency; plus `fsops` unit tests.

## v0.0.5

Phase 2b shipped: the shelf-genre resolver that decides each album's filed-under genre.

- **Resolver (`src/shelf_genre.rs`):** `normalize` splits raw tags on `;` `/` `,`, case-folds for matching, and maps through the alias vocabulary, keeping canonical/original casing in the output. `resolve_shelf_genre` runs the spec §5.2 priority chain (manual override → single album-level tag → most-common normalized track genre, ties broken by `genre_priority` rank then first-seen → `Unknown`). `resolve_album` is the DB-driven entry point; raw `track_genres` are read but never mutated (the §5.2 decoupling).
- **Genre vocabulary (spec §16.4, now settled):** empty and user-built. Conservatory ships no default alias map or priority list; the schema can seed one (beets `lastgenre` or MusicBrainz) later without a migration.
- **DB + CLI:** `album_track_genres` reads an album's per-track genres; `debug-shelf-genre <db>` derives and compares against the stored value (the headless usable artifact).

## v0.0.4

Phase 2a shipped: the path-template engine that renders the on-disk tree from the database.

- **Path-template engine (`src/path_template.rs`):** `PathTemplate::parse` validates a template (unbalanced braces, unknown tokens, malformed format specs are errors); `render(&TrackFields)` is infallible once parsed. The default music template renders `{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}`. An album resolves to one path; compilations bucket under Various Artists (spec §5.1). Per-field fallbacks keep structural folders non-empty; optional pieces (year, track, disc) collapse with their surrounding literals.
- **Sanitization (docs/path-template.md):** per-component path-separator replacement, reserved-device-name escaping, trailing dot/space trimming, whitespace collapse, and a per-component byte cap. Raw tags never reach the filesystem; the embedded tag keeps the true value (spec §5.5).
- **Collision detection:** `find_collisions` groups tracks that render to the same target, for the Phase 2c mover to refuse or disambiguate.
- **DB + CLI:** `track_render_rows` joins tracks with their album/artist context; `debug-paths <db>` renders a whole library and reports collisions (the headless usable artifact).

## v0.0.3

Phase 1c shipped: the engine can read a real audio file.

- **Tag reader (`src/tags.rs`):** `read_track` reads embedded tags and audio properties into a `TrackDraft` (title, artists, album, track/disc numbers and totals, year, raw multi-value genres, ReplayGain, format, bitrate, sample rate, duration, embedded cover). Raw genres are kept verbatim, decoupled from the eventual shelf genre (spec §5.2). Built on `lofty`, signed off over `symphonia` (spec §7.1) so one library also serves the Phase 5b write-back.
- **Cover accent (`src/accent.rs`):** `compute_accent` decodes a cover and derives a packed-RGB accent via a median-cut quantizer ranked by vibrancy, a faithful port of Hermitage (spec §7.4, docs/accent.md). `find_cover_bytes` prefers the embedded picture, falling back to a sibling cover file. `image` signed off with jpeg + png features.
- **CLI:** `debug-tags <file>` reads a file into a draft and prints it with the accent (the headless usable artifact).
- **Tests + fixtures:** per-format integration tests (flac/mp3/opus/m4a) over the first committed binary fixtures in the workspace, plus deterministic accent unit tests over in-memory covers. Fixtures regenerate via the `gen_audio_fixtures` example (ffmpeg + lofty); CI reads the committed files and stays hermetic.

## v0.0.2

Phase 1a + 1b shipped, and the workspace restructured around compile-time plugins.

- **Phase 1a, the writer:** single-writer SQLite worker (panic-catch-and-restart loop, per-op `oneshot` replies, ported in shape from `belfry-core` / Viaduct), read-only connection pool, WAL + pragma discipline, and the numbered `user_version` migration runner. CI (fmt / clippy / test) landed with it.
- **Phase 1b, the music data model:** migration `0001` (artists / albums / tracks / genres / `track_genres` / `genre_aliases` / `genre_priority`, plus `track_fts` and `album_fts` with sync triggers), the domain models, insert and read helpers, a synthetic fixture builder, and the `debug-roundtrip` / `debug-fixture` CLI smoke verbs.
- **Plugin restructure (spec §2.2, §16.13):** music is the native program; podcasts and audiobooks become compile-time plugins. New feature-gated workspace crates `conservatory-podcasts` (filled at Phase 6) and `conservatory-audiobooks` (filled at Phase 7), stubbed now so the wiring exists from day one. Both binaries gain `podcasts` / `audiobooks` features (default on) and report their enabled plugin set; CI gains a music-only (`--no-default-features`) job. The boundary rule: plugins are code and dependencies, not the database; all schema stays in core's single migration ledger and the unified queue stays a core commitment.

## v0.0.1

First commit. Project bootstrapped out of the design spec.

- Cargo workspace with the four planned crates (`conservatory-core`, `conservatory-search`, `conservatory-cli`, `conservatory`), all building as dependency-light skeletons.
- Portfolio scaffolding: `README.md`, `roadmap.md`, this file, `CLAUDE.md`, `ATTRIBUTIONS.md`, `VERSION`, GPL-3.0-or-later `LICENSE`, `.gitignore`, and a Meson packaging stub.
- Build deferral lifted. The spec previously parked the build behind an Atrium shipping milestone; that decision was reversed and the build now proceeds concurrently with Atrium, with hard phasing as the mitigation (spec §16.1, §17).
- Belfry retirement remains gated on podcast parity (Phase 6); nothing in Belfry has been removed.
