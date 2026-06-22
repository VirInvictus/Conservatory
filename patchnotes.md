# Patch Notes

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
