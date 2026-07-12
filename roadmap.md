# Conservatory Roadmap

Phasing is deliberate and hard (spec §17): each stage must be usable on its own, so attention can swing back to Atrium between phases without leaving Conservatory half-built. The manager half (Phases 1 to 3) must be usable before the player half is finished, the player must be usable before podcasts arrive, and audiobooks (Phase 7) come last because they lean on the podcast engine.

Each top-level phase is split into independently shippable sub-phases (the way the Atrium and Viaduct roadmaps actually grew). A sub-phase carries its own checklist, a `Tests:` line, and a *usable artifact* exit condition: the thing that must work before the sub-phase is called done. Provenance (what each piece is ported or modeled from) is noted inline rather than in a separate section.

## Version milestones

A `0.x.0` / `x.0.0` is a **capability milestone**: a cluster of phases delivering a nameable new tier. Patch releases are the sub-phases within it (the reason the version stayed `0.1.x` through Phases 16, 16.5, and 17: those were all the first-release capability tier maturing). `1.0.0` is the intended scope of `spec.md`, verified on a real library and installable via Flatpak; per semver it means "the major features intended, reliable enough for general release," not "every idea." The post-1.0 tiers pick up the doors the spec deliberately left open (§16.3 genre-tree, §16.5 MusicBrainz tagging, §16.10 audiobook metadata, §16.11 chapterize) plus the researched gaps against MusicBee / Calibre (online metadata fetching, format conversion, multiple libraries).

| Version | Milestone | Phase(s) | Status |
|---|---|---|---|
| `0.1.0` | First release: manager + player + podcasts + audiobooks + maintenance | 1–15 | ✅ tagged |
| `0.1.x` | Power-user interaction, UX completeness, player table-stakes | 16, 16.5, 17 | ✅ (through v0.1.26) |
| **`0.2.0`** | **Grammar & columns** | 18 | ✅ tagged |
| **`0.3.0`** | **Hyprland-native design (de-adwaita)** | 26 (+ the Phase 25 audits as its verification tail) | ✅ tagged |
| **`0.4.0`** | **Immersive & history** | 19 + 9 | in progress (9a v0.3.1, 9b v0.3.2 shipped) |
| **`1.0.0`** | **Verified & packaged** (the endgame) | 20 | planned |
| `1.1.0` | Metadata intelligence | 21 | committed, beyond 1.0 |
| `1.2.0` | Curation depth | 22 | committed, beyond 1.0 |
| `1.3.0` | Library operations | 23 | committed, beyond 1.0 |
| `2.0.0` | Long-form & background | 24 | committed, beyond 1.0 |
| `2.1.0` | Compact mode (the mini-player, what remains of Phase 25) | 25 | committed, beyond 1.0 |

Re-sequencing note (Brandon, 2026-07-10): with the Colophon pilot shipped (its Phase 6, v2.0.0, 2026-07-10), Phase 26 is pulled forward as the next milestone rather than waiting behind the 1.0 runway; versions track what actually ships, in order, so de-adwaita is `0.3.0` and the former `0.3.0` cluster shifts to `0.4.0`. Phase numbers stay stable. Phase 25's audit items fold into Phase 26 as its verification tail (auditing the adwaita shell separately first would mean doing the work twice); the compact mini-player, Phase 25's one genuinely new surface, stays deferred as its own phase at `2.1.0`.

The detail for each lands in its phase section below; `1.0.0` is the honest release target, and the `1.x` / `2.0` tiers are what make it category-leading afterward. Nothing here changes the spec's §14 "out of scope, forever" lines (recommendations, social, DRM, video, Windows/macOS, out-Picard-ing Picard).

## Continuous (every phase)

These run alongside every phase rather than belonging to one; called out here so they are not forgotten.

- [x] CI: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` on Linux, green from Phase 1 on. (Workflow landed at 1a: `.github/workflows/ci.yml`.)
- [x] **Observability:** a tracing subscriber in both binaries (v0.0.38). The crates emit `tracing` events (worker, player engine, podcast fetch/refresh); without an installed subscriber they were silent no-ops, which is why the player ran with no diagnostics. The GUI defaults to `info` and takes a `--debug` flag (raises our crates to `debug`: the player load / advance / buffering transitions); the CLI honours `RUST_LOG`. The Atrium / Viaduct pattern. New `tracing` log lines land with the subsystems they cover.
- [ ] Memory checkpoint per phase: a `heaptrack` / `massif` note against the spec §13 budgets; features that miss budget get gated. The harness is ported from Viaduct's `mem_check` binary (synthetic corpus, reads `VmHWM` from `/proc/self/status`, reports pass/fail against the ceiling) and grows new checkpoints as subsystems land.
- [ ] Docs kept in step (`docs/`): schema, keymap, path-template reference, genre-normalization notes, libmpv profile reference. Each lands with the sub-phase that creates its subject.
- [ ] `ATTRIBUTIONS.md` updated as each dependency is signed off and added (spec §11), and as each ported pattern lands.

## Phase 0 — Design and bootstrap ✅

- [x] Design contract (`spec.md`).
- [x] Workspace skeleton: four crates, portfolio docs, GPL-3 license, build files.
- [x] Build deferral lifted; build proceeds concurrently with Atrium (spec §16.1, §17).

## Phase 0.5 — Plugin restructure (v0.0.2) ✅

Music is the native program; podcasts and audiobooks become **compile-time plugins**: feature-gated workspace crates, on by default, internal-only API (spec §2.2, §16.13). Landed between Phases 1b and 1c, while zero podcast/audiobook code existed, so it was a workspace/docs change rather than a code migration.

- [x] Stub plugin crates `conservatory-podcasts` (filled at Phase 6) and `conservatory-audiobooks` (filled at Phase 7), so the feature wiring exists from day one.
- [x] `podcasts` / `audiobooks` features on both binaries (default on); bare invocation prints the enabled plugin set.
- [x] CI: a `music-only` job (`--no-default-features` clippy + test on both binaries) keeps the music-only build green forever.
- [x] The boundary rule recorded (spec §2.2, §4): plugins are code and dependencies, not the database. All schema stays in core's single append-only migration ledger; the unified queue stays a core commitment.

---

## Phase 1 — `conservatory-core` foundation

The headless engine's spine. Nothing user-facing ships here, but every later phase rests on it, and the CLI smoke tests make it exercisable from 1a on.

### Phase 1a — The writer ✅

- [x] Single-writer SQLite worker: a dedicated thread owns the one writable `rusqlite::Connection`; the rest of the engine holds a `Sender<Command>` and never touches the connection directly (spec §2.1). Port the structure from `belfry-core` / Viaduct's `database/worker.rs`: a panic-catch-and-restart loop around the receive loop, and a per-op `oneshot` reply so callers await their own result.
- [x] Read-only connection pool: a small fixed pool (Viaduct runs 3) of `SQLITE_OPEN_READ_ONLY` connections with a `busy_timeout`, so timeline reads / searches / counts never queue behind a long write. Spawned after the writer has initialized the file. (Ships read-only + `busy_timeout` behind the `ReadPool` abstraction; opens a fresh handle per call, with the persistent fixed-ring deferred as a post-profiling tuning per the Belfry precedent.)
- [x] PRAGMAs: WAL, `foreign_keys = ON`, `synchronous = NORMAL`, `journal_size_limit`, a bounded `mmap_size` (spec §4). (Also `temp_store = MEMORY`.)
- [x] Migrations versioned via `user_version`, append-only and backwards-compatible post-1.0. This is the **Atrium discipline, deliberately not Viaduct's**: Viaduct sets its schema up with `CREATE TABLE IF NOT EXISTS` plus ad-hoc `ALTER TABLE`, which Conservatory does not adopt; a numbered-migration ledger is the contract here because the library is the user's irreplaceable data, not a regenerable feed cache. (Runner machinery only; the registry is empty until 1b ships migration `0001`, so `user_version` is 0 at 1a.)
- [x] Fixtures: a synthetic library builder for tests (small, deterministic, no real audio needed for the DB layer). (Deferred to 1b and delivered there: `db::fixtures` with small/medium/large scales, surfaced as the `debug-fixture` CLI verb; 1a tests used a debug `_probe` table and synthetic inline migrations.)
- [x] Tests: first integration suite. Open, migrate, write-through-worker, read-from-pool round-trip; a migration-from-vN fixture; a writer-panic-restart test.

*Usable artifact:* `conservatory-cli` (a debug verb is fine) opens the DB, applies migrations, and round-trips a row through the worker and read pool.

### Phase 1b — Music data model + FTS5 scaffolding ✅

- [x] Schema: `artists`, `albums`, `tracks`, `genres`, `track_genres`, `genre_aliases`, `genre_priority` (spec §4.1), as the first numbered migration set. (Migration `0001_initial.sql`; the runner owns `user_version`, so the file is pure DDL.)
- [x] FTS5: `track_fts` (title, artist, album) and `album_fts` (title, album artist), kept in sync by triggers (spec §4.4). Pattern follows Viaduct's trigger-synced virtual table. (Ordinary, not external-content, tables: the denormalized artist/album columns are looked up by the triggers, including on artist/album rename.)
- [x] Read helpers and the `Track` / `Album` / `Artist` models the CLI and later the GTK side consume. (`db::models` + `db::reads`: counts, lookups, `list_albums`; inserts go through the worker, `db::writes`.)
- [x] Tests: schema migration test; FTS trigger sync (insert/update/delete keeps the index correct); read-helper counts against a fixture.

*Usable artifact:* a fixture library loads into the schema; counts and basic lookups verify through the read pool. (Shipped as `conservatory-cli debug-fixture <db> --scale small|medium|large`.)

### Phase 1c — Tag read + cover/accent ✅

- [x] Dependency sign-off: `lofty` over `symphonia` for tag read (spec §7.1, §11; ATTRIBUTIONS.md). `lofty` (read + write, broad coverage) serves both the 1c read and the 5b write-back; `symphonia` is decode-only and would need a second crate for writes.
- [x] Tag reader: read embedded tags into the import draft (title, artists, album, track/disc no + totals, year, raw multi-value genres, ReplayGain, format/bitrate/sample-rate, duration). `src/tags.rs`: `read_track` → `TrackDraft`. Raw genres kept verbatim (the §5.2 decoupling); splitting is Phase 2b's job.
- [x] Cover art + accent: extract embedded cover or locate a sibling cover file, and compute the accent via a median-cut quantizer (`albums.accent_rgb` is populated at Phase 2 import; 1c computes the value headless). `src/accent.rs`, ported from Hermitage (docs/accent.md, spec §7.4). `image` signed off with jpeg + png features (webp deferred).
- [x] Tests: read committed per-format fixtures (flac/mp3/opus/m4a) into a draft (`tests/tags.rs`); accent extraction is deterministic against synthesized covers (`src/accent.rs` unit tests). Fixtures regenerable via `examples/gen_audio_fixtures.rs` (ffmpeg + lofty), CI stays hermetic.

*Usable artifact:* `conservatory-cli debug-tags <file>` reads a real audio file into a populated draft with an accent colour, headless.

---

## Phase 2 — Import and organize

The manager becomes usable headless here. This is the phase that earns the trust commitment in spec §5, so the file mover (2c) is the load-bearing sub-phase and gets the heaviest test suite.

### Phase 2a — Path-template engine ✅

- [x] Template tokenizer/renderer for the path-template string (spec §5.1); the default `{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}`. `src/path_template.rs`: `PathTemplate::parse` (validates braces/tokens/specs) + infallible `render(&TrackFields)`.
- [x] An album resolves to exactly one path (one shelf genre, one album artist) even when track-level tags disagree; compilations resolve the album-artist component to a **Various Artists** bucket. Per-field fallbacks keep structural folders non-empty (`Unknown` / `Various Artists` / `Unknown Album` / `Untitled`); optional pieces (year, track, disc) collapse with their literals.
- [x] Filesystem-safe rendering: per-component sanitization (path separators → `_`, reserved device names, trailing dot/space, byte cap, whitespace collapse) without leaking raw tags onto disk (docs/path-template.md).
- [x] Tests: 17 unit tests (rendering across field combinations, sanitization edge cases, Various Artists bucketing, missing-field fallbacks, parse errors, collisions) + a `track_render_rows` integration test (`tests/paths.rs`).

*Usable artifact:* `conservatory-cli debug-paths <db>` renders correct target paths for a fixture library and reports collisions. The engine is pure (`find_collisions` exposes batch collision detection for the Phase 2c mover).

### Phase 2b — Shelf-genre resolver ✅

- [x] Normalization layer: split on `;` `/` `,`, case-fold for matching, map through `genre_aliases` (spec §5.2). §16.4 settled: **empty and user-built** (no default vocabulary; the schema can seed one later without a migration). `src/shelf_genre.rs`: `normalize` keeps canonical/original casing in the output.
- [x] Priority chain: manual override → single album-level tag → most-common normalized track genre (ties broken by `genre_priority` rank, then first-seen) → `Unknown` bucket. `resolve_shelf_genre` (pure) + `resolve_album` (DB-driven).
- [x] `shelf_genre` is the only input to the genre folder level; raw `track_genres` are read but never touched (the decoupling in spec §5.2 and the CLAUDE.md hard rule).
- [x] Tests: 10 unit tests (agreeing/disagreeing/absent genres, alias-map application, priority + first-seen tie-breaks, multi-value-in-one-tag split) + a fixture-backed integration test (`tests/shelf_genre.rs`).

*Usable artifact:* `conservatory-cli debug-shelf-genre <db>` derives a stable `shelf_genre` for every album in a fixture library (matches the stored value).

### Phase 2c — File mover (dry-run + undo journal + crash-safe replay) ✅

The headline risk (spec §5.4, CLAUDE.md hard rule). Release-blocking, not nice-to-have.

- [x] Dry-run preview: `mover::plan` is pure (reads the filesystem, changes nothing); reports the operations that would run, conflicts, and in-place skips.
- [x] Undo journal written **before** the move (SQLite migration `0002`, durable via WAL) and replayed on restart (`mover::recover`, roll-forward, idempotent). Every relocation is a reversible job; `mover::undo` reverts a completed job. The crash-safety ordering is documented in docs/mover.md.
- [x] Conflict handling: duplicate target paths, missing sources, existing destinations all refuse the job (no overwrite). Cross-filesystem moves use copy → fsync → verify → delete (`fsops::relocate`). Partial-batch failure is covered by the journal + roll-forward replay.
- [x] Copy vs move is a per-job choice (`MoveMode`; copy leaves originals, move consumes them; undo deletes the copy vs moves the file back).
- [x] Tests: `tests/mover.rs` (move/undo round-trip leaves tree + DB consistent; simulated mid-move crash rolls forward cleanly; conflict refusal; copy mode; tree↔DB consistency as a §5.6 spot-check) + `fsops` unit tests (rename fast path, idempotent replay, cross-fs copy path). Full §5.6 re-import coverage lands with Phase 2d/5b.

*Usable artifact:* `conservatory-cli debug-organize <db> <root> [--apply] [--undo <id>]` plans, applies, and fully undoes a real move job; a crash between any two steps leaves a recoverable state (roll-forward on next run).

### Phase 2d — Import pipeline + CLI verbs ✅

- [x] Wire the pipeline: scan/drop → read tags (1c) → resolve into DB → derive `shelf_genre` (2b) → render target (2a) → move/copy (2c). `src/import/` (scan + resolve + two-pass pipeline; resolve in memory then a conflict pre-check before any DB write, so a refused import changes nothing). Accent (1c) computed + stored per album.
- [x] CLI: `import` (copy default, `--move`), `organize` (re-render from DB; dry-run/`--apply`/`--undo`), `shelf-genre-set` (spec §9). Output `--tsv` (default) / `--json` / `--human`. (`tag set` deferred to the Phase 5a editor; `--json` is a compact numeric summary until serde is signed off.)
- [x] Tests: `tests/import.rs` (end-to-end import of the committed per-format fixtures into a managed tree; copy keeps sources / move consumes them; re-import refused and DB unchanged; `shelf-genre-set` then `organize` moves the album) + the resolver/scan unit tests. Verified by hand against two real albums (mp3 + opus) from the library.

*Usable artifact:* **the manager is usable headless.** `conservatory-cli import <db> <folder> <root>` gives an organized, database-owned library; `organize`/`shelf-genre-set` re-shelve it.

---

## Phase 3 — GTK browse

A working library browser. The search crate (3a) is headless and could in principle ship before the GUI; it sits here because the browse surface is its first real consumer.

### Phase 3a — `conservatory-search` crate ✅

- [x] Grammar pipeline ported in *shape* from `atrium-search`, implemented independently (spec §3.4, ATTRIBUTIONS.md): `lex` → `parse` (typed AST + extracted `sort:` specs) → `ast` (round-trippable `Expr`) → `eval` (in-memory fallback) and `sql_translate` (all-or-nothing dual path: emit SQL only if every node maps, else fall back). Storage-agnostic (`SqlValue`, no rusqlite); deps `regex` + `chrono` only.
- [x] Domain semantics modeled on CalibreQuarry: datatype-dispatched matching, multi-valued `genre:` faceting, numeric relops (`rating:>=4`, `bitrate:`, `duration:`), date keywords + precision + ranges (`added:thisweek`, `year:1998..2004`), `true`/`false` presence. Typed against the Track domain (album/artist exposed through it; podcast/audiobook at 6/7).
- [x] Fields per spec §3.4: `artist:`/`albumartist:`/`album:`/`title:`, `genre:` vs `shelfgenre:`, `year:`/`added:`, `rating:`/`bitrate:`/`duration:`/`format:`, `is:played`/`is:starred`/`is:queued` (`is:queued` matches nothing until the queue table lands at 4b). Podcast/audiobook fields degrade to substring until 6/7.
- [x] Match modifiers (substring / `=`exact / `~`regex / `?`fuzzy Damerau-Levenshtein), boolean `AND`/`OR`/`NOT` + implicit AND + `!`, comparison/range, `sort:`/`sort:-` lifted to result metadata.
- [x] **Forgiving parser:** never errors; unknown fields/states/sorts and structural failures (unbalanced parens) degrade to substring + a warning.
- [x] Ranking: bare-text hits ordered by FTS5 `bm25` blended with recency (`rank::blend_relevance`) over the 1b FTS tables (`fts_rank` read).
- [x] **Perspectives:** `vl:NAME` expanded at parse time via a `PerspectiveResolver` with cycle detection (forgiving: a cycle degrades to empty + warning). Persistent storage + the save/load UI land at 3c.
- [x] CLI: `search <db> '<expression>'` (`--tsv`/`--json`/`--human`). Crate is GUI/storage-agnostic and fuzzable.
- [x] Tests: parse round-trip (parse → Display → re-parse stable); **SQL-vs-eval parity** over a 2k-track fixture (`tests/search_parity.rs`); degrade-to-substring; `vl:` cycle guard; per-field eval + per-node SQL.

*Usable artifact:* `conservatory-cli search '<expr>'` filters the library with the full grammar (verified against the real imported `testdata/` albums via both the SQL and eval paths).

### Phase 3b — Columns UI faceted panes ✅

The first GTK4/libadwaita code (programmatic UI; facet logic in `conservatory-core`, the binary renders).

- [x] Hierarchical filter panes (default Genre → Album Artist → Album), the deadbeef-cui layout as a first-class window. (User-reconfigurable order / field expressions and their **persistence** deferred to the config layer, Phase 10; 3b ships the default hierarchy.)
- [x] Multi-value faceting: a track tagged `Electronic; Ambient` appears under both Genre rows (joins `track_genres`), while single-valued shelf genre drives the filesystem (the §5.2 decoupling). `core::db::facets`.
- [x] Memoized per-facet track counts; an `[All (N)]` synthetic row tops each pane (selecting it clears that pane's constraint).
- [x] Debounced selection-change before downstream recompute (the deadbeef invariant), via the ported coalescing queue; the cascade recomputes only panes downstream of the changed one + the leaf.
- [x] Coalescing-delta plumbing: ported Viaduct's `CoalescingQueue` (main-thread, interval + max-interval flush, dedup) and used it for the selection debounce. (`BatchUpdate` and live `LibraryChanges` delivery deferred until there is an in-GUI writer, Phase 5a.)
- [x] Tests: facet counts + cascade + multi-value genre against a fixture (`core/tests/facets.rs`, headless); coalescing burst → single flush (`ui::coalescing` test, headless glib). GTK widgets verified by build + manual launch.

*Usable artifact:* `conservatory <db>` launches the browse window; facet selection narrows downstream panes and the leaf list. (`conservatory-cli debug-facets <db>` exercises the same logic headless.)

### Phase 3c — Track list + Perspectives UI ✅

- [x] The leaf track list: sortable columns, multi-select (Ctrl/Shift), row affordances (rating stars, hover lift) shared with the future episode list. Sorting delegates to a pure `core::cmp_tracks`/`sort_tracks` (the GTK `CustomSorter` and the headless comparator share it); rating renders as accent-tinted symbolic stars; `TrackBrief` gained a name-ordered `genres` roll-up + `rating`. (The per-row playing/status glyph waits for playback state, Phase 4.)
- [x] Filter bar wired to `conservatory-search`; `Ctrl+F` focuses it; no separate search mode (spec §3.4). Always-on `SearchEntry`; the facet set and the grammar intersect on the leaf; malformed input degrades to substring + a yellow tint. The composition lives in a non-GTK `query.rs` (headless-tested), keeping core runtime-search-free (the `conservatory-search` dep stays consumer-side by design).
- [x] Perspectives surfaced in the UI: save, name, reload (re-parsed from text). Migration `0003` adds the core-owned `perspectives` table; the sidebar lists Default + saved searches; `vl:NAME` resolves from storage. Saves/deletes go through the **single-writer worker, now stood up in the GUI** on a tokio runtime (the in-GUI writer, pulled forward from Phase 5a to back persistence).
- [x] Tests: sort comparator (case-fold, stable tie-breaks, numeric keys); filter-bar facet∩grammar composition + `vl:` round-trip (binary, headless); Perspective CRUD + `vl:` resolution (core).

*Usable artifact:* **a working library browser.** Browse, filter, sort, and save Perspectives over the managed library.

---

## Phase 4 — Playback

A daily-driver music player. Profile switching at album/kind boundaries (spec §16.9) is the prototyping risk; tackle it in 4b where the unified queue makes it concrete.

### Phase 4a — libmpv host + music profile ✅

- [x] Dependency sign-off: `libmpv2` (spec §11; ATTRIBUTIONS.md) and the system `libmpv` (0.36+) requirement. `libmpv2 4.1` pulled into `conservatory-core` (the player lives in core, spec §16.13); `libmpv-dev` added to both CI jobs.
- [x] A single libmpv instance kept alive across items (`player::host::MpvHost`, property API + the input-command layer, spec §6). The threaded `Player` handle + command channel are deferred to 4b, where the GTK Now-bar is the second consumer; 4a drives the host directly from the CLI loop (no speculative plumbing).
- [x] Music profile (`player::profile`, pure + tested): gapless within an album (`gapless-audio`), ReplayGain via mpv's native `replaygain` property (mpv reads the file tags `lofty` stored), with the DB `replaygain_*` columns driving mode resolution (album→track→off downgrade by available tags). Crossfade is carried through (config field) but rendered at 4b with the queue (a between-tracks behaviour). *(Later dropped at Phase 5.5a: true crossfade is impossible in a single libmpv instance; the field is removed.)* **§16.7 deferred:** read-only ReplayGain, no in-app scan. **§16.6 deferred:** no EQ/DSP in 4a.
- [x] State persistence (`player::state`, pure + tested): position written on the insurance interval (30 s) and on the forced points (pause/seek/item-end/quit), through the single-writer worker into the new singleton `playback_state` table (migration `0004`); `play_count` + `last_played` bumped on a natural end-of-file only (`EndReason::Eof`).
- [x] Tests: profile resolution + ReplayGain downgrade (8 unit tests); state-write debounce + only-Eof-counts (4 unit tests); `playback_state` round-trip + play-count increment through the worker, and an `ao=null` libmpv smoke test that decodes a committed fixture to EOF (`tests/playback.rs`).

*Usable artifact:* `conservatory-cli play <db> [track_id]` plays a track from the managed library with gapless + ReplayGain through libmpv; the position is persisted on the insurance interval and `play <db>` (no id) resumes the saved cursor across a restart. The threaded engine, the unified queue, and the Now-bar land at 4b.

### Phase 4b — Unified queue + Now-bar

Split into two shippable sub-phases: **4b-i** lands the queue + the threaded engine headless (CLI-testable, the hard rule); **4b-ii** is the GTK Now-bar + queue view that consumes them.

#### Phase 4b-i — Unified queue + threaded Player (headless) ✅

- [x] `queue` table (migration `0005`, spec §4.3) → in-memory `Vec<PlayableItem>`; the full column set lands (the unified queue is a core commitment) but only `track_id` carries a foreign key — `foreign_keys = ON` refuses any DML on a child whose parent table is absent, so the `episode_id`/`book_id` FKs are added when `episodes`/`books` land (Phases 6/7) via a table rebuild. Positions stay contiguous `0..n-1`, renumbered transactionally on the single writer (`enqueue`/`remove`/`reorder`/`clear`/`replace`).
- [x] The threaded `Player`: a dedicated `std::thread` owns the `!Send` `MpvHost` (built there via a `make_host` factory), behind a `Send + Clone` `PlayerHandle` (command channel out, `Arc<Mutex<PlayerSnapshot>>` polled back). The 4a CLI pump-loop is lifted into `player::engine`. `PlayableItem { track_id, source, profile, album_id, kind }`.
- [x] On advance, apply the item's profile before playing (per-item `host.load`, the spec §16.9 boundary switch with the music profile). Advance only on natural `Eof`; an errored item skips; self-initiated `Stop`/`Redirect` (from our own load) do not advance. Persistence split (spec §6.4): debounced ticks fire-and-forget, terminal writes (pause/seek/stop/shutdown/final-EOF play-count) block on the worker so they land. *Audible within-album gaplessness (mpv playlist append) is deferred to 4b-ii, where it is verified by ear.*
- [x] `is:queued` wired up (was inert since 3a): `sql_translate` emits a `queue` subquery on the SQL fast path; the eval path reads `SearchRow.queued` (an `EXISTS` against the queue), populated in `search_rows`.
- [x] CLI: `queue add|list|remove|clear`; `play <db> <root> [track_id]` drives the engine through the queue (root resolves the relative `file_path`s), polling the snapshot until the queue ends.
- [x] Tests: queue position integrity (enqueue/remove/reorder stay contiguous); `is:queued` membership; the engine plays a queue of imported fixtures to its end through a null audio output, incrementing each play count and landing the cursor on the last item (`tests/queue.rs`).

*Usable artifact:* `conservatory-cli queue add` / `play <db> <root>` builds and plays a queue headlessly; the engine advances item to item applying each profile, persists position + play counts, and resumes from the saved cursor.

The GTK half is itself sizable, so it splits again: **4b-ii-a** makes the window play (engine + Now-bar + transport); **4b-ii-b** adds the visible queue panel and drag-and-drop reorder.

#### Phase 4b-ii-a — Player engine in the GUI + Now-bar transport ✅

- [x] The threaded `Player` stood up in the browse window (`player::spawn(worker.clone(), rt.handle())` on the existing in-GUI runtime); a libmpv init failure leaves it unset and the transport inert (browse still works). The window holds the `PlayerHandle`, the snapshot poll source, the playing queue's id→label map, and the library root.
- [x] Persistent bottom **Now-bar** (`now_bar.rs`, attached via `ToolbarView::add_bottom_bar`): title/artist, prev / play-pause / next (symbolic glyphs), position label + seek `Scale` (driven by `change-value`, so the refresh's programmatic `set_value` never loops), and a volume `ScaleButton`. Buttons are non-blocking `PlayerHandle` sends.
- [x] **Double-click / Enter plays the visible leaf list from that row** (the deadbeef idiom, spec §3.6): the selection model's display order is the queue, the activated index is the start. The id list + a `Track` batch-read (`get_tracks`) feed a pure `playqueue::build_play_queue` (order preserved, `source` = root-joined, profile resolved, start re-indexed past any vanished track).
- [x] A 250 ms `glib::timeout_add_local` polls the snapshot → `refresh_now_bar` (position/seek/icon every tick; title/artist only on track change). Clean teardown on `close-request`: remove the timer, then `player.shutdown()` (joins the engine thread; its terminal flush still has the worker), then worker/runtime drop.
- [x] The library root arrives as an optional second CLI arg (`conservatory <db> [root]`); Phase 10 config replaces it.
- [x] Tests: `build_play_queue` (order, root-join, start re-index, missing tracks) + time formatting unit tests; `get_tracks` cross-chunk worker test. The widgets are verified by build + manual launch (the 3b/3c precedent).

*Usable artifact:* `conservatory <db> <root>` — double-click a track to play the visible list from there, with a working Now-bar transport (play/pause, prev/next, seek, volume) that reflects state.

#### Phase 4b-ii-b — Queue view + drag-and-drop reorder (GTK) ✅

- [x] A right-side slide-in queue drawer (`queue_panel.rs`, a `gtk::Revealer`, header toggle + `Ctrl+U`): a `ListView` of `QueueRow` (kind icon + title/artist), the playing row accent-highlighted, **drag-and-drop reorderable** (the Atrium idiom: the row carries its position, the `DropTarget` computes Above/Below from cursor Y, controllers torn down in `unbind`), plus keyboard `Alt+↑/↓` reorder, `Delete`, `Ctrl+Shift+C`.
- [x] The engine gained **in-place mutation** (`MoveItem`/`RemoveItem`/`ClearQueue`) so editing the live queue never restarts the current track; the `current_index` math is pure + unit-tested (`move_current_index`/`remove_current_index`). The GUI applies the identical `(from, to)` to `worker.reorder_queue` and `player.move_item`, so DB position == engine index stays invariant; double-click now **writes the DB queue through** (`replace_queue_with_tracks`) so the drawer reflects the spec §4.3 source of truth.
- [x] Core read `load_queue_display` (queue ⋈ tracks ⋈ artists) backs the drawer; the highlight follows playback via the 250 ms snapshot poll.
- [x] Tests: the index helpers (8), `drop_target_position` (Above/Below, up/down, clamp), an engine null-host integration test (move/remove track `current_index` without restarting), and a `load_queue_display` worker test. Widgets verified by build + manual launch.

*Usable artifact:* build and play a queue in the GUI; open the drawer and reorder it by drag (or keyboard); the playing row is highlighted and the Now-bar reflects state.

#### Phase 4b-ii-c — Queue polish (GTK) ✅

- [x] Launch-resume: on GUI startup `resume_saved_queue` loads the saved DB queue into the engine **paused at the cursor** (a new `paused` flag on the engine's `SetQueue`, exposed as `PlayerHandle::resume`), so reopening the app is silent until play.
- [x] `Ctrl+Enter` appends the browse selection to the queue (DB tail via `enqueue_tracks` + live engine tail via the new `AppendItems` command, which starts playing if the queue was idle); plain Enter / double-click still replaces.
- [x] Tests: engine null-host integration — append-to-idle starts playing, a second append extends the tail, and a fresh engine resumes the whole queue paused at the cursor.
- [ ] **Deferred:** a cover thumbnail in the Now-bar (blocked: `albums.cover_path` is unpopulated until cover-to-disk lands, spec §7.4); the audible within-album gapless prototype (mpv internal playlist append, spec §16.9); the `playback_state` explicit queue-entry reference; the library root sourced from config (Phase 10) rather than a CLI arg.

*Usable artifact:* reopen the app and pick up where you left off (paused at the cursor); `Ctrl+Enter` appends the selection to a playing queue.

### Phase 4c — System integration

Split: **4c-i** is the D-Bus half (MPRIS2 + the suspend inhibitor, on `zbus`); **4c-ii** is the audio-output-device picker (an mpv-property + GUI-menu mechanism, no D-Bus).

#### Phase 4c-i — MPRIS2 + suspend inhibitor ✅

- [x] `conservatory-core/src/mpris.rs` serves `org.mpris.MediaPlayer2` + `…Player` on the session bus via `zbus 5` (signed off, ATTRIBUTIONS.md): metadata, `PlaybackStatus`, `Position`, `Volume`, `CanGoNext/Previous`, and `Play`/`Pause`/`PlayPause`/`Next`/`Previous`/`Stop`/`Seek`/`SetPosition` driving the `PlayerHandle`. `run(player, pool)` polls the snapshot (~300 ms), emits `PropertiesChanged` on change, and resolves the current track's metadata via a new `track_metadata` read. The GUI spawns it on its runtime; **media keys, the GNOME overlay, and the lock screen come for free.**
- [x] Suspend inhibitor: a logind `org.freedesktop.login1.Manager.Inhibit("sleep", …, "block")` proxy on the system bus, the FD held while playing and dropped on pause/stop (best-effort: a missing system bus doesn't break MPRIS).
- [x] Tests: pure mapping helpers (`playback_status`, `can_go_next/previous`, `wants_inhibit`, volume/position conversions, `metadata_fields`) + a `track_metadata` worker test. Live D-Bus serving + the logind inhibit are verified manually (`playerctl`, `systemd-inhibit --list`), the build-plus-manual precedent.

*Usable artifact:* `playerctl` and the keyboard media keys drive playback; the GNOME media overlay/lock screen show the track; the machine won't suspend mid-track.

#### Phase 4c-ii — Output-sink picker (GTK) ✅

- [x] `MpvHost::audio_devices()` parses mpv's `audio-device-list` (node → `AudioDevice { name, description }`) and `set_audio_device()` sets the `audio-device` property; the engine queries the list once at init and exposes it (plus the current selection) on the snapshot; a `SetAudioDevice` command applies a switch.
- [x] A header `MenuButton` (`set_create_popup_func`, built fresh on open from the snapshot) lists the sinks (the current one checked) and switches output on click (spec §6.5).
- [x] Tests: a host integration test (`audio_devices()` includes `auto`; `set_audio_device("auto")` succeeds); the menu is verified by build + manual launch.

*Usable artifact:* **a daily-driver music player.** It replaces deadbeef for the managed library, with full system media integration and output-device selection.

---

## Phase 5 — Bulk editing + embedded write-back

### Phase 5a — Bulk metadata editing

Split headless-first (the CLI-testable rule): **5a-i** is the editing logic + worker commands + `tag` CLI verb; **5a-ii** is the GTK bulk-edit dialog. Cover editing is Phase 5d (`cover_path` is unpopulated until then); embedded write-back is Phase 5b.

#### Phase 5a-i — Field editing + path-affecting move (headless, CLI) ✅

- [x] Core write commands (`db::writes` + worker): `update_track` (title / rating / track artist, get-or-create by derived sort name), `update_album` (title / year / shelf genre / album artist), `set_track_genres` (clear + re-link the raw §5.2 multi-value side). `COALESCE`-guarded so only the changed fields move; the FTS triggers re-sync on every UPDATE (verified by test).
- [x] Pure resolver (`conservatory-core/src/edit.rs`, unit-tested): parse `field=value`, classify track-level vs album-level and **path-affecting** (album / albumartist / year / shelfgenre, the default-template fields), build the typed `TrackEdit`/`AlbumEdit`, split raw genres, and literal search-and-replace. Shared by the CLI and (5a-ii) the GTK dialog.
- [x] Path-affecting edits reuse the Phase 2c mover (dry-run preview + undo), re-rendering only the touched albums (the generalized `shelf-genre-set` → `organize` flow).
- [x] CLI: `tag set <db> <selector> <field=value>... [--root] [--apply]` and `tag replace <db> <selector> <field> <find> <replace> [--root] [--apply]`, selector via `conservatory-search`.
- [x] Tests (`tests/edit.rs`, committed + synthetic fixtures): field updates re-read; FTS follows a title/artist/album/albumartist rename; `set_track_genres` replaces (not appends); a year edit re-renders, moves, and `undo` reverts DB + tree. Hand-verified against the `testdata/` albums.

*Usable artifact:* `conservatory-cli tag set <db> '<expr>' year=1992 --root <root> --apply` edits the matched library and re-shelves any moved files, fully headless and undoable.

#### Phase 5a-ii — GTK bulk-edit dialog ✅

- [x] A bulk-edit dialog (`adw::AlertDialog` with a labelled-entry grid, the Perspective-save precedent) over the leaf multi-selection, opened by a header pencil button or `Ctrl+E`; one entry per field (album artist, album, year, shelf genre, track artist, title, raw genres, rating), blank means unchanged. Reads the selection with the existing `is_selected` + `downcast::<TrackRow>` loop, parses each filled field through `core::edit::parse_assignment` (rejecting the whole set if a value is invalid), and applies via the new worker commands.
- [x] Path-affecting edits show a move **preview-and-confirm** (`mover::plan` → a "Move N files?" `AlertDialog` → `mover::apply`, `MoveKind::Organize`, scoped to the touched albums); the browse refreshes via `populate_initial` after the edit.
- [x] Search-and-replace is available headless (`tag replace`, 5a-i); the in-dialog replace mode is deferred (the per-field set covers the common case). Live incremental `LibraryChanges`/`BatchUpdate` delta delivery stays deferred (a full reload is used).
- [x] The dialog is verified by build + manual launch (the 3b/3c precedent); the underlying edit/move logic is covered by the 5a-i tests.

*Usable artifact:* select tracks in the browser, bulk-edit their fields (`Ctrl+E`), and have path-affecting edits move files safely behind a preview.

### Phase 5b — Embedded-tag write-back (§5.5)

Headless-first: **5b-i** is the core write + `embed-tags` CLI + tests; **5b-ii** is the GTK action. Only the rebuildable descriptive layer is written; the curated layer (rating, shelf genre, play counts, starred) stays DB-only (§5.6).

#### Phase 5b-i — Core write-back + `embed-tags` ✅

- [x] `tags::write_track_tags(path, &TagWrite)` (lofty write, signed off at 1c): write the format's canonical primary tag authoritatively (title, track artist + sort, album, album artist + sort, year, track/disc, raw multi-value genres), creating it if absent, dropping the legacy ID3v1. `db::writeback_rows` is the one join that supplies all of it (display + sort names + group-concat genres).
- [x] CLI `embed-tags <db> <selector> --root <root> [--apply]`: dry-run shows the per-file field diffs (current tags vs DB); `--apply` writes. Re-derivable from the DB (the source of truth), so dry-run is the safety and there is no undo journal.
- [x] Tests (`tests/writeback.rs`): per-format round-trip (edit DB → embed → re-read the file), and the **§5.6 re-import contract** (embed → fresh DB → re-import → the edited descriptive field survives). Hand-verified against the `testdata/` albums.

*Usable artifact:* `conservatory-cli embed-tags <db> '<expr>' --root <root> --apply` writes the curated metadata into the files; a wipe-and-reimport reconstructs the descriptive layer (§5.6 holds).

#### Phase 5b-ii — GUI action ✅

- [x] An "Embed metadata into files" header action (the save icon) over the leaf selection (explicit, not auto-on-edit, the Calibre model), behind a "Write tags to N file(s)?" confirm and a result dialog; writes through `write_track_tags`. Verified by build + manual launch.

> **APE-strip deferred.** The Lattice `apestrip` hygiene (strip a stray APEv2 that shadows ID3 on MP3, with optional APE→ID3 migration) is **not** in 5b: lofty reads APE on MPEG but neither writes nor removes it, so a reliable strip needs byte-level surgery (exactly why `apestrip.py` is hand-rolled). Deferred to a byte-level pass, paired with the Phase 8c "detect stray APE" audit. (`embed-tags` writes the canonical ID3v2 correctly; it just cannot remove a pre-existing APE shadow on MPEG.)

### Phase 5c — ReplayGain scan (resolves spec §16.7) ✅

Phase 4a reads ReplayGain but never scans (the §16.7 open decision); this settles it on the "scan in-app" side so the player can normalize untagged albums.

- [x] **Mechanism (settled): `rsgain`** (installed, v3.6), not the `ebur128` crate. The crate measures only decoded PCM and the pure-Rust decoder (symphonia) can't decode Opus (half the library); rsgain decodes every format itself and writes correct RG2.0 tags (incl. Opus R128 + album gain). External tool (ATTRIBUTIONS.md; bundle for Flatpak later). `conservatory-core/src/replaygain.rs`: `scan_album_files` shells `rsgain custom -a -s i -c p -l <lufs>` (the Lattice invocation); `replaygain_from_file` re-reads the written gains.
- [x] DB refresh: a `set_track_replaygain` worker command updates `tracks.replaygain_*` from the scan, so the Phase 4a profile resolution (album → track → off) sees the values unchanged.
- [x] CLI: `replaygain scan <db> <selector> --root <root> [--apply] [--target-lufs N]` (per-album grouping; dry-run lists the albums, `--apply` scans + syncs).
- [x] Tests (`tests/replaygain.rs`): the DB-sync half is hermetic (write a known tag → read → feed the profile); the rsgain scan is a skip-if-absent integration test (covers FLAC + Opus).

*Usable artifact:* `conservatory-cli replaygain scan <db> '<expr>' --root <root> --apply` gives untagged albums correct ReplayGain in-app.

### Phase 5d — Cover art to disk + cover management (spec §7.4) ✅

Implements the §7.4 covers-on-disk story that the Now-bar thumbnail and MPRIS art were deferred behind (Phases 4b-ii-c, 4c-i). The **trust-critical mover is left untouched**: covers are derived (re-extractable from embedded art), so they are synced idempotently after a move, not journaled.

- [x] `conservatory-core/src/covers.rs`: `write_cover` (sniff PNG vs JPEG), `sync_album_cover` (write into the album folder, drop a stale cover at the old location), `resync_album_covers` (ensure every album's cover is in its current folder + `cover_path` matches, after a move). Import writes each album's `cover.jpg`/`.png` and records `cover_path`; `organize` and path-affecting edits resync.
- [x] Set / replace a cover from a file: CLI `set-cover <db> <album_id> <image> --root` (writes the file, updates `cover_path`, refreshes the accent). (The in-dialog GUI cover field is deferred; the CLI verb covers it.)
- [x] Wired the unblocked consumers: the Now-bar cover thumbnail (a `gtk::Image`, loaded on track change) and MPRIS `mpris:artUrl` (`file://<root>/<cover_path>`, root passed into `mpris::run`).
- [x] Tests (`tests/covers.rs`): import writes the cover + populates `cover_path` + accent; a year edit + organize moves the cover and updates `cover_path` (old removed). The artUrl mapping is a `mpris::build_metadata` unit test; the Now-bar image is build + manual.

*Usable artifact:* albums show their cover on disk, in the Now-bar, and in the GNOME media controls.

---

## Phase 5.5 — Audio engine (DSP chain, EQ, output quality)

The music daily-driver's "feel good" phase, and a foundational refactor. Today the engine sets three mpv properties (`gapless`, `replaygain`, `audio-device`) and builds no `af` filter chain at all (`conservatory-core/src/player/`). This phase turns the flat `MusicProfile` into a **labelled `af`-chain builder** (`@rg → @eq → @comp → @boost` stages, built once per item, parameters mutated only via `af-command` so changes never tear down the graph and click the audio). That chain engine is shared infrastructure: the Phase 6c spoken-word chain (Smart Speed / Voice Boost) and the Phase 7c audiobook chain become **presets on it**, not a parallel hardcoded path, which is why it lands before the **Phase 6c spoken-word chain** (spec §17), not before all of Phase 6: the podcast manager and triage (6a/6b) are independent of the audio engine and shipped first. No new Rust dependency: every filter rides the already-linked libmpv/ffmpeg (spec §11). Resolves spec §16.6. Settled scope (spec §6.2, §6.5): a real but bounded chain (EQ + compressor/limiter/leveler) and correct output, not a deadbeef-class everything; **crossfade is dropped** (impossible in a single libmpv instance, maintainer-rejected; the dead `crossfade_seconds` key is removed), and exclusive/bit-perfect output, LADSPA/raw-`af` hosting, and crossfeed are **deferred** (recorded, not built).

### Phase 5.5a — Chain foundation + correct ReplayGain staging (headless, core) ✅ (v0.0.39)

- [x] Evolve `player/profile.rs` + `player/host.rs` from flat-field application to a **labelled `af`-chain builder** (`player/chain.rs`, `build_af_chain`): one chain per item, each stage a labelled `lavfi` filter (`@rg` now; `@eq` / `@comp` / `@boost` slots documented for 5.5b/c), set on mpv's `af` at load. (Runtime `af-command` parameter mutation lands with the first mutable stage, the EQ at 5.5b; 5.5a builds the chain fresh per item, which is gap-free across the per-item loadfile.)
- [x] **Own the ReplayGain gain stage** (fixes mpv bug #8267): `@rg:lavfi=[volume=<dB>]` at the chain *head* from the 5c-scanned `tracks.replaygain_track/_album`, recomputed per track (the host rebuilds the chain from each item's profile on load). mpv's built-in `--replaygain` is dropped (applied *after* the `af` chain; not re-applied per track across a gapless boundary). `replaygain_preamp` added; **clip-prevention is the no-peak-data gain clamp** (attenuate-only when `replaygain_clip`, the safe default) — the brick-wall limiter + peak-aware attenuation are 5.5c (the schema has no peak columns).
- [x] `--gapless-audio=weak` when gapless (preserve the source rate on a mixed-rate library), `no` for single items; `audio-samplerate`/`audio-format` left unset.
- [x] **Drop crossfade**: removed the unused `crossfade_seconds` field (`profile.rs` + `PlaybackConfig`) and its test; ship gapless-only.
- [x] CLI: `debug-dsp <db> [track_id]` prints the resolved `af` chain + the RG breakdown (mode / raw gains / preamp / clip / net dB) + gapless / speed.
- [x] Tests: `chain.rs` builder (the `@rg` string, empty when off, **different gains → different chains** = the #8267 guard); `profile.rs` resolution (mode downgrade / preamp / clip clamp / off→None / episode→None); the libmpv `playback.rs` EOF test now sets a real `@rg` chain (proves the mpv `af` syntax). Music-only build green.

*Usable artifact:* correct, per-track ReplayGain on a labelled `af` chain, headless. `debug-dsp` shows it; a track plays with the head `volume` applied (verified against the real `testdata/` albums).

### Phase 5.5b — Equalizer (core + GTK)

Split headless-first: **5.5b-i** lands the graphic EQ in the chain + persistence + the CLI (applied at load); **5.5b-ii** adds live per-band `af-command` mutation and the first GTK "Sound" preferences dialog. Settled scope: a **10-band ISO octave** graphic EQ (31 / 62 / 125 / 250 / 500 / 1k / 2k / 4k / 8k / 16k Hz); the **parametric `anequalizer`** is a later follow-up, not 5.5b.

#### Phase 5.5b-i — Graphic EQ core + persistence + CLI (headless) ✅ (v0.0.40)

- [x] Graphic EQ in the chain (`player/chain.rs`): `build_af_chain(profile, eq)` appends `@eq:lavfi=[equalizer@b0=f=31:t=o:w=1:g=…, … equalizer@b9=…]`, a stack of named `equalizer` peaking bands at the ISO centres, **only when the EQ is non-flat** (flat ⇒ no `@eq`, the no-op chain). Each band is named `equalizer@b<n>` so 5.5b-ii can target it live with `af-command`. Avoids `superequalizer`/`firequalizer` (no runtime command). `eq_stage` is pure + unit-tested.
- [x] Named EQ presets + the active state **persisted** in the DB (migration `0008`, the `perspectives` precedent): `eq_presets(name, bands CSV)` seeded `Flat`, and the singleton `eq_state` (live bands + selected preset). `EqState` model (`bands: [f64; 10]`, `preset`), reads (`get_eq_state` / `list_eq_presets` / `get_eq_preset`), worker writes (`set_eq_state` / `save_eq_preset` / `delete_eq_preset`). The engine gained `PlayerCommand::SetEq` (host holds the EQ, applied on the next load); CLI `play` sends the persisted EQ.
- [x] CLI: `eq show` (bands + preset + the resolved `@eq` chain), `eq set <band> <gain>` (±24 dB clamp; marks custom), `eq preset list|save|load|delete` (`Flat` is undeletable).
- [x] Tests: the `@eq` builder (flat → no stage; non-flat → named bands at the ISO centres; `@rg` precedes `@eq`); EqState/preset round-trips + forgiving CSV parse (`tests/eq.rs`); the migration table-exists; the libmpv EOF test now sets a non-flat `@eq` chain (proves the `equalizer@b0=f=31:t=o:w=1:g=…` mpv syntax). Music-only build green.

#### Phase 5.5b-ii — Live mutation + the GTK "Sound" dialog ✅ (v0.0.41)

- [x] Live per-band gain via `af-command` (`host.af_command` → `equalizer@b<n>`'s `gain` command, gap-free; ffmpeg's `equalizer` supports it). `host.set_eq_band` does the live path when the `@eq` stage is present, and a **structural rebuild** only at the flat↔non-flat boundary (the stage appears / disappears) or on a preset switch (`set_eq` applies-when-playing). The host now keeps the `current_profile` so it can rebuild mid-playback. Engine `SetEqBand` + `PlayerHandle::set_eq_band`; a pure `eq_band_command` (unit-tested: a band change maps to `af-command @eq gain <dB> b<n>`).
- [x] First GTK preferences surface: a "Sound" `adw::PreferencesPage` in an `adw::PreferencesDialog` (the app's first; Phase 10 builds on it), opened by a header button or `Ctrl+,`. An Equalizer group of 10 vertical sliders (−12..+12 dB, 0-detent) under their ISO centre labels + a preset `ComboRow` (the saved presets + "Custom") + Save as… / Delete / Reset. Sliders drive the engine live and select "Custom"; preset/reset push the whole state; persistence is on close (slider edits) and immediate (explicit actions). The persisted EQ is also pushed to the engine at startup (`apply_persisted_eq`), which the GUI never did before.
- [ ] (Later) the parametric option via `anequalizer` (per-band freq/Q/gain, live `change`).
- [x] Tests: the `eq_band_command` mapping (no chain rebuild); an engine null-host integration that mutates bands live mid-playback and still reaches EOF (the real mpv `af-command` path); the `match_preset` projection; build + manual for the dialog. Music-only build green.

*Usable artifact:* (5.5b-i) a graphic equalizer with persisted presets via the CLI, applied to playback; **(5.5b-ii) the same with live sliders in the Sound preferences dialog — drag a band and hear it move.** **Phase 5.5b is complete.**

### Phase 5.5c — DSP modules + output quality (core + GTK)

Split headless-first, the 5.5b rhythm: **5.5c-i** lands the DSP modules in the chain + persistence + CLI; **5.5c-ii** adds output backend / resampler control and consolidates the GTK "Sound" page. Settled scope (spec §6.2, §16.6): a bounded, useful chain (compressor + brick-wall limiter + `dynaudnorm` leveler), not a deadbeef-class everything.

#### Phase 5.5c-i — DSP modules core + persistence + CLI (headless) ✅ (v0.0.42)

- [x] DSP modules as toggleable chain stages (`player/dsp.rs`, pure): compressor (`acompressor`), a brick-wall limiter (`alimiter`, `level=disabled` so it is a transparent peak catcher and the ReplayGain clip safety net), and a volume leveler (`dynaudnorm`, single-pass/live; NOT `loudnorm`, whose accurate mode is two-pass/offline-only). `build_af_chain(profile, eq, dsp)` appends `@comp` / `@limit` / `@boost` after `@eq` in signal-flow order, each present only when its module is enabled. User-facing dB knobs (compressor threshold, limiter ceiling) are converted to the filters' linear forms in the one stage builder.
- [x] Each module is `{ enabled, settings }` (`ModuleState<T>`): the settings persist while the module is off, so toggling a tuned module back on restores its parameters. The host holds the active `DspState` and applies it on each load; a settings change does a structural `af` rebuild (an explicit settings change, gap-acceptable; DSP has no per-slider live path like the EQ). Engine `SetDsp` + `PlayerHandle::set_dsp`.
- [x] Persisted in the singleton `audio_state` table (migration `0009`, the `eq_state` precedent): the playback defaults (ReplayGain mode / preamp / clip, gapless), the DSP modules, and the output backend / resampler — one row holding the whole audio config, so 5.5c-ii needs no second migration. `AudioState` model + `ResamplerQuality` enum, forgiving read (`get_audio_state`), worker write (`set_audio_state`). (The playback + output halves are consumed at 5.5c-ii; 5.5c-i seeds and stores them.)
- [x] CLI: `dsp show` (modules + the resolved `@comp` / `@limit` / `@boost` chain), `dsp comp on|off [--threshold --ratio --attack --release]`, `dsp limiter on|off [--ceiling]`, `dsp leveler on|off [--target --gausssize]`; `debug-dsp` prints the DSP breakdown; `play` applies the persisted DSP.
- [x] Tests: `dsp.rs` builders (off → no stage; on → expected lavfi string + correct dB→linear conversion); `chain.rs` full-chain order (`@rg < @eq < @comp < @limit < @boost`); `AudioState` round-trip + params-survive-an-off-toggle through the worker (`tests/audio_state.rs`); the migration table-exists; the libmpv EOF test now sets a real `@comp`/`@limit`/`@boost` chain (proves the `acompressor`/`alimiter`/`dynaudnorm` mpv syntax). Music-only build green. No new dependency.

Split headless-first, the 5.5b/5.5c rhythm: **5.5c-ii-a** lands the output backend / resampler apply in the player host + the engine commands + the CLI `output` verb group (CLI-testable); **5.5c-ii-b** consumes the persisted config in the GUI and consolidates the "Sound" page.

##### Phase 5.5c-ii-a — Output backend + resampler apply + CLI (headless) ✅ (v0.0.43)

- [x] Output quality applied in the player host: the **backend** (mpv `ao`, switched live via `ao-reload`, gap-acceptable) and the high-quality **resampler** knobs (`audio-resample-filter-size` / `-cutoff`, re-asserted per load; avoid-resample stays the default, `audio-samplerate` / `audio-format` left unset). `MpvHost` holds both; the seeded-but-unused `audio_state.output_backend` / `resampler_quality` (migration `0009`) are finally consumed.
- [x] Engine plumbing: `PlayerCommand::SetOutputBackend` / `SetResamplerQuality` + `PlayerHandle::set_output_backend` / `set_resampler_quality` (the `SetDsp` precedent). `PlaybackConfig::from_audio_state` maps the persisted playback defaults (RG mode / preamp / clip, gapless) into the resolver, kept in `player/profile.rs` so the db layer stays free of the `ReplayGain` enum.
- [x] CLI: `output show` / `output backend <auto|pipewire|pulse|alsa|jack>` / `output resampler <default|high>` (read `get_audio_state`, write `worker.set_audio_state`, the `dsp` verb precedent); `debug-dsp` gains the backend + resampler line.
- [x] Tests: host null-AO integration (`set_output_backend("null")` exercises the `ao` + `ao-reload` path hermetically; `set_resampler` High/Default; the EOF smoke now re-asserts a High resampler through `load`); `PlaybackConfig::from_audio_state` mapping (each mode + forgiving fallback); the CLI verbs verified end-to-end against a fixture DB. Music-only build green. No new dependency, no new migration.

##### Phase 5.5c-ii-b — The consolidated GTK Sound page ✅ (v0.0.44)

- [x] The "Sound" page (from 5.5b) consolidates the full chain: ReplayGain (mode / preamp / clip), EQ, DSP modules (each an `adw::ExpanderRow` with an enable switch + tuning rows, the app's first `ExpanderRow`), output backend / device / resampler, gapless. Defaults flow into `build_play_queue` / `build_mixed_queue` instead of `PlaybackConfig::default()` (a `playback_config()` read of `audio_state` via `PlaybackConfig::from_audio_state`), and a startup `apply_persisted_audio` hook pushes the DSP / backend / resampler into the engine (the `apply_persisted_eq` precedent; also fixing that 5.5c-i's DSP was stored but never GUI-applied). The device picker stays in the header too (a second write-through `ComboRow` in the page). DSP / output drive the engine live; ReplayGain / gapless resolve per-queue. The whole `audio_state` persists on dialog close.
- [x] **Deferred and recorded (not built):** exclusive/bit-perfect output (ALSA `hw:` + `--audio-exclusive`, bare-install-only, fights the Flatpak sandbox); LADSPA / raw-`af` escape hatch (needs the `org.freedesktop.LinuxAudio.Plugins` extension + ffmpeg `--enable-ladspa`); native `crossfeed` headphone module (cheap, a natural future stage); the parametric `anequalizer`; peak-aware ReplayGain attenuation (no peak columns; the limiter is the safety net).
- [x] Tests: the pure picker-mapping helpers in `ui/sound.rs` (`option_index` / `option_value` / `option_labels`, forgiving fallback + round-trip); the dialog widgets build + manual launch. Music-only build green. No new dependency, no new migration.

*Usable artifact:* (5.5c-ii-a) the output backend + resampler are driven from the CLI and persist (`output backend … pipewire` / `output resampler … high`), with the playback defaults flowing into the queue builders; **(5.5c-ii-b)** a foobar2000-style Sound/DSP preferences surface over the music engine. **Phase 5.5c-ii and Phase 5.5 are complete; the music daily-driver's audio engine is done.**

---

## Phase 6 — Podcasts (absorb Belfry)

Podcast parity. Belfry retires only when 6c lands (spec §16.8, CLAUDE.md). The fetch/parse port is `belfry-core`'s; Viaduct contributes the HTTP-client baseline. The subsystem lands as the **`conservatory-podcasts` plugin crate** (spec §2.2), which is where the heavy dependencies (`reqwest`, `feed-rs`, `ammonia`, `id3`, `oo7`) get pulled; its schema still lands in core's migration ledger (the boundary rule).

### Phase 6a — Fetch/parse port (headless)

The headless absorption, split so each piece leaves a usable artifact: **6a-i** is the core DB foundation (schema + worker, no network); **6a-ii** is the Viaduct-style fetcher + `feed-rs`/namespace parse + the refresh pipeline; **6a-iii** is OPML round-trip, credentials, and episode download.

#### Phase 6a-i — Podcast schema + core worker + models (no network) ✅

- [x] Schema: ported Belfry's eight podcast tables (`shows`, `episodes`, `playback`, `show_settings`, `listening_sessions`, `chapters`, `tags`, `show_tags`) as migration `0006`, with one change (spec §4.2): triage Queue state lives in the unified `queue` table, so `playback` drops Belfry's `in_queue` / `queue_position` (Inbox / Queue / Played derives from `playback.played` plus `queue` membership). `episode_fts` / `show_fts` added to the FTS set (ordinary tables + triggers, matching the music FTS style). The migration lands in `conservatory-core`'s ledger, not the plugin crate (spec §2.2).
- [x] **Queue `episode_id` foreign key**: migration `0006` rebuilds the `queue` table to add the FK deferred at `0005` (the 4b-i note: `foreign_keys = ON` refused it until `episodes` existed); `book_id` stays plain until Phase 7. `docs/schema.md` updated.
- [x] Core domain + DB plumbing: `Show` / `Episode` / `Playback`+`PlayedState` / `ShowSettings`+`InboxPolicy` / `ListeningSession` / `Chapter` / `Tag` models (`db/models.rs`); reads (`get_show`/`list_shows`, `get_episode_by_guid`/`list_episodes_for_show`, `get_playback`, `get_show_settings`, `list_chapters`, `list_tags_for_show`); worker commands + `WorkerHandle` methods (`get_or_create_show`, `update_show` carrying the conditional-GET state, `delete_show`, `upsert_episode` by `(show_id, guid)`, `upsert_playback`, `upsert_show_settings`, `replace_chapters`, `get_or_create_tag`, `set_show_tags`). The schema is core-owned (spec §2.2); the plugin (6a-ii+) consumes these typed methods.
- [x] Tests (`tests/podcasts.rs`): show get-or-create idempotency; episode upsert dedups by guid + a re-fetch never erases a downloaded path; FTS sync across edit/delete; playback + show-settings round-trip; chapters replace; tags round-trip; the queue `episode_id` FK verified via `PRAGMA foreign_key_list`. Migration table-exists test extended. Music-only build (`--no-default-features`) stays green (core is feature-free; the tables apply in every build).

*Usable artifact:* a show, its episodes, and its triage/playback/settings/chapters/tags round-trip through the single-writer worker, headless; the unified queue can now reference episodes.

#### Phase 6a-ii — Fetch + parse + refresh pipeline (headless)

Split again: **6a-ii-a** is the RSS-catching layer (HTTP client + conditional-GET fetcher, no parse); **6a-ii-b** is feed-rs/namespace parse + the refresh orchestration + CLI.

##### Phase 6a-ii-a — HTTP client + conditional-GET fetcher ✅

- [x] `conservatory-podcasts/src/http.rs`: the `reqwest` client baseline ported from **Viaduct's `network/http.rs`** (rustls, gzip/brotli, `POOL_MAX_IDLE_PER_HOST=4`, 30 s idle/request + 10 s connect timeouts, a descriptive `Conservatory/<ver>` User-Agent, the `ACCEPT_FEED` header). `build_client()`.
- [x] `conservatory-podcasts/src/fetcher.rs`: the conditional-GET `Fetcher` ported from the network slice of **Viaduct's `network/fetcher.rs`** (the mature path; Belfry's loop was never implemented). `fetch(url, etag, last_modified)` sends `If-None-Match` / `If-Modified-Since`, short-circuits 304 (empty body), extracts `ETag` / `Last-Modified` / `Cache-Control: max-age`, and keeps a per-host 429 cooldown honouring `Retry-After`. The broadcast request-coalescing and the content-hash re-parse skip are deliberately dropped/deferred (each show has a distinct feed URL; the content hash lives with the refresh state at ii-b). `FetchError` (`error.rs`).
- [x] Deps activated in `conservatory-podcasts`: `reqwest` (rustls/gzip/brotli), `tokio`, `chrono`, `thiserror`, `tracing` (+ `wiremock` dev). `bytes` deferred (body is `Vec<u8>`); ATTRIBUTIONS.md updated with the Viaduct/NNW provenance.
- [x] Tests (`tests/fetcher.rs`, `wiremock`, hermetic): 200 body + `ETag`/`Last-Modified`/`Cache-Control` extraction; conditional GET sends `If-None-Match` and handles 304; 429 + `Retry-After` returns `RateLimited` and the cooldown short-circuits the next fetch (verified by an `expect(1)` mock); invalid-URL path; `max-age` parse + UA/client smoke.

*Usable artifact:* a `Fetcher` that does conditional GET against a feed URL, honouring 304 and 429, headless and wiremock-tested.

##### Phase 6a-ii-b — feed-rs/namespace parse + refresh + CLI ✅

- [x] Parse (`parse.rs`): `feed-rs` for the RSS/Atom/JSON core plus Belfry's hand-rolled `podcast:` namespace handler (`namespace.rs`, ported, merged to feed-rs entries by position with a guid cross-check); a fresh `Entry → ParsedEpisode` mapping (enclosure from `entry.media`, guid = item `podcast:guid` else feed-rs id, pub_date/duration/season/episode/type); episode identity by `(show_id, guid)`. The namespace handler was **extended** to also read `itunes:season`/`itunes:episode`/`itunes:episodeType` (feed-rs surfaces none, and real feeds carry them there; `podcast:` wins). Show-note sanitize (`ammonia`) and chapter storage stay deferred to 6a-iii / 6b (the chapters URL is captured, not persisted). `feed-rs` / `quick-xml` signed off (ATTRIBUTIONS.md).
- [x] A `slugify` + `episode_dir` for the managed `Podcasts/<slug>/<date>--<ep-slug>` layout (spec §5.3, `slug.rs`). The refresh orchestration (`refresh.rs`) fetches concurrently under a `Semaphore(REFRESH_PARALLELISM)` (`tokio::task::JoinSet`), parses, upserts episodes through the 6a-i worker methods, and stamps `etag` / `last_modified` / `last_fetched` on the show (preserving the user-configured fields; triage is 6b). CLI `podcast add|remove|refresh` behind `#[cfg(feature = "podcasts")]`; the plugin gained a `conservatory-core` dep to drive the typed worker methods (no cycle; the §2.2 boundary is code, not schema).
- [x] Tests: `(show_id, guid)` dedup through a refresh, the podcast/itunes namespace parse + precedence, conditional-GET round-trip (etag stored on `add` then replayed for a 304), against `wiremock` feed fixtures + a real core worker on a temp DB.

*Usable artifact:* `podcast add <url>` then `podcast refresh` subscribes to and pulls a show's episodes entirely headless.

#### Phase 6a-iii — OPML + credentials + download (headless)

Split: **6a-iii-a** is OPML round-trip (network-free, no new deps); **6a-iii-b** is the credential store (`oo7`) + HTTP Basic auth wiring + episode download.

##### Phase 6a-iii-a — OPML round-trip ✅

- [x] OPML import/export round-trip, preserving tags and `applePodcastsID` (spec §8). `conservatory-podcasts/src/opml.rs`: `parse_opml` (forgiving, flattens folder hierarchy, every `<outline>` with an `xmlUrl` is a subscription; title = `title` else `text` else URL; tags from the Pocket Casts `category="a,b"`) and `write_opml` (OPML 2.0, XML-escaped). Import is **network-free**: `import_opml` creates the shows + tags through the worker (`get_or_create_show` / `get_or_create_tag` / `set_show_tags`), `applePodcastsID` into `shows.apple_podcasts_id`; episodes arrive on the next `refresh`. `export_opml` reads shows + tags back out.
- [x] CLI: `import-opml <db> <file>` / `export-opml <db> [--out <file>]` behind `#[cfg(feature = "podcasts")]`. No new deps (`quick-xml` from 6a-ii-b).
- [x] Tests: round-trip with escaping + the forgiving/nested/foreign parse + the title fallback (unit); import-through-a-worker creates shows + tag links + the Apple id, a re-import is idempotent, and export-then-reparse round-trips (`tests/opml.rs`).

*Usable artifact:* `export-opml` backs up your subscriptions; `import-opml` brings a list in (then `podcast refresh` pulls episodes).

##### Phase 6a-iii-b — Credentials (oo7) + episode download ✅

- [x] A `CredentialStore` (an enum, not a `dyn` trait, so the async methods stay simple) with an `oo7`/libsecret backend and an in-memory test backend; `shows.auth_pass_ref` stores the lookup key, never the password (spec §8). HTTP Basic auth wired into the fetch path (`fetch_authed`) from `(auth_user, password)`; `refresh_show` / `refresh_all` resolve a show's credential and attach it. `oo7` activated (`default-features = false` + `tokio` + `native_crypto`, since the default async-std runtime clashes with the workspace's tokio).
- [x] Episode `download` into the managed `Podcasts/<slug>/<date>--<ep-slug>/` tree (spec §5.3): stream to a sibling `.part` file, fsync, rename (the `mover::fsops` crash-safe shape); a new core `set_episode_audio_path` worker command + `get_episode` read record/resolve the row (`upsert_episode` preserves `audio_path`, so download sets it explicitly). CLI `podcast download <db> <episode_id> --root <root>`.
- [x] Tests: credential store round-trip + resolve rules (in-memory backend); a Basic-auth-gated download (401 without creds, 200 with, the password flowing through the store); download writes the file and sets `audio_path`; filename derivation. Hermetic (`wiremock` + a temp-DB worker).

*Usable artifact:* **Phase 6a (the headless podcast subsystem) is complete.** A private (Basic-auth) feed refreshes, and an episode downloads into the managed tree. What remains for podcasts is the GUI (Phase 6b).

### Phase 6b — Podcasts tab + triage

Split so the window-root restructure is isolated from the podcast feature work: **6b-i** turns the single-view music window into the multi-view shell of spec §2.3 (the adaptive `AdwViewSwitcher` over an `AdwViewStack`, Music as the first page, an empty Podcasts page); **6b-ii** fills that page with Belfry's triage. The shell is where the second tab first exists, so it lands here rather than implicitly inside the triage work.

#### Phase 6b-i — Window shell (AdwViewStack + adaptive view switcher) ✅

- [x] Restructure the window root (spec §2.3): the music browse moves out of `AdwToolbarView`'s content into an `AdwViewStack` page ("Music"); the header gains an `AdwViewSwitcher` (`policy = wide`) bound to the stack. `AdwViewSwitcherTitle` is deprecated since libadwaita 1.4 and is not used. (The always-on filter bar moved from a global top bar into the Music page so it does not show over the Podcasts tab; its behaviour is unchanged.)
- [x] Adaptive collapse: an `AdwBreakpoint` (max-width 550sp) hides the header switcher and reveals a bottom `AdwViewSwitcherBar` on narrow widths.
- [x] Now-bar / switcher-bar stacking (spec §2.3): the persistent Now-bar stays the stable innermost bottom bar; the `AdwViewSwitcherBar` is added after it (reveals *beneath* it) only at the narrow breakpoint.
- [x] Feature-gated: the switcher, bottom bar, breakpoint, and Podcasts page exist only behind `#[cfg(feature = "podcasts")]` (the binary's first feature gates); `--no-default-features` (music-only) keeps a single-page stack with no switcher chrome, visually unchanged (spec §2.2, §2.3). (7b generalises the gate to include `audiobooks`.)
- [x] Lazy page construction: the Podcasts page builds its child on the child's first `::map` (an empty placeholder until 6b-ii); `AdwViewStack` retains each page's widget state once built.
- [x] Keyboard: `Alt+1` / `Alt+2` / `Alt+3` switch top-level views (a global `ShortcutController`, the AdwTabView convention; `Ctrl+1/2/3` left free for the podcast triage lists). `Alt+3` is inert until the Audiobooks tab (7b). `docs/keymap.md` updated.
- [x] Tests: the `Alt+N` → page-name mapping is a pure unit test; a launch smoke confirms the new tree constructs and runs cleanly; the music-only build compiles + runs with no switcher widgets present.

*Usable artifact:* the GUI is a multi-view window. Switch between Music and an empty Podcasts tab, the switcher collapsing to a bottom bar on narrow widths, the Now-bar persistent across both. The music-only build is visually unchanged (no switcher).

#### Phase 6b-ii — Triage panes

Split: **6b-ii-a** is the read-only three-pane browse (sidebar + episode list + detail); **6b-ii-b** is the triage actions (mark played/archived, star) + the Tags sidebar (DB + GUI, no engine); **6b-ii-c** is episode playback + the unified queue + per-show overrides (the engine-touching half).

##### Phase 6b-ii-a — Triage browse (read-only) ✅

- [x] The Podcasts view (spec §3.7): a sidebar of triage buckets (Inbox / Queue / Played) and subscribed shows, an episode list (`ColumnView`) with a played-state glyph + title/date/length, and a detail pane with show notes, filling the 6b-i page (built lazily on `::map`). `conservatory/src/ui/podcasts.rs` (nested `gtk::Paned`; AdwNavigationSplitView is a later refinement). Tags sidebar section deferred to 6b-ii-b (needs a tag-filtered read).
- [x] Core triage reads: `EpisodeListRow` + `episodes_for_show` + `episodes_in_bucket` (the §4.2 derivation: Queue = unified-queue membership, Played = `played >= PlayedFully`, Inbox = the rest). CLI `podcast episodes <db> [--show <id> | --bucket inbox|queue|played]` (the headless surface).
- [x] Tests: the bucket derivation (core integration test); `EpisodeRow` formatting (unit); the GTK view's construction (display-guarded build test). Music-only build stays green.

*Usable artifact:* open the Podcasts tab and browse your subscriptions: pick Inbox/Queue/Played or a show, see its episodes with state, read the show notes.

##### Phase 6b-ii-b — Triage actions + Tags ✅

- [x] Triage transitions (mark played / unplayed / archived, star) via **partial** playback upserts (`set_episode_played` / `set_episode_starred`, so an action never clobbers a sibling field; marking unplayed rewinds the position). A detail-pane action bar in the GUI; the list glyph + bucket counts refresh after each action. CLI `podcast mark` / `podcast star` (the headless surface).
- [x] A Tags sidebar section: `list_all_tags` + a tag-filtered `episodes_for_tag` read; `Source::Tag` in the view.
- [x] Tests: the partial writes (mark-played keeps starred and vice-versa; mark-unplayed rewinds; archived → ArchivedUnlistened) + the tag-filter read + bucket reflection, a core integration test.

*Usable artifact:* the Podcasts inbox is actionable: mark episodes played / archived, star them, filter by tag, all reflected live.

Episode playback splits again, because the per-kind persistence + resume is the engine-risky part: **c-1** plays episodes forward (persistence guarded to track-only); **c-2** adds episode resume + per-kind persistence; **c-3** adds per-show overrides.

###### Phase 6b-ii-c-1 — Episode playback (forward) ✅

- [x] The structural change from Belfry: **Queue is the shared unified queue**, so an episode and a track sit next to each other. `enqueue_episodes` / `replace_queue_with_episodes`; the `load_queue_display` episode join (else queued episodes render blank); `build_episode_queue` (downloaded file `root`+`audio_path`, else the enclosure URL; libmpv `loadfile` streams a URL as-is) + a basic `resolve_episode_profile`; `EpisodeListRow`/`EpisodeRow` carry `audio_path`/`audio_url`.
- [x] **Per-kind persistence guard:** the engine persists position + play counts only for `MediaKind::Track`, so an episode plays to EOF but never writes an episode id into the music `playback_state` / `tracks.play_count`. (Episode resume + per-kind persistence is c-2.)
- [x] GUI: the Podcasts episode list plays on double-click / Enter (the visible list from that row) and appends on `Ctrl+Enter`, the music leaf idiom.
- [x] Tests: episode queue write + display join (core); `build_episode_queue` local/stream/skip (unit); an engine null-sink test playing an episode to EOF that asserts the guard held; the existing track-playback test still passes (music-regression check).

*Usable artifact:* double-click an episode in the Podcasts list and it plays (downloaded or streamed) in the unified queue, with the Now-bar + queue drawer.

###### Phase 6b-ii-c-2 — Episode resume + per-kind persistence ✅

- [x] The engine writes the podcast `playback` table on episode tick/EOF (position + `played`/`play_count`), not the music `playback_state` singleton; the resume cursor learns the current item's kind so a restart resumes an episode to the second. Migration `0007` adds `playback_state.kind` + `episode_id` (the cursor); new partial-upsert worker writes `set_episode_position` (InProgress, preserving starred/play_count) and `complete_episode` (PlayedFully + play_count bump) carry the per-episode state. The engine's three persistence sites dispatch by `MediaKind` (the episode position write is synchronous + guarded on `!ended` so a terminal flush cannot clobber the completion). The CLI `play` and GUI `resume_saved_queue` rebuild a mixed track+episode queue (`build_mixed_queue`) and resume at the cursor's `(kind, id)`.
- [x] Tests: episode persistence + cursor round-trip through the worker (`podcasts.rs`); an episode plays to EOF writing the podcast `playback` row (PlayedFully + play_count) but never the music cursor, with a colliding track untouched (`queue.rs`); `build_mixed_queue` interleave / skip / cursor re-index (`playqueue.rs`); the music-only build stays green.

###### Phase 6b-ii-c-3 — Per-show overrides

Split, because the six settings span playback (resolved into the profile) and management (refresh-time routing + retention), and a GUI editor: **c-3-a** is per-show playback speed, headless; **c-3-b** is inbox-policy routing + retention; **c-3-c** is the GUI per-show settings panel. Smart Speed / Voice Boost are flags carried for Phase 6c (the `af`-chain), not built here.

###### Phase 6b-ii-c-3-a — Per-show playback speed (headless) ✅

- [x] `MusicProfile` (the de-facto single per-item profile, spec §6.1) gains `speed` + `pitch_correction`; `resolve_episode_profile(Option<&ShowSettings>)` resolves the speed from the show's `playback_speed` (clamped to `[0.25, 4.0]`, pitch correction on), music stays at 1.0. `MpvHost::load` applies mpv `speed` + `audio-pitch-correction` (scaletempo2) before `loadfile` (1.0/off is a no-op for the track path).
- [x] The episode-queue builders (`build_episode_queue`, `build_mixed_queue`, the CLI `resolve_queue_items`) thread each episode's show settings in: `EpisodeSource`/`MixedQueueRow`/`QueueDisplayRow` carry `show_id`, a new core `show_settings_map` batch-reads them, and the builders resolve speed per show. `EpisodeRow` exposes `show_id`.
- [x] CLI `podcast settings <db> <show_id> [--speed N]` views or sets a show's overrides (preserving the other fields), the headless surface.
- [x] Tests: profile speed resolution + clamp; `build_episode_queue` / `build_mixed_queue` apply per-show speed; a host integration test asserts `load` sets mpv's `speed`; music-only build green.

*Usable artifact:* set a show's speed (`podcast settings ... --speed 1.5`) and its episodes play at that rate with corrected pitch, in the unified queue.

###### Phase 6b-ii-c-3-b — Inbox-policy routing + retention ✅

- [x] Apply a show's `inbox_policy` to new episodes on refresh (Inbox / AlwaysQueue / AlwaysArchive); prune downloaded episodes beyond `keep_count` (retention). These are management settings (`conservatory-podcasts`), not playback. Routing rides `refresh::apply_feed`: the show's settings are read once (default Inbox when absent), and **only genuinely-new episodes route** (a re-refresh never re-queues one the user removed); `AlwaysQueue` enqueues to the unified queue, `AlwaysArchive` marks `ArchivedUnlistened`, `Inbox` is a no-op (the §4.2 derivation). Retention is a separate **root-aware** pass (`retention.rs`: `plan` → `apply`, the mover's dry-run-then-apply shape): downloaded episodes beyond `keep_count` (0 = keep all) lose their file + `audio_path` (revert to stream-only); a new `clear_episode_audio_path` worker command + the `podcast prune <db> [show_id] --root [--apply]` verb (dry-run default).
- [x] Tests: a new episode routes per policy and an already-seen one does not re-route (`refresh.rs`); retention prunes the oldest downloads, keeps the newest, ignores `keep_count = 0` and stream-only episodes (`retention.rs`); music-only build green.

###### Phase 6b-ii-c-3-c — GUI per-show settings panel ✅

- [x] A per-show settings surface in the Podcasts detail pane (spec §3.7): speed, Smart Speed / Voice Boost toggles (the flags 6c consumes), skip intro/outro, inbox policy. Writes through `upsert_show_settings`. A gear button appears in the detail pane when a **show** is the selected sidebar source, opening an `adw::AlertDialog` whose `extra_child` is an `adw::PreferencesGroup` (`SpinRow` speed/skip, `SwitchRow` Smart Speed/Voice Boost, `ComboRow` inbox policy), pre-populated from `get_show_settings` (or the schema-default skeleton). Reuses the bulk-edit dialog idiom + the `rt.block_on(worker.*)` write idiom; the working episode-detail flow is untouched. First use of the libadwaita preference-row widgets (no new dep; libadwaita 0.7.2). The panel preserves the global-inherit `skip_forward`/`skip_back` fields it does not expose.
- [x] Tests: the pure form mapping (`inbox_policy_*` index round-trip, out-of-range degrade; `settings_from_form` applies edits + preserves the skip fields; `default_settings`) unit-tested headless in `conservatory/src/ui/podcasts.rs` (constructs no widgets); the dialog itself is build + manual (the 3b/3c GUI precedent). Music-only build green.

*Usable artifact:* podcasts play in the one queue (downloaded or streamed), resuming where you left off, with per-show speed/boost settings editable in the GUI. (Smart Speed / Voice Boost filters are 6c.) **Phase 6b-ii-c (episode playback + per-show overrides) and the c-3 split (a speed / b routing+retention / c GUI) are complete; 6b-ii is done.**

### Phase 6c — Podcast playback profile + parity

Split into the audio engine (6c-i / 6c-ii, the parity-critical half, done first) and a follow-on for the surfacing work (6c-iii+). Belfry retires only when the whole of 6c lands (spec §16.8), so the follow-on is part of parity, not optional.

Two filter choices were settled against the 5.5 findings (`docs/libmpv-profiles.md`): variable speed via mpv `--speed` + `audio-pitch-correction` (scaletempo2) rather than a chained `rubberband` at every speed, and live single-pass `dynaudnorm` rather than two-pass/offline `loudnorm`. Smart Speed is the `silenceremove` af-filter (spec contract). **A consequence: `rubberband` is no longer actually used**, so at 6c-i the spec §15 / ATTRIBUTIONS "GPL-3 forced by librubberband" rationale was re-confirmed and reworded: GPL-3 is forced by the **linked GPL stack** (the GPL ffmpeg build the chain filters ride, and librubberband where the mpv build carries it), not by a `rubberband` call we make. GPL-3 stands.

#### Phase 6c-i — Smart Speed + Voice Boost af stages (headless core + CLI) ✅ (v0.0.45)

- [x] Podcast profile ported from Belfry §5: Smart Speed (silence-skip via `silenceremove`) and Voice Boost (compression + voice EQ + live `dynaudnorm`), **built as presets on the Phase 5.5 `af`-chain engine, not a parallel hardcoded path** (`player/spoken.rs`: `smart_speed_stage` `@ss`, `voice_boost_stages` `@vbcomp`/`@vbeq`/`@vbnorm`). `MusicProfile` carries `smart_speed` / `voice_boost`; `resolve_episode_profile` sets them from the show settings (false for music, so the music chain is unchanged). `build_af_chain` appends them after the music stages, Smart Speed first.
- [x] CLI `podcast debug-chain <ep>` dumps an episode's resolved chain; the GUI per-show dialog's "audio processing arrives later" caption is gone.
- [x] Tests: the `spoken.rs` builders; `build_af_chain` episode-vs-music; the libmpv `ao=null` EOF run sets both flags (proves the `silenceremove` + Voice Boost mpv syntax decodes). Music-only build green. No new dependency, no new migration.

#### Phase 6c-ii — Time-saved accounting (headless core + CLI) ✅ (v0.0.46)

- [x] Episodes share the unified queue and the per-item profile switch prototyped in 4b; **append-only `listening_sessions` discipline**. The engine writes one session row per episode boundary (`player/session.rs`'s pure `SessionAccumulator` + the engine's start-on-load / close-on-boundary wiring); `smart_speed_saved = max(0, audio_seconds/speed − real_seconds)` (the non-linear-timeline math `silenceremove` requires, with its forward playhead jumps counted as covered audio), with user-seek ticks excluded and pauses resynced rather than accrued. `insert_listening_session` (write) + `listening_totals` (aggregate read); CLI `podcast stats`. No new migration, no new dependency.
- [x] Tests: the accounting math (seek-excluded ticks, Smart-Speed-off ≈ 0, variable-speed nets zero, pause accrues nothing, divide-by-zero guard); a `listening_sessions` append round-trip; an engine null-host run lands exactly one session row with sane totals.

#### Phase 6c-iii+ — Chapters + Now Playing additions (the follow-on)

Split into the chapters core (persistence then navigation) and the surfacing work:
**a** = chapter persistence, **b** = chapter navigation (the shared engine
mechanism 7c reuses), **c** = the Now Playing episode surface, **d** = sleep timer.

##### Phase 6c-iii-a — Chapter persistence (headless + CLI) ✅ (v0.0.47)

- [x] Persist the parsed chapter set (the 6a-ii note: the `podcast:chapters` URL was captured but not stored). `conservatory-podcasts/src/chapters.rs` fetches the URL and parses the Podcast Index JSON; `refresh::apply_feed` stores it for each genuinely-new episode through the existing `replace_chapters` worker command (best-effort: a fetch/parse failure is logged, never fatal). CLI `podcast chapters <ep>`. `serde`/`serde_json` activated in the podcasts crate. ID3-CHAP fallback (from a downloaded file) stays deferred to the -c fold-in.
- [x] Tests: the JSON parser (full / empty / malformed / blank strings); a wiremock refresh that serves a feed + its chapters JSON and asserts the set lands.

##### Phase 6c-iii-b — Chapter navigation (the shared engine mechanism) ✅ (v0.0.48)

- [x] A **skip-to-next / skip-to-previous-chapter** transport action (an absolute `seek` to the neighbouring `chapters.start_time`) wired to buttons in the Now-bar and a keybinding (`Ctrl+Shift+←/→`). Built generic in the core player (`player/chapters.rs`: `ChapterMark` on `PlayableItem`, pure `current_chapter_at` / `neighbour_chapter` helpers, a `SkipChapter` command + snapshot `current_chapter`/`chapter_count`) so the audiobook engine reuses it at 7c with `book_chapters` (not a podcast-only path). The GUI attaches marks after a queue build (`attach_episode_chapters`); the Now-bar chapter buttons appear only for a chaptered item.
- [x] Tests: the `neighbour_chapter` helpers (forward / back / clamped at the ends); an engine skip forward-to-boundary / back-to-start (paused); the filter-graph swap between a track and an episode mid-queue (both play to completion, proving the §16.9 profile switch). No new dependency, no new migration.

##### Phase 6c-iii-c — Now Playing episode surface ✅ (v0.0.49)

- [x] Now Playing additions for episodes: a clickable chapter list in the bottom drawer (jump-to via `seek`, current-chapter highlight that follows the playhead), `ammonia`-sanitized show notes, and a Smart Speed indicator (live saved time). Show notes are cleaned **at ingest** (`conservatory-podcasts/src/notes.rs`, HTML → plain text), so every reader benefits and the DB column is clean. The snapshot gained `smart_speed_active` / `smart_speed_saved`; the chapter highlight + Smart Speed line tick from the existing 250 ms poll (a class toggle, not a rebuild). `ammonia` activated in the podcasts crate.
- [x] Tests: the `sanitize_notes` cases (tags / entities / paragraph breaks / dropped `<script>` / malformed / blank-line collapse); a snapshot assertion that an episode with Smart Speed on reports it active. No new migration.
- ID3-CHAP embedded-chapter fallback (from a downloaded file, `id3`) stays deferred (a small later fold-in).

##### Phase 6c-iii-d — Sleep timer ✅ (v0.0.52)

- [x] Sleep timer (15 / 30 / 45 / 60 min, end of episode, end of queue, tap-to-extend; Belfry §3.6), the `S` keybinding. Built engine-side as a pure, unit-tested clock (`player/sleep.rs`: `SleepMode` `After`/`EndOfItem`/`EndOfQueue` + `SleepClock`) the engine ticks each loop turn: a duration timer counts down only while playing and pauses on elapse (the session-accumulator idle precedent), opening a 30 s tap-to-extend window that the next `Play` re-arms; "end of item" pauses cued on the next item at the EOF boundary; "end of queue" disarms when the queue ends. Transient per-session state, so no DB persistence and no migration. **Media-agnostic** (the user's scope decision: an album track gets a sleep timer too, broadening spec §3.6's episode-only wording; the menu's boundary row reads "End of track" / "End of episode" / "End of book" by kind). Surfaced as a Now-bar moon `MenuButton` (the `build_output_menu_button` popover idiom; shows the `M:SS` remaining for a duration timer) plus a "Sleep · …" line in the Now Playing drawer; `S` pops the menu (a window-local controller so the bare letter does not fire in the filter entry). CLI `play --sleep <15|30|45|60|episode|queue>` arms it headless.
- [x] Tests: the `sleep.rs` clock (count-down-only-while-playing, fire-at-zero, tap-to-extend inside/outside the 30 s window, boundary modes have no countdown); an engine null-host run where a duration timer fires + pauses mid-queue then tap-extends, and an "end of item" timer pauses at the first item's boundary without playing the second (`tests/sleep.rs`); the GUI label helpers (`fmt_sleep_remaining` rounds up, `sleep_boundary_label` by kind, `sleep_drawer_text`). No new dependency, no new migration. Music-only build green.

*Usable artifact:* **podcast parity reached.** One queue, one engine, both media types, full Smart Speed / Voice Boost, with the sleep timer the last piece. **Belfry has now retired** (v0.0.52, spec §16.8): the GitHub repo is archived and the `~/.gitrepos` project map updated; the local clone is kept frozen as reference.

---

## Phase 7 — Audiobooks (the third tab)

Audiobooks are the third media type (spec §3.8), landing as the **`conservatory-audiobooks` plugin crate** (spec §2.2). They are long-form speech, so they reuse the absorbed spoken-word engine (Smart Speed, Voice Boost, variable speed, sleep timer) from Phase 6c and the unified queue; that is why this phase lands after podcast parity. The data model, import, and browse surface are modeled on **Cozy** (the GTK4/libadwaita audiobook player); the metadata model and folder conventions on **Audiobookshelf**; chapter handling technique on **m4b-tool** (all three cloned under `~/.gitrepos/` as read-only reference, ATTRIBUTIONS.md). Belfry's retirement at 6c is unaffected. Metadata is local-source-only in v1 (online providers deferred, spec §16.10).

### Phase 7a — Audiobook model + import (headless)

Split i/ii/iii like Phase 6a: **7a-i** the schema foundation (migration + models + worker CRUD), **7a-ii** the tag/sidecar reader + chapter resolver (carries the embedded-M4B-chapter-reading dependency decision), **7a-iii** the path template + import pipeline + CLI.

- [x] **Schema (7a-i, v0.0.53):** migration `0011` per spec §4.5: `book_people` (authors + narrators, role-tagged), `series`, `books`, `book_authors`, `book_narrators`, `book_chapters`, `book_playback`. `book_fts` (title, author, narrator, series) trigger-synced (spec §4.4), with author/narrator/series denormalized from the link tables. The unified `queue` gains the `book_id` foreign key (the deferred 0006 rebuild). Core models + read helpers + single-writer CRUD, with a worker round-trip + FTS + queue-FK test suite. The migration lands in `conservatory-core`'s ledger, not the plugin crate (spec §2.2).
- [x] **Tag + sidecar reader (7a-ii, v0.0.54):** embedded M4B/ID3 tags (lofty), then the Audiobookshelf sidecar conventions (`.opf` via `quick-xml`, `desc.txt`, `reader.txt`, `cover.jpg`), then folder structure, into a `BookDraft` (author, narrator, series + decimal sequence, year, publisher, ISBN/ASIN, description, language). Author and narrator are distinct roles, merged by precedence sidecar > tags > folder. Custom `NARRATOR`/`SERIES`/`SERIES-PART` frames are read across both ID3v2 `TXXX` and MP4 freeform spellings; people sort last-name-first (`person_sort_name`, not the music article-move). `conservatory-audiobooks` filled; `audiobook debug-read` CLI artifact.
- [x] **Chapter resolver (7a-ii):** embedded M4B markers (via an `ffprobe -show_chapters` shell-out, no Rust MP4 dep, the rsgain precedent) → else one-file-per-chapter folder (ordered by the part tag) → else a whole-file single chapter. Each chapter is a `(file_path, file_offset, duration)` draft addressing either a standalone file or an M4B span. Opt-in silence detection stays deferred (spec §16.11).
- [x] **Audiobook path template (7a-iii, v0.0.55):** `Audiobooks/{author}/{series}/{series_index:02}. {title} ({year})`, the series components collapsing for standalone books (a standalone renders under the literal `Standalone`, so every author folder is two levels deep, spec §5.7). The new tokens (`{author}`, `{narrator}`, `{series}`, `{series_index}`) extend the Phase 2a engine through a small `TemplateFields` trait shared with the music `TrackFields`, so music rendering is byte-for-byte unchanged (the existing 17 path tests guard it). The series index is decimal-aware: an integral `1.0` zero-pads to `01`, a fractional `1.5` renders unpadded.
- [x] **Import pipeline + the mover learns about books (7a-iii):** the plugin's `import_book` resolves a `BookDraft` into rows and moves the book's files into the managed tree via the Phase 2c mover (dry-run + undo, books owned like music, not ephemeral like podcasts). The journaled mover gains a `book_id` column (migration `0012`): a book op rewrites every `book_chapters` row of the book whose path matches the moved file, so a single M4B that backs many chapters follows the one moved file. Move ops are built per unique physical file, not per chapter. A conflict refuses the whole import with no rows written; one book per call.
- [x] **Cover + accent (7a-iii):** the Hermitage median-cut accent into `books.accent_rgb` at insert, and the cover written into the moved book folder via the existing folder-based cover sync (`set_book_cover_path`), the music-import shape (spec §7.4).
- [x] **CLI (7a-iii):** `audiobook import` (copy default, `--move`, a conflict refuses with a nonzero exit) and `audiobook set` (rating / starred / shelf genre; path-affecting edits deferred to 7b). Undo is the existing media-agnostic `organize --undo <job>` (spec §9).
- [x] **Tests (7a-iii):** the book path render (default template, standalone, integer + decimal index, collapsing groups, sanitization, and the music regression set); the mover book round-trip (multi-file + the single-M4B many-chapters case, undo, crash replay); and the end-to-end import of the multi-file fixture (rows, moved files, conflict refusal). Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

*Usable artifact:* point the CLI at a folder or an `.m4b` and get an organized, database-owned audiobook with ordered chapters, headless. **Phase 7a complete.**

### Phase 7b — Audiobooks tab (browse)

Split like Phase 6b: **7b-i** the read-only browse surface (the cover shelf + detail pane + state derivation), **7b-ii** the filter bar + bucket sidebar, **7b-iii** bulk edit.

- [x] **The Audiobooks view (7b-i, v0.0.56):** the third `AdwViewStack` page added to the Phase 6b-i shell (the switcher now offers all three, Alt+3), built lazily on `::map`. A cover-grid **shelf** (`gtk::GridView`, the app's first; accent-tinted tiles, the Hermitage unit, the first `accent_rgb` use in the GUI) beside a book **detail pane** (cover, title, author/narrator · series/sequence · year, a progress bar + state, and the chapter list), a side-by-side `gtk::Paned`. The multi-view chrome split out of `attach_podcasts_view` into a shared `install_view_chrome` so podcasts and audiobooks are independent (either alone still gets the switcher). Per-book speed / sleep-timer controls are playback, so they land at 7c. Cozy's layout over Conservatory's database. New core read `list_book_rows` (denormalized author/narrator/series + progress, the `EpisodeListRow` precedent) + the `audiobook list` CLI artifact.
- [x] **State derivation (7b-i):** New / In progress / Finished from `book_playback` (`BookState::derive`), pure and tested; `sort_shelf` surfaces in-progress books first (most recently played first).
- [x] **Filter bar (7b-ii, v0.0.57):** the Audiobooks shelf gains an always-on `gtk::SearchEntry` wired to `conservatory-search`; same grammar, no separate search mode, `Ctrl+F` focuses it (Managed scope, so it does not collide with the window's global music `Ctrl+F`). The audiobook fields joined the **shared** `Field`/`State`/`SearchItem` (`author:`/`narrator:`/`series:`/`is:finished`), so they are known on every surface (a book field in the music bar matches nothing); they are **eval-only** (`sql_translate` returns `None` for them, forcing the in-memory path). `is:finished` follows the `is:played`/`is:starred` shape; negate with `NOT is:finished` (the spec's `is:finished false` example predates the `is:` mechanics). The shelf is loaded whole and re-filtered in memory per keystroke (no debounce; tens of rows). New GUI `book_query::filter_books` (headless, unit-tested) + the `audiobook list <db> [expr]` CLI filter (the headless artifact). A degraded expression tints the bar (`filter-warn`).
- [x] **Bulk edit, headless half (7b-iii-a, v0.0.58):** the typed `BookEdit` resolver (`conservatory-audiobooks/edit.rs`, pure: path-affecting = author/series/series_index/title/year; narrator/shelf_genre/rating/starred are not), the core worker writes (broadened `update_book`, new `set_book_series` (clears to standalone), `set_book_authors`/`set_book_narrators` clear+relink), and the **book reorganize** mover path (`reorg.rs`: `apply_book_edit` writes metadata, `plan_book_reorg`/`apply_book_reorg` re-render the folder and move the files via the Phase 2c journaled mover under `MoveKind::Organize` — no mover change needed, a `book_id` op rewrites chapters + folder the same as import). CLI `audiobook set` gained `--title/--year/--author/--narrator/--series/--series-index` + `--root`/`--apply` (dry-run previews the move; `--series ""` clears to standalone). Cover follows the move (best-effort). Tests: resolver units, the reorg round-trip (multi-file + single-M4B + undo + unchanged no-op + conflict refusal). Verified end-to-end against the import fixture.
- [x] **Bulk edit, GTK half (7b-iii-b, v0.0.59):** the shelf became `MultiSelection` (a plain click still selects one, so the detail browse is unchanged; the detail pane follows the first selected book). A pencil button on the filter bar + `Ctrl+E` (Managed scope, no collision with the window's global music shortcuts) open an `adw::AlertDialog` bulk-edit grid (Author(s)/Narrator(s) `;`-separated, Series, Series index, Title, Year, Shelf genre, Rating, plus a **Standalone** checkbox = the explicit series clear). Blank = unchanged; a bad value rejects the whole set. Apply writes metadata via `apply_book_edit`, then a path-affecting edit aggregates `plan_book_reorg` across the books into a single **"Move N files?"** confirm → `apply_book_reorg` per book → shelf reload. The pure edit/move logic is the 7b-iii-a tests; the dialog is build + manual. **Completes Phase 7b.**
- [ ] Tests: shelf/filter model logic; book-state derivation; Perspective save/reload over books.

*Usable artifact (7b-i):* browse the audiobook library as a cover shelf, in-progress first, and open a book for its metadata, progress, and chapters; filter / sort / bulk-edit follow in 7b-ii/iii.

### Phase 7c — Audiobook playback (chapters + first-class resume)

Split into three commits, mirroring the 6b-ii-c episode rollout: the headless segment engine, then resume + per-book profile + chapter navigation, then the GTK / MPRIS surface.

- [x] **Book plays as one item, the segment engine (7c-i, v0.0.60):** a book is one `PlayableItem` (kind `Audiobook`). The new pure `player/book.rs` (`plan_book` / `BookSegment` / `BookPlan` / `locate` / `build_book_item`) maps a book's chapters into per-file **segments** with cumulative book-absolute starts and lifts the chapter marks to absolute book time. The engine advances file to file *internally* on each file's EOF (no queue advance, the session stays open) and completes the book (`book_playback.finished`) only at the last file's EOF (spec §6.1); the snapshot reports book-absolute position / duration. CLI `audiobook play <db> <id> --root`. Decision: engine-driven file advance (an M4B is gapless and chapters are internal seeks; a multi-file book gaps briefly at file boundaries, true cross-file gapless a later refinement). Tests: multi-file advance through every file, single-M4B completion, the segment math units.
- [x] **Resume + per-book profile + chapter navigation (7c-ii, v0.0.61):** migration 0013 (`playback_state.book_id` cursor; `listening_sessions` rebuilt with a nullable `book_id`/`episode_id` + an exactly-one CHECK so book Smart-Speed time-saved feeds `stats`). `resolve_book_profile` reuses the spoken-word `af`-chain (variable speed, Smart Speed, Voice Boost) with the per-book overrides from `book_playback` (spec §6.3); no new filter graph. The engine now speaks a **book-absolute** timeline: a unified `playhead()` maps the current segment's cumulative start onto the host's per-file `time_pos`, so the snapshot position/duration, the resume write (`book_playback.position` on the insurance interval / pause / seek, spec §6.4), the cursor (kind + `book_id`), the listening session, and the chapter highlight all span the book's files. `seek_book_absolute` maps an absolute target to `(segment, in-file offset)`, loading the right file, so a slider seek, a launch-resume, and a chapter skip all cross the file boundary. CLI `audiobook play --resume` + `audiobook settings --speed/--smart-speed/--voice-boost`. Tests: resume position + cursor persist mid-book, a cross-file seek lands in the third file, chapters progress across all three files, a completed book writes one book-keyed session, and `resolve_book_profile` defaults/overrides/clamp.
- [x] **GTK, MPRIS, per-book settings (7c-iii, v0.0.62):** double-click / Enter on a shelf cover plays the book and the rest of the shelf into the unified queue (`build_audiobook_queue`); Ctrl+Enter appends the selection (`enqueue_books` / `replace_queue_with_books` worker commands). The Now-bar and Now Playing drawer gained a book surface (`book_metadata` → title / first author / series / duration / cover; `book_fields` projection; the clickable chapter list synthesized from the book-absolute marks, reusing the shared seek). `load_queue_display` joins `books` so a book shows in the queue drawer and the launch-resume cursor (`build_mixed_queue` Audiobook arm + `attach_book_chapters`) rebuilds it. MPRIS `current_meta` now dispatches by kind (track / episode / book), spec §6.5. A gear in the detail pane opens a per-book playback-settings dialog (speed / Smart Speed / Voice Boost → `upsert_book_playback`, preserving the resume position). `book_fields` unit-tested; the GUI is build + manual-launch (the GTK-view precedent).

*Usable artifact:* **audiobook parity.** Play a book from the shelf with chapters, variable speed, sleep timer, and exact resume, in the one unified queue alongside music and podcasts.

**Phase 7 is complete (v0.0.62).** Audiobooks are a full third media type: model + import (7a), browse + filter + bulk edit (7b), and playback with chapters, per-book speed / Smart Speed / Voice Boost, sleep timer, and first-class resume (7c), all on the one unified queue. The three tabs (Music | Podcasts | Audiobooks) are each first-class.

---

## Phase 8 — Library maintenance and audits

A read-only health-and-hygiene suite modeled on **Lattice** (Brandon's CLI/TUI music auditor; ATTRIBUTIONS.md). Lattice scans the filesystem and reports, never mutating; Conservatory already owns the database, so these audits run against the DB plus the managed files. Each surfaces as a CLI verb first (the every-surface-CLI-testable rule), with GUI reports layered on later. The phase is **media-type-agnostic and depends only on Phases 1 to 3**, so it can be pulled forward of Phases 6/7 if a library-integrity need arises; it is placed here so it can cover all three media types at once. Integrity and decode checks shell out to `flac` / `ffmpeg` (external-tool sign-off, spec §11).

Deliberately **not** adopted from Lattice: the AI-readable library exports (`--ai-library` / `--ai-wings`, an LLM-prompt text dump) and the per-genre "wings" text trees, both superseded by Conservatory's live faceted browse; and Lattice's path-pattern tag fallback (Conservatory's tags come from the database, not the path).

### Phase 8a — Integrity verification

Modeled on Lattice's `--testFLAC` / `--testMP3` / `--testOpus` / `--testWAV` / `--testWMA` and its four-tier classification.

- [x] Decode-verify every file (or a selection) with parallel workers (`conservatory-core/src/verify.rs`, `std::thread::scope` over `available_parallelism()`). Tooling (**signed off, shipped v0.0.63**): `flac -t -s` for FLAC (authoritative, MD5-verifies the decoded audio, catching bit-rot a plain decode misses) and `ffmpeg -v warning -i … -f null -` for the rest (strict decode to a null sink). Both shell out the `rsgain` / `ffprobe` way; the libmpv-reuse alternative was rejected because a player decoder is lenient by design. Classify each CORRUPT (tool error / non-zero exit, or a FLAC that decodes fewer samples than declared), SUSPECT (decoded to the end but the tool flagged trailing data / concealment), METADATA (a real container/tag note survived), or OK, the Lattice tiers, with a benign-note allowlist so an mp3's "estimating duration" warning does not flag a clean file. The classifiers are pure + unit-tested.
- [x] Persist results (migration 0014 `verify_results`: verdict + `checked_at`, keyed by **path** + `file_size`/`file_mtime`) so a re-verify skips unchanged files (`--force` overrides); path-keyed (not track-keyed) so podcasts/audiobooks reuse the table later. `corrupt_or_suspect` is the report read; a GUI list comes later.
- [x] CLI: `verify <db> <query> --root [--verbose] [--force]`, the replaygain-scan selector idiom, with a non-zero exit only when CORRUPT files exist (the Lattice contract), so it is scriptable in a cron/backup hook. Up-front availability guards for the decoders the selection needs.
- [x] Tests: an availability-gated integration test (clean FLAC fixture → OK, runtime-truncated copy → CORRUPT) + the cache round-trip / overwrite through the worker; pure classifier units for both tools, including the benign-note regression. End-to-end verified against the real `testdata/albums/` (clean → all OK, a truncated mp3 → CORRUPT + exit 1, re-run skips the cached files).

*Usable artifact:* `conservatory-cli verify <db> <query> --root <root>` reports library corruption with the same conservative tiers as Lattice (v0.0.63).

### Phase 8b — Duplicate detection

Modeled on Lattice's `--duplicates` (four-section report).

- [x] A four-tier dupe report (`conservatory-core/src/dedup.rs`, v0.0.64): exact albums (`(norm artist, norm album)` in >1 folder), within-album multi-format (`(track_no, norm title)` in >1 file extension), fuzzy similar-name candidates (a hand-rolled difflib `SequenceMatcher.ratio()` port over the loose album key, threshold 0.85, skipping exact-tier pairs), and track-level cross-album (`(norm artist, norm title)` across ≥2 albums, clustered by duration Δ ≤ 2 s so a studio and a live take surface apart). Normalization mirrors Lattice exactly: NFKC (the new `unicode-normalization` dep, signed off), quote/dash folding, whitespace collapse, lowercase; the loose key adds `feat.`/`ft.`/`featuring` and trailing-paren stripping. DB-canonical: a Lattice "directory" maps to an album (the managed folder); one `dedup_rows` read feeds all four tiers.
- [x] CLI: `duplicates <db> [--tier exact|multiformat|similar|tracks ...] [--format human|tsv|json]`. Report only (cleanup goes through `organize`, the Phase 2c mover); exit 0.
- [x] Tests: each tier against a planted `DedupRow` set (album in two folders → exact; flac+mp3 → multi-format; "Album" vs "Album (Remastered)" → similar; one recording in two albums, with a live take splitting off by duration → tracks); the `norm_key`/`loose_key` Lattice vectors; the difflib `ratio` parity (`ratio("abcd","abce") == 0.75`). End-to-end on the real `testdata/albums/`: a clean two-album library reports zero (no false positives). **Note: the importer is album-aware (`get_or_create_album` reuses by artist+title and refuses a path conflict), so an on-disk exact duplicate cannot arise from a plain re-import; the tiers are proven by the unit fixtures.**

*Usable artifact:* `conservatory-cli duplicates <db>` finds duplicate albums and tracks across the managed library (v0.0.64).

### Phase 8c — Library health audits + statistics

Modeled on Lattice's `--auditTags` / `--auditBitrate` / `--auditReplayGain` / `--missingArt` / `--auditArtQuality` / `--stats`. Sliced into **8c-i** (audits, shipped v0.0.65), **8c-ii** (statistics), **8c-iii** (stray-APE detect + byte-level strip).

- [x] **8c-i (v0.0.65):** Audits in `conservatory-core/src/audit.rs` (pure where possible, sibling of `verify.rs`/`dedup.rs`): missing critical tags (title / artist / track number / genre), bitrate below a floor (default 192 kbps, lossless formats skipped so they are never false-flagged), ReplayGain coverage per album (missing / partial / no-album-gain / ok). The Opus `R128_*` convention is recognized via a lazy targeted lofty read of only the Opus tracks whose DB gain is NULL (`tags::read_r128_presence`), so an R128-only Opus is not flagged missing when `--root` is given. Missing cover art (NULL `cover_path`, plus a recorded-but-absent file when `--root` is given) and low-resolution cover art (a pixel floor, default 500×500, decoded header-only from the cover file via `image`). Two dedicated reads (`audit_track_rows` / `audit_album_rows`) feed all tiers.
- [x] **8c-ii (v0.0.66):** Library statistics in `conservatory-core/src/stats.rs` (pure aggregation + a file-size stat pass): overview (track / album / artist totals, total size, duration, fully-tagged percent), per-format counts with size and percent, bitrate average / range / below-floor, rating distribution (0 = unrated per Conservatory's default, 1..5 stars), genre distribution + per-genre rating tally, and top artists. DB-canonical via `stats_track_rows` / `stats_genre_rows` + `library_counts`; the one fact the schema does not store, file size, comes from a `stat()` pass that needs `--root` (sizes read "n/a" without it). CLI `stats <db> [--root R] [--top N=15] [--format human|tsv|json]`.
- [x] **8c-iii detect (v0.0.67):** Detect MP3s carrying stray APEv2 tags (they shadow ID3 in foobar2000 / DeaDBeeF and silently defeat tag edits); report-only, folded as an `ape` tier into `audit` (`audit --tier ape --root R`). The core `conservatory-core/src/ape.rs` is the hand-rolled byte parser: footer-anchored `locate_ape` / cheap-tail `has_ape`, with a reserved-zero + header-consistency guard against a stray `"APETAGEX"` in audio. (The byte strip itself is the next commit.)
- [x] **8c-iii strip (v0.0.68):** The **fix**, a mutating `apestrip <db> --root R [--apply] [--undo]` verb. Dry-run by default (previews what would be stripped); `--apply` removes each stray APE with a crash-safe write (sibling temp + fsync + a lofty decode check + atomic rename) and journals the excised bytes in `ape_strips` (migration 0015) *before* touching the file, so `--undo` re-splices them exactly (guarded by the stripped-state size so a file edited since is skipped). lofty cannot strip APE on MPEG (the Phase 5b deferral); the byte surgery lives in `conservatory-core/src/ape.rs`. Optional APE→ID3 migration deferred (the DB is source of truth and `embed-tags` rewrites canonical ID3). **Phase 8c complete.**
- [ ] (Minor) Rating normalization across player conventions on read (POPM scale differences between WMP, foobar2000, and DeaDBeeF), the Lattice `tags.py` / `rerate.py` lesson, so imported ratings land consistently on the 0 to 5 scale.
- [x] **8c-i:** CLI `audit <db> [--tier tags|bitrate|replaygain|art|artres ...] [--root R] [--bitrate-floor N] [--min-art-px N] [--format human|tsv|json]` (read-only, exit 0; the FS / R128 tiers degrade with a printed note when `--root` is absent). `ape` joins in 8c-iii. **8c-ii (v0.0.66):** `stats <db> [--root R] [--top N] [--format human|tsv|json]`.
- [x] **8c-i / 8c-ii:** Tests: each audit tier flagged against a planted set and clean on a good one (inline pure tests + `tests/audit.rs` for the cover-art FS path); stats aggregation unit-tested over planted rows (format / bitrate / rating / genre / artist / fully-tagged) with `tests/stats.rs` for the size pass.

*Usable artifact:* `conservatory-cli audit <db> --root <root>` gives a one-command health report (v0.0.65), and `stats <db> --root <root>` a library summary (v0.0.66); the APE detect+strip (8c-iii) follows.

### Phase 8d — Playlist export / import (.m3u)

Modeled on Lattice's `--playlist` (rule-based smart `.m3u`), bridged to Conservatory's Perspectives (saved searches).

- [x] Export an ad-hoc search expression (or a `vl:NAME` Perspective) to a `.m3u` / `.m3u8` (root-relative paths by default, `--absolute` for root-joined), and import an existing `.m3u` straight into the queue (append, or `--replace`), resolving paths back to managed tracks. (v0.0.69)
- [x] CLI: `playlist export <db> '<expr|vl:NAME>' <out.m3u> [--root R] [--absolute]`; `playlist import <db> <in.m3u> [--root R] [--replace]`. Wiring the `vl:NAME` resolver into the shared selector path also lit up `vl:` for `search` / `verify` / `duplicates` / `audit`.
- [x] Tests: the `build`→`parse` path round-trips; e2e exporting `format:mp3` to m3u then re-importing resolves the same track set; missing-path entries are reported, not fatal.

*Usable artifact:* move playlists in and out of Conservatory as portable `.m3u` files. **Phase 8 (library maintenance) complete.**

*Deferred:* import into a Perspective (a Perspective is a dynamic saved *search*; a static imported list does not map to an expression, so the queue is the import target) and a separate static-playlist concept; a GUI playlist surface; podcast/audiobook queue export.

---

## Phase 9 — Listening history sync (optional, off by default)

A peripheral "feel good" addition for the music-and-podcast lifer: scrobble completed plays to an external listening-history service. **Optional and off by default**, it sits late and self-contained so it never blocks the engine work, and it is the deliberate, scoped reversal of the spec §14 no-social/no-cloud stance (recorded there). Local-first is preserved: with sync off the app is unchanged, and **ListenBrainz leads** (open, self-hostable, fits the offline-first rule) with Last.fm as a secondary optional target. Reuses `reqwest` (already in the workspace via `conservatory-podcasts`); no new Rust dependency. Hooks the existing play-completion path (spec §6.4) that already updates play counts.

Split headless-first (the CLI-testable rule): **9a** lands the outbox, the ListenBrainz client, the config, and the CLI (all headless); **9b** wires the engine's play-completion hook and the GUI (the "Sync" prefs + the spawned submitter); **9c** adds Last.fm as the optional second target.

### Phase 9a — Outbox + ListenBrainz client + config (headless, CLI) ✅ (v0.3.1)

- [x] The app-wide secret store moved to core: `CredentialStore` (libsecret via `oo7`) promoted from `conservatory-podcasts` to `conservatory-core::secret`, so the scrobble token and the podcast Basic-auth credentials share one store (the podcast crate re-exports it, public API unchanged). `reqwest` + `oo7` now build into core (scrobbling covers music, the native program).
- [x] The `scrobble_outbox` table (migration 0020): a completed play is snapshotted here first (local-first), so a rename cannot corrupt history and submission needs no join. Worker commands `enqueue_scrobble` / `delete_scrobble` / `bump_scrobble_attempt`; pool reads `pending_scrobbles` / `count_pending_scrobbles`.
- [x] The core `scrobble` module: a service-neutral `Listen`, the pure ListenBrainz `submit-listens` payload builder (unit-tested), the `ListenSubmitter` trait (so the drain loop is tested with a fake), and `ListenBrainzClient` (reqwest, base-URL overridable for self-hosting / wiremock) with token validation. The drain loop (`drain_ready` + `run`) deletes on success, reschedules with exponential backoff (capped hourly) on a transient failure, and parks a permanent one; nothing is lost.
- [x] Config `[scrobble]`: `enabled = false`, `service = "listenbrainz"` (the token stays in libsecret, never the file).
- [x] CLI `scrobble` verb (the headless surface): `status`, `token set/clear`, `flush`, `test`.
- [x] Tests: outbox persists + retries across a simulated offline window then submits; permanent failure parks; a different service's listens are left alone and an empty pass is a no-op; the ListenBrainz client speaks the wire protocol against a wiremock server; the ListenBrainz payload JSON shape; credential-store round-trip (in-memory backend). Config round-trip. (Core 385, podcasts 59 green.)

*Usable artifact:* the `scrobble` CLI enqueues, inspects, and drains listens to ListenBrainz (or a mock), surviving offline, with the subsystem inert when unused.

### Phase 9b — Engine hook + GUI ✅ (v0.3.2)

- [x] The engine's `EndReason::Eof` completion path enqueues a listen for a natural track / episode completion (guarded by an engine `scrobble` flag set from `[scrobble]` via a new `SetScrobble` command); the metadata is resolved once off the writer connection and snapshotted into the outbox in one atomic step (`WorkerHandle::enqueue_scrobble_for` → the new `reads::scrobble_source`). Honours scope: music tracks and podcast episodes only; audiobooks excluded (a 14-hour book is not a "listen"), enforced both at the engine guard and in `scrobble_source` (a book resolves to `None`).
- [x] The GUI spawns `scrobble::run` on its runtime (the `mpris::run` precedent, held by an `AbortHandle` for respawn); a Preferences "Sync" page enables scrobbling, picks the service, and stores + validates the ListenBrainz token (libsecret, never the config file). Enable/service apply on dialog close via `refresh_scrobbling`, which syncs the engine flag and restarts the submitter.
- [x] Tests: the completion hook enqueues exactly once per qualifying EOF (four fixtures → four listens, service + descriptive fields snapshotted), the disabled state is a true no-op (empty outbox), and `scrobble_source` never scrobbles an audiobook (null-host engine integration + a data-layer unit check).
- [ ] **Deferred:** the ephemeral "now playing" update on load. It bypasses the outbox (a live ping to the service, a different transport than the completed-play queue) and is not needed to scrobble; it lands as a small follow-on rather than bloating the completed-play path.

*Usable artifact:* real completed plays scrobble to ListenBrainz from the running app, off by default.

### Phase 9c — Last.fm (planned)

- [ ] Last.fm as the optional second target: session auth + `api_sig` signing, behind the same config / prefs / outbox (the `service` column already routes per-listen).

*Usable artifact:* Last.fm works as an alternative to ListenBrainz. **Tags `0.4.0`** once Phase 19 also lands.

---

## Phase 10 — Configuration & preferences

Conservatory's first `config.toml` (spec §10), at `$XDG_CONFIG_HOME/conservatory/`. **Ownership decision (non-breaking):** the file owns only the app/library-level settings that are not otherwise persisted (`[library]`, `[genre]`, `[podcasts]`, `[audiobooks]` defaults, the `[browse]` facet-pane layout); the audio engine state (ReplayGain / EQ / DSP / output) **stays in the SQLite singletons** the Sound dialog already mutates live. Spec §10's `[playback]`/`[audio]` blocks are DB-canonical for now (a recorded reconciliation, not a migration).

### Phase 10a — Config foundation + library root from config ✅ (v0.0.70)

- [x] `conservatory-core/src/config.rs` (new, pure, glib-free so it stays CLI-testable): a serde `Config` mirroring the owned spec §10 sections, every section `#[serde(default)]` so a partial or absent file loads to the documented defaults and round-trips losslessly. `config_path()` resolves `$XDG_CONFIG_HOME` (else `~/.config`) `/conservatory/config.toml`; `load` (missing file → defaults, malformed → error), `save`, `to_toml_string`. The path-template defaults reuse `DEFAULT_MUSIC_TEMPLATE` / `DEFAULT_AUDIOBOOK_TEMPLATE` so there is one source. `toml` (already a workspace dep) wired into core; no new dependency.
- [x] The GTK binary sources the library root from config: a CLI positional still overrides (dev / tooling), else `[library] root`, else none (a pure `resolve_root` helper, unit-tested). The library root stops being a required CLI arg (the carry-forward note from 4b-ii-a / 4b-ii-c).
- [x] CLI `config` verb (the headless test surface): `config path` / `config show` (effective config as TOML) / `config init` (writes a default file, never clobbers).
- [x] Tests: default round-trip, partial-file merge, missing-file defaults, explicit-field parse, malformed error, `config_path` honours `XDG_CONFIG_HOME`, and the GTK `resolve_root` precedence. E2e verified in a scratch `XDG_CONFIG_HOME`.

*Usable artifact:* launch `conservatory` with no arguments and it finds the library from `config.toml`; inspect/init the config from the CLI.

### Phase 10b — Preferences: General + Library pages ✅ (v0.0.71)

- [x] Generalized the Sound `AdwPreferencesDialog` (`conservatory/src/ui/window.rs`, now `open_preferences`, dialog titled "Preferences") with a **General** page (library root chooser via `gtk::FileDialog::select_folder`, music path template, import mode, embed-tags-on-edit, genre default) and a **Library** page (podcast subdir + max downloads; audiobook subdir + path template + default speed + Smart Speed + Voice Boost). Both read and write `config.toml` through the 10a loader (`config::load_default` on open, `save_default` on dialog close); the existing Sound page (EQ / RG / DSP / output) keeps persisting to the DB singletons, untouched. The header button + `Ctrl+,` now open Preferences on the General page.
- [x] Apply timing surfaced honestly: the library root applies on the next launch (the running session holds the root set at startup); a group description says so.
- [x] Tests: the `import_mode` ↔ combo-index round-trip (the one non-trivial projection); the dialog build is manual (the 3b/3c precedent), saving through the 10a-proven `save_default` path.

*Usable artifact:* configure the app (library root, import defaults, podcast/audiobook defaults) from Preferences, not a hand-edited TOML or CLI args. The Library page is built to receive the facet-pane configuration in 10c.

### Phase 10c — Configurable facet panes (1 to 5) ✅ (v0.0.73)

- [x] The §3.2 promise: the browse panes are built from `[browse] panes` (a field per pane, order, 1 to 5) instead of the hard-coded Genre → AlbumArtist → Album. The `FacetField` enum is generalized from three variants to seven (Genre, Shelf Genre, Album Artist, Artist, Album, Year, Format), each with a config key / title / plural and `filter_sql` + `target_sql` arms; `panes_from_config` resolves the keys (unknown dropped, capped at 5, default hierarchy when empty). The GTK window builds N panes from config (`build_pane(field)`); the cascade, `imp.panes`, and coalescer were already N-pane generic.
- [x] Edited from the 10b Library page: a **Browse panes** group of five ordered field/`(none)` `ComboRow`s; the non-empty slots become `[browse].panes`. Applies on the next launch (no live rebuild of the central browse widget; matches the 10a/10b precedent), surfaced in the group description.
- [x] Tests: the key round-trip + `panes_from_config` (skip/cap/default) as pure units; a fixture-DB test that the new single-valued facets partition all tracks and the cascade narrows a new-field pane. The editor + config-driven build are build + manual (the 3b/3c precedent). E2e: a custom `[browse].panes` round-trips through `config show`.
- [ ] **Deferred:** live pane rebuild on prefs close; `Composer`/`Work` facets (need new schema); `Rating`/`Added` facets (easy follow-ons).

*Usable artifact:* reorder, add, or remove browse panes (up to five, choosing the field per pane), persisted across launches. **Phase 10 (Configuration & preferences) complete.**

---

## Phase 11 — Browse & player polish (Columns UI parity)

The finishing pass that brings the music surface up to the deadbeef / foobar2000 Columns UI the browse is modeled on (spec §3.2, §3.3): the side panels, the chrome, and the player conveniences a daily driver is expected to have. Each piece is small and self-contained, GTK-side over logic that already exists in core, so they ship independently and none blocks the others. Modeled on the reference deadbeef layout (the cover-art + properties + status-bar furniture around the central facet/track view).

### Phase 11a — Track properties inspector + cover-art panel ✅ (v0.0.72)

- [x] A **properties / metadata inspector** for the selected track (the deadbeef `selproperties` widget): title, artist, album, year, genre, track/disc, duration, format, bitrate, sample rate, file size, ReplayGain, rating, plays, last played, added, location, MusicBrainz ids, cover file. Read-only; all from the DB (`tracks`/`albums`) or a cheap `std::fs` stat (file size). A right-docked collapsible `gtk::Revealer` (the queue-drawer twin), not a modal; toggled by a header button and `Ctrl+P`; refreshed on selection change (a no-op while closed).
- [x] A **large cover-art panel** atop the inspector (the deadbeef `coverart` widget): the album art at 240px from `albums.cover_path` (Phase 5d), accent-tinted via the display-wide CSS-class technique (the Hermitage unit), distinct from the small Now-bar thumbnail; a placeholder when there is no cover.
- [x] Tests: the pure `inspector_fields` projection (a `Track` + `Album` → the displayed rows, skipping empties), mirroring `now_playing_panel::track_fields`; the panel build is manual (the 3b/3c precedent).
- [ ] **Deferred:** channels (not a stored column; needs a schema/importer change or a per-selection decode) and a multi-select aggregate (the inspector shows the first selected track).

*Usable artifact:* select a track and see its full technical metadata and a large cover, as in the deadbeef layout.

### Phase 11b — Status bar + play-status glyph column ✅ (v0.0.74)

- [x] A **status bar** (spec §3.2 footer): the playing track's format / sample-rate / channels (channels read live from mpv, not a stored column), plus the active view's track count and total playtime (the deadbeef "N tracks, D total playtime" line), switching to the selection's total when 2+ rows are selected. A thin bottom bar above the Now-bar; the aggregate is computed from the already-loaded leaf set (`TrackBrief` carries duration), not a re-query.
- [x] The **play-status glyph column** (the leftmost ♫ in the deadbeef track list): a per-row playing / paused indicator. This is the item **explicitly owed from Phase 3c** ("the per-row playing/status glyph waits for playback state, Phase 4"); Phase 4 shipped the playback state, so it is now unblocked. Driven by the engine snapshot's current item (a symbolic icon, no font assumption). `TrackRow` gained a `playing` glib property so the glyph cells bind `notify::playing` and only the affected rows repaint when playback moves (no full-store rebind on a 50k-track library).
- [x] Tests: the aggregate + playtime/thousands formatting, the technical line, and the glyph-state selection are pure units (`statusbar.rs`); widgets build + manual.
- [x] **Pulled forward at v0.0.38 (playback feedback):** the snapshot gained `kind` / `streaming` / `buffering`, so the Now-bar shows a **buffering spinner** (mpv `core-idle`) and a **streaming glyph** for an undownloaded episode, and the Podcasts episode list gained a **downloaded vs stream-only** glyph column. (The full status-bar line and the in-list play-status glyph above are still to do.)

*Usable artifact:* the browse window shows the playing row at a glance and a foobar-style status line.

### Phase 11c — Now Playing surface (bottom drawer)

- [x] **A bottom Now Playing drawer landed at v0.0.38** (the lighter realization of the spec §3.6 surface, chosen over a full-bleed takeover): a slide-up `gtk::Revealer` above the Now-bar, the horizontal twin of the right-docked queue drawer, opened by clicking the Now-bar cover/title or `Ctrl+I`. It shows the current item's full metadata (track: format / bitrate / sample rate / ReplayGain / path / rating / plays / album / year; episode: show / date / runtime / size / source stream-or-local / notes), refreshed as the queue advances. The pure field projection is unit-tested. **The drawer's content area is the intended home for the spectrum visualizer** (the deferred item below).
- [x] **The richer surface (v0.0.75):** a full-bleed accent-tinted cover (the larger twin of the Now-bar thumbnail, reusing the inspector accent-class technique), an accent-tinted scrubber (a draggable seek `Scale` under the title, seeking through the shared handle), a queue-tail "Up next" peek (the next items from `load_queue_display`, refreshed on track change and queue edit), and a track audio-engine state line (active EQ preset / enabled DSP modules / gapless, from the `eq_state` + `audio_state` singletons). Chapters / Smart Speed indicator / sleep timer were already wired in with Phase 6c, so they were not rebuilt here.
- [x] Tests: the audio-engine line (`audio_state_line`) and the queue-tail slice (`upcoming`) are pure units; the cover / scrubber / up-next widgets are build + manual.

*Usable artifact:* clicking the Now-bar (or `Ctrl+I`) slides up a Now Playing drawer with the current item's full surface: a large accent cover, a draggable scrubber, the live metadata, the audio-engine state, and a queue-tail peek, across all media types.

### Phase 11d — Transport conveniences ✅ (v0.0.76)

- [x] **Stop-after-current** (the deadbeef `toggle_stop_after_current`, the user's `Ctrl+M`): the engine finishes the current item, then pauses at the boundary instead of playing on, and disarms. A `stop_after_current` flag on the engine + a `SetStopAfterCurrent` command + a snapshot field, consulted at the EOF boundary alongside the `EndOfItem` sleep mode (the shared precedent).
- [x] **Jump-to-current-track** (the user's `Ctrl+J`): select and scroll the browse list to the playing track (a no-op when it is an episode / book or filtered out of the view), via the pure `current_row_index` + `ColumnView::scroll_to`.
- [x] Both as menu actions (a new header primary menu, the stop toggle stateful with a checkmark) and keybindings (`docs/keymap.md`; `Ctrl+M` / `Ctrl+J` reassigned from the proposed Mute / jobs-surface slots), the spec §3.1 "every action visible and keyboard-accessible" rule.
- [x] Tests: the stop-after-current flag gates the advance (engine null-host integration, beside the `EndOfItem` test); the jump resolves the current row index (pure unit).

*Usable artifact:* **Columns UI parity.** The music surface matches the deadbeef daily-driver layout: side panels, status bar, an expanded Now Playing view, and the expected transport conveniences. **Phase 11 (Browse & player polish) complete.**

### Deferred (recorded, not built)

- [ ] **Spectrum visualizer** (the deadbeef `spectrum` widget): a real-time frequency-bar analyzer. Captured here at the user's request; it needs an audio-tap off the libmpv output (an `af` data sink or a visualizer hook). **Pulled forward into Phase 12d** (the user asked for it as part of the visual overhaul); its home is the v0.0.38 Now Playing drawer (11c).

## Phase 12 — Visual identity & album-art-forward UI (the "life" overhaul)

The shipped UI was flagged as "dull and lifeless": no palette (the flat grey system theme), no album art in the music browse, an info-thin playback bar. Research compared us to the user's DeaDBeeF, Cozy, and Amberol. Decisions (user-confirmed): a fixed Kanagawa Dragon palette with the per-album accent tinting highlights only (not Amberol's full adaptive recolour); a cover column plus a large docked cover panel (the deadbeef `coverart` layout, no new album-grid view); the spectrum visualizer in scope, a blurred cover background deferred (needs a `GskBlurNode` render widget, no GTK4 CSS route). See `docs/theme.md`.

### Phase 12a — Kanagawa Dragon theme + centralized accent ✅ (v0.0.77)

- [x] The Kanagawa Dragon palette (Dragon variant) mapped onto libadwaita's named colours via `@define-color` in `main.rs`, and the dark scheme forced (`AdwStyleManager` `ForceDark`). The whole app is warm-dark with the dragonRed (`#c4746e`) accent instead of flat grey. Mapping documented in `docs/theme.md`.
- [x] Lifted album-cover cards: a 10px radius + Amberol-style drop shadow (`.cover-art` and the per-surface cover variants), and the previously-styleless `.now-bar-cover` filled in.
- [x] A centralized accent helper, `ui/accent.rs` (`AccentProvider` + `apply_cover_ring`): one display-wide-provider technique (the non-deprecated route to dynamic per-item colour) for the 2px accent ring layered over the drop shadow. The inspector is migrated onto it (de-dups the first of the three copies); the browse covers, Now-bar, and Now Playing surfaces adopt it in 12b/12c. Pure helpers (`accent_class`, `cover_ring_css`) unit-tested.

*Usable artifact:* the app finally has a visual identity. Launch and it is unmistakably Kanagawa Dragon, with shadowed cover cards, before any structural change.

### Phase 12b — Album art in the browse (cover column + cover panel) ✅ (v0.0.78)

- [x] `TrackBrief` + `facet_tracks` gain `cover_path` / `accent_rgb` (the `albums` join already existed, so a one-line SELECT add); threaded through `query_leaf` (a single change point feeds both the SQL-fast and in-memory-filter paths). A core read test asserts the projection.
- [x] A 40px cover-thumbnail column leads the track list, rounded via `.cover-thumb` + `overflow: hidden`, decoded once through a shared downscaling texture cache (`ui/covers.rs`, `CoverCache`) so a large library does not re-decode on scroll. No per-row accent ring (a single display-wide provider cannot serve N row accents, and a ring on a 40px thumbnail is negligible); the ring stays on the big surfaces.
- [x] The Phase 11a inspector promoted into the large cover panel: cover 240→300px and **open by default** (the deadbeef `coverart` + `selproperties` right column). `Ctrl+P` still toggles it.

*Usable artifact:* the music browse shows album art, a thumbnail per row plus a large cover panel open by default. The downscaler is the GTK stack's own gdk-pixbuf (no new dependency).

### Phase 12c — Now-bar enrichment + Now Playing polish ✅ (v0.0.79)

- [x] The Now-bar cover grows 40→56px in an accent-ringed frame; the secondary line reads `artist · album` (folding the duplicate for a podcast, via the pure `now_bar_subtitle`); the seek fill takes the album accent (the `now-bar-seek > trough > highlight` rule, the now-playing scrubber idiom). The playing item's accent rides the existing `NowPlaying` metadata read (a new `album_accent_rgb`, populated for track / episode / book).
- [x] Now Playing drawer breathing room: cover 132→160px, larger gaps and margins.

*Usable artifact:* the playback bar is informative and album-art-led; the Now Playing drawer reads as composed.

### Phase 12d — Spectrum visualizer ✅ (v0.0.80)

- [x] **Spike outcome:** libmpv has no PCM tap or audio callback (confirmed in the mpv source + `docs/libmpv-profiles.md`), and its metadata filters give levels, not FFT bins. The user chose the faithful FFT spectrum, so it taps **outside** libmpv, at PipeWire.
- [x] Pure DSP in core (`conservatory-core/src/player/spectrum.rs`, `realfft`): a Hann-windowed real FFT → log-spaced frequency bands → dB-normalized levels, plus a fast-attack / slow-decay `SpectrumSmoother`. Unit-tested headless (a 1 kHz tone lands in the right band; silence is flat; the smoother rises fast, decays slow).
- [x] Capture thread in the binary (`conservatory/src/viz.rs`, `pipewire`): a capture stream, never altering playback, downmixed to mono and analyzed, publishing bands to a shared buffer. *Originally a default-sink-monitor tap (saw all system audio); v0.1.1 retargeted it to Conservatory's own mpv output node (`target.object` = the player's `audio-client-name`), gated on playback so it never falls back to the microphone.*
- [x] Widget in the binary (`conservatory/src/ui/spectrum.rs`): a `gtk::DrawingArea` in the Now Playing drawer, redrawn on a **frame-clock tick** (display rate, independent of the engine's ~10fps snapshot), drawing accent-coloured bars. Capture starts on map (drawer opens) and stops on unmap, so it is free when closed.

*Usable artifact:* a smooth, accent-coloured real-time frequency spectrum in the Now Playing drawer. **Phase 12 (Visual identity & album-art-forward UI) complete.**

## Phase 13 — Sleekness pass, layout fix, and code tidy

A UI/UX polish pass plus a focused code tidy, prompted by a concrete layout bug (a closed side panel not giving its space back) and a request to make the app feel sleek. A code-quality audit found the codebase already clean (no dead code, good comments, solid error handling), so the tidy is a focused dedup, not a sweep.

### Phase 13a — Layout space-reclaim fix + quick CSS/spacing wins ✅ (v0.0.82)

- [x] **The bug:** closing the inspector / queue Revealer left an empty gap; the content Box had no `hexpand` child to absorb the freed width. Fixed by making the browse `body` (and the content Box) the horizontal expand-sink (`window.rs`).
- [x] Explicit Revealer transition durations (queue / inspector / Now Playing) for consistent motion.
- [x] Animated hover feedback on track rows, column headers, chapter rows, and the sleep-timer menu (CSS).
- [x] Opened up the cramped 2px grids/lists (track-properties, Now Playing detail, chapters, up-next).
- [x] CSS polish: layered cover shadow, accent-tied text selection + focus ring, rounded scrollbar sliders.

*Usable artifact:* closing a side panel reclaims its space, and the app reads more polished (smoother motion, breathing room, depth, accent).

### Phase 13b — Structural UX: empty states, header grouping, toasts ✅ (v0.0.83)

- [x] `adw::StatusPage` empty/idle states: the leaf table swaps to a centered status page when the library is empty ("No tracks yet") or a filter has no matches ("No matches"); the Now Playing drawer shows a "Nothing playing" page when idle.
- [x] Clustered the header buttons into linked groups (panel toggles | edit/embed | utility) for visual hierarchy.
- [x] `adw::ToastOverlay` feedback on tag-embed and bulk-edit (the embed "Done" modal became a toast). Import / playlist export are CLI-only, so no GUI toast applies there.

### Phase 13c — Code tidy: accent + helper dedup ✅ (v0.0.84)

- [x] Finished the Phase 12a accent-provider migration: `now_playing_panel` and `audiobooks` both route through the shared `ui/accent.rs` `AccentProvider` (the audiobooks shelf hands it an N-rule CSS string; the provider swap is the same), so the three inline copies are gone.
- [x] Consolidated the duplicated `push()` and the four `*_fields()` projections (`track`/`episode`/`book`/`inspector`) into a new `ui/fields.rs`, tests moved with them.
- [x] Confirming pass: the audit found the codebase otherwise clean (no dead code, good comments, solid error handling), so no churn was manufactured. Documented rather than invented.

### Phase 13d — Typography: bundled OFL fonts per UI role ✅ (v0.0.85)

- [x] Bundled three SIL OFL fonts in `data/fonts/` (with per-family `OFL.txt`): Inter (base UI), Fraunces (headers), IBM Plex Mono (technical fields). ~1.5 MB total.
- [x] Registered them at startup with no host-font assumption (spec §7.2.9): `register_bundled_fonts()` writes a fontconfig file including the system config plus the bundled dir and sets `FONTCONFIG_FILE` before GTK lays out text (pango v1_56 `add_font_file` is unavailable on the 0.20 stack). No new dependency, no version bump of the GTK stack.
- [x] Applied per role in CSS with generic fallbacks: Inter on `window`/`popover`/`dropdown`/`tooltip`, Fraunces on the title/heading classes, a new `.tech` class for IBM Plex Mono on the path / id property rows (`ui/fields::is_tech_field`) and the status-bar technical line.
- [x] Packaging + docs: a `meson.build` font install target, `ATTRIBUTIONS.md` font entries, and a Typography section in `docs/theme.md`.

## Phase 13e — deadbeef-cui column-browser parity (interaction + shortcuts)

Research found the per-column `[All (N)]` item and track double-click-to-play already match deadbeef-cui, so this phase fills the two real gaps: facet activate-to-play and the unwired keyboard shortcuts.

### Phase 13e-i — Facet activate-to-play ✅ (v0.0.86)

- [x] Double-click / Enter on a facet value (a genre, an artist) plays its filtered set, the deadbeef-cui `activate_row()` move. The facet panes previously only wired `selection_changed` (filter cascade); now the pane `ColumnView` has `connect_activate` → `on_facet_activated(i)`, which flushes the debounced cascade (`recompute_from`) so the leaf reflects the clicked row, then plays it from the top (shared `play_leaf_from` extracted from `on_track_activated`). The `[All]` row plays everything under the other panes.

### Phase 13e-ii — Keyboard shortcuts: playback, navigation, conveniences ✅ (v0.0.87)

- [x] Wired the researched playback / navigation keys: Space (play/pause, via a capture-phase key controller that yields to text entry, the foobar2000 rule), Ctrl+→/← (next/previous), Ctrl+↑/↓ (volume ±5), Ctrl+0 (mute toggle, `pre_mute_volume` remembers the level), Ctrl+L (clear filter), Ctrl+Q (quit). All reuse existing `PlayerHandle` methods.
- [x] Deliberately skipped (documented, not silent): bare/Shift arrow seek (conflicts with list navigation; deadbeef-cui does not bind it either) and Delete = remove-from-library (destructive, needs a confirmation flow).

### Phase 13e-iii — Shortcuts reference + keymap doc ✅ (v0.0.88)

- [x] `F1` (and a "Keyboard Shortcuts" header-menu entry) opens a grouped, curated shortcuts reference, built as an `adw::PreferencesDialog` rather than the deprecated `gtk::ShortcutsWindow` (and `AdwShortcutsDialog` postdates the project's libadwaita), so it stays current and inherits the app typography.
- [x] `docs/keymap.md` brought in line with reality: the now-live playback keys marked wired, facet activate-to-play documented, and the still-deferred keys (arrow seek, save-Perspective, remove-from-library, jobs) honestly flagged. **Phase 13 (sleekness, layout, tidy, typography, browser parity) complete.**

## Phase 14 — Debug mode & observability

A `--debug` firehose across both binaries, 0.1.0-prep: SQL, IO, network, and memory to stderr on four filterable channels (`conservatory::{sql,io,net,mem}`). No new dependencies (rusqlite's `trace` feature is already enabled; memory via `/proc/self/status`).

### Phase 14a — The debug spine: SQL + memory + the mode ✅ (v0.0.89)

- [x] New `conservatory-core/src/debug.rs`: an `ENABLED` flag (`set_enabled`/`enabled`) the binaries flip for `--debug`, gating the costly hooks so a normal run pays nothing; a `rusqlite` profiler (`install_sql_profiler`) wired into both `open_writer`/`open_reader` that logs every statement + timing to `conservatory::sql`; `/proc`-based RSS (`rss_kb`, `log_memory`) and a 5 s `spawn_memory_sampler` to `conservatory::mem`.
- [x] Both binaries: `--debug` calls `set_enabled(true)`, routes output to stderr, and raises our crates + the channels to `debug` (`RUST_LOG` still narrows). The CLI gained the global `--debug`/`-d` flag it lacked; the GUI samples memory at startup, library-loaded, and every 5 s.
- [x] De-flaked the suite properly: `after_timer_fires_pauses_and_tap_extends` (a real-time engine countdown, flaky under heavy build load) is now `#[ignore]`d from the default gate, its behaviour covered deterministically by the `player::sleep` unit tests and the boundary sibling integration tests; run on demand with `-- --ignored`.

### Phase 14b — IO + network instrumentation ✅ (v0.0.90)

- [x] `conservatory::io` events at every filesystem mutation: the mover (`fsops.rs`: rename, cross-device copy + fsync + rename, copy revert), cover writes, tag write-back, APE strip, the import scan summary, podcast download and retention delete, and the CLI playlist / OPML export. A `--debug` import prints a line per file moved with byte counts (verified live).
- [x] `conservatory::net` carries all HTTP: the feed fetcher's three events retargeted onto the channel, plus new GET/wrote events in episode download and chapter fetch. Confirmed nothing else in the workspace makes network calls.
- [x] New `docs/debugging.md` (the flag on both binaries, the four channels, `RUST_LOG` narrowing, the `/proc` memory note, the sleep-test caveat); README gains a "Debugging" pointer and a Phase 14 status line.

## Phase 15 — 0.1.0 readiness gate (quality, no new features)

The bar for tagging **0.1.0**: a quality and confidence milestone, not a feature phase (Phase 9 scrobbling stays deferred past 0.1.0). Nothing here adds capability; it proves that what is built is safe and within budget and that the release scaffolding is sound. Each item is a verification with a recorded result, not a code change; a fix only lands if verification turns up a gap.

### Phase 15a — Move-safety release-blockers, proven (spec §5.4)

The headline gate. A move bug damages a real library, so this is the item that actually blocks the tag. The dry-run preview, undo journal, and crash-safe replay already carry unit and integration coverage; this is the end-to-end confidence pass.

- [x] Dry-run preview accuracy: run an import / organize dry-run against a real library copy and confirm the previewed moves match what an `--apply` would do (no surprise relocations; genre and path-template output correct). **(v0.0.91: against the two committed real albums, the dry-run listed exactly the 6 track moves `--apply` then executed; shelf-genre and path-template output correct.)**
- [x] Undo journal round-trip: `--apply` a move, then undo, and confirm byte-identical restoration to the original layout (checksums, not just paths). **(v0.0.91: SHA-256 manifest confirmed byte-identical after undo. This pass found a real gap: `organize --undo` reverted the track moves but left the album cover stranded with a stale `cover_path`, because covers are not journaled and the undo branch, unlike apply, did not re-sync them. Fixed by mirroring apply's idempotent `resync_album_covers` call; regression test `cover_resyncs_back_on_undo`.)**
- [x] Crash-safe roll-forward: simulate a crash mid-move (kill between the file op and the journal-complete write) and confirm replay completes the operation idempotently with no data loss. Audit whether an automated test covers this exact path; write one if it does not. **(v0.0.91: `crash_mid_job_rolls_forward_on_recovery` already covers this exact path: journal written, files relocated without the completion marker, `recover()` rolls forward idempotently with tree and DB consistent. No new test needed.)**
- [ ] Run the above against Brandon's **real music library** (a working copy), not only synthetic fixtures, since the real library is the actual risk surface. **(Deferred by decision: synthetic-only verification chosen for 0.1.0. The two committed real albums stand in; the full-library pass is a tracked pre-1.0 item below.)**

### Phase 15b — Memory budget, measured (spec §13)

Phase 14's `--debug` RSS sampling is the instrument; this turns the budget from an assumption into a number.

- [x] Load a large library (Brandon's real one, or `debug-fixture --scale large`) and read RSS via `--debug` at **idle** (target under 200 MB) and **active playback** (target under 300 MB on roughly 50k tracks). **(v0.0.91, release build: idle on the 12k fixture ~196 MB; active playback ~187 MB, playback adding no measurable RSS over the warm floor. The floor is dominated by the GTK+libmpv base; per-track scaling is gentle, ~9 MB across 12k tracks.)**
- [x] Record the numbers (patchnotes or a note). If either exceeds budget, file the overage as a pre-0.1.0 fix; do not tag over budget. **(Recorded in v0.0.91 patchnotes. Both within budget at 12k. Caveat: the synthetic fixture tops out at 12k, so the 50k idle target is unverified; idle sits at the 200 MB line at 12k and extrapolates to ~215–230 MB at 50k. Tracked pre-1.0 item below: confirm/optimize idle RSS at 50k before the `v1.0.0` tag.)**

### Phase 15c — Release scaffolding sanity

Mostly already true; a confirm pass, not new work.

- [x] `LICENSE` (GPL-3.0-or-later) and `ATTRIBUTIONS.md` present and current; the GPL chain analysis still matches the linked libraries. **(v0.0.91: confirmed. Also removed a dead `id3` entry from the workspace dependency catalog: unreferenced by any crate, not compiled or linked, so ATTRIBUTIONS correctly omitted it.)**
- [x] CI green on both the default build and `--no-default-features` (music-only); `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all clean. **(v0.0.91: all green on both feature sets, after the 15a fix.)**
- [x] README, spec.md, and docs/ confirmed current (the v0.0.90 staleness sweep did this; re-confirm nothing regressed before the tag). **(v0.0.91: VERSION matches the workspace `Cargo.toml`, README links the `--debug` docs, all 12 `docs/` references present; no regression.)**
- [x] Decide the Flatpak / Meson packaging question for 0.1.0: is a working installable build part of *this* tag, or a 0.1.x follow-on? **(Decided: a 0.1.x follow-on. The 0.1.0 tag rests on the quality/safety gate, not on a shipped installable build.)**

### Tracked pre-1.0 items (surfaced by the Phase 15 gate)

- [ ] **Idle memory at 50k:** confirm the spec §13 idle target (< 200 MB) on a real ~50k-track library, since the synthetic fixture tops out at 12k and the 12k idle (~196 MB) extrapolates to ~215–230 MB at 50k. If over, optimize before the `v1.0.0` tag.
- [ ] **Full-library move-safety pass:** run the 15a move/undo/crash checks against a working copy of Brandon's real library (the synthetic-only pass covered 0.1.0).

*Usable artifact:* a `v0.1.0` tag that the move logic, the memory budget, and the release scaffolding have all been verified to earn, with the numbers recorded. **(Cut at v0.1.0. The gate was exercised and recorded at v0.0.91: the synthetic passes hold and the one move-safety bug found is fixed. Packaging is dropped from the gate and the two real-library items above are post-0.1.0 follow-ons, not blockers, by decision.)**

---

## Phase 16 — Power-user interaction layer

From the v0.1.2 UI/UX deep-dive: a competitor study against MusicBee, foobar2000, Roon, Plexamp, Quod Libet, and Navidrome, plus the local GTK apps (amberol, g4music, gnome-music, lollypop, deadbeef). The finding was that Conservatory already meets roughly seven of the top eight power-user discriminators (database-owned library, unified interleaved queue, Calibre-grade search grammar, folder-level cover cache and accent, write-back-to-files, the dry-run/undo/crash-safe mover, the labelled ReplayGain chain), and the remaining gaps are interaction, not architecture. This phase closes the biggest ones. The browse surface stays facet-columns only (no album-grid view, by decision); playlists grow to three crisp primitives (Perspective, Smart Playlist, Static Playlist).

### Phase 16a — Context-menu spine + Play Next + Remove from Library ✅ (v0.1.3)

Right-click context menus, the single most glaring gap (before this the only pointer gesture in the app was the Now-bar tap), across all five browse surfaces, plus the two verbs that needed new engine and database plumbing.

- [x] Track-list menu: Play, Play Next, Add to Queue, Edit…, Rating ▸ 0–5, Reveal in Files, Remove from Library. A reusable `RowContextFn` per-cell secondary-click gesture (a `ColumnView` exposes no per-row widget, so the gesture reads `item.position()` at click time and survives re-sorts); right-clicking an unselected row selects just it first.
- [x] Facet-pane menu: Play / Play Next / Add to Queue over the facet's narrowed set (the popover re-parents to the clicked pane, since the panes are distinct `ColumnView`s).
- [x] Queue-drawer menu: Remove from Queue / Clear Queue, reusing the keyboard-op methods.
- [x] Podcast-episode menu (Play, Add to Queue, Mark Played/Unplayed, Star/Unstar, Archive) and audiobook-shelf menu (Play, Add to Queue, Edit…), each a local `gio` action group on the self-contained tab module.
- [x] **Play Next:** a new `PlayerCommand::InsertItems` with a pure, unit-tested `insert_current_index` helper, mirrored in the database queue by `insert_queue_tracks_at`; it inserts after the current item with the engine and the database queue in lock-step.
- [x] **Remove from Library:** a database-only unlink (the file stays on disk, re-importable) behind a destructive confirm, riding the schema cascades (`queue.track_id ON DELETE CASCADE`, `playback_state.track_id ON DELETE SET NULL`, the `tracks_ad` FTS trigger).
- [x] Tests: `queue_insert_at_shifts_later_positions`, `delete_track_removes_it_and_cascades_the_queue`, `engine_play_next_inserts_after_the_current_item`, and the `insert_at_or_before_current_shifts_it_up` unit test; both feature sets build.

### Phase 16b — Click-to-rate ✅ (v0.1.4)

The rating column's stars are now clickable (Apple's "click in the rating column"), writing a single-track rating through a targeted row repaint rather than a full reload.

- [x] A primary-click gesture on the star row maps the pointer x across the five stars to a 1–5 rating; clicking the current top star clears it to 0 (the Apple toggle). The geometry is a pure `rating_from_click` helper with unit tests, and the press is claimed so it does not also select or activate (double-click play) the row.
- [x] `TrackRow` gains a `rating` glib property (the `playing`-glyph precedent); the star column binds `notify::rating` so a rate repaints only that one row, and `update_rating` keeps the property in step with the `brief` the rating sorter reads. The write goes through `worker.update_track`; the inspector's Rating field refreshes with it.
- [ ] A live drag-sweep across the stars is a natural ergonomic follow-on (the click already sets any value directly, so it is not required).

### Phase 16c — "Mixed values" bulk edit ✅ (v0.1.5)

Upgrade the bulk-edit dialog from "blank means unchanged" to the foobar/MusicBee affordance: a checkbox, label, and entry per field, with the shared value pre-filled and differing selections shown as "multiple values".

- [x] Each field pre-fills the value shared across the selection, or reads "multiple values" when the tracks differ (`bulk_edit_commons` collapses each field to a shared value or `None`; the collapse is the pure, unit-tested `common_value`). Album artist / album / year / shelf genre come from the album, track title / artist from the render row, genres / rating from the leaf briefs.
- [x] Only ticked fields are written, and editing a field ticks it (so a shared value is not silently rewritten). The existing write + move-preview pipeline (`apply_bulk_edit` → `confirm_and_move`) is unchanged.
- [ ] Inline single-click cell editing for the text columns, and clearing a field to empty (a ticked-but-empty field), are follow-ons (empty currently fails the year/rating parse, so a clear needs a dedicated path).

### Phase 16d — Smart + Static playlists

Three crisp primitives, kept distinct to avoid Roon's Tags-vs-Bookmarks confusion: Perspective (the existing live saved query), Smart Playlist (query plus a limit and prioritization, new, a live queue source), and Static Playlist (a frozen curated list, new).

**Architecture note:** core is deliberately search-free at runtime (spec §2.2), so smart-playlist *materialisation* (evaluating the query) lives in the CLI / GUI, which depend on the search grammar; core owns the storage and a pure `ordered_track_ids` SQL order/limit primitive. `random` order is deferred to the Phase 17 shuffle work; the deterministic keys (added / rating / lastplayed / title / artist) ship now.

#### Phase 16d-i — Schema + core storage + CLI ✅ (v0.1.6)

- [x] Migration 0017: `playlists` (id, name, kind, query, limit_n, order_by, created_at) and `playlist_entries` (mirroring the queue's synthetic-id + non-unique-position + multi-kind shape, so the reorder shift transfers cleanly). Core models `PlaylistKind` / `PlaylistOrder` / `Playlist`.
- [x] Core CRUD (create / delete / rename / append / remove-entry / reorder-entry) through the single-writer worker; reads `list_playlists` / `get_playlist` / `static_playlist_track_ids`; the `ordered_track_ids(where_sql, params, order, limit)` SQL primitive (fixed-whitelist ORDER BY, no injection). Unit + integration tests.
- [x] CLI: `playlist create-static | create-smart | add | list | show | delete`, with smart materialisation glue (translate → `ordered_track_ids`, else eval-fallback + best-effort sort). Verified end-to-end against a fixture library.

#### Phase 16d-ii — GUI (Playlists sidebar + rule builder) ✅ (v0.1.7)

- [x] The left sidebar is split into two labeled sections: **Perspectives** on top (the existing saved-search list, its save button already saves the current filter), **Playlists** below. New imp fields + `refresh_playlists` mirror the perspectives pattern; each shares the vertical space.
- [x] Activating a playlist plays it (materialise → replace the queue → play): static via `static_playlist_track_ids`, smart via a new GUI `materialize_smart` in `query.rs` (the CLI's dual-path mirrored, since core stays search-free).
- [x] Create from a `+` menu: **New Static…** (name) and **New Smart…** → a rule-builder dialog (name, a query pre-filled from the current filter so it doubles as "save current search", an optional limit, an order picker). Delete from the trash button.
- [x] The 16a "Add to Playlist ▸" context verb is now real: a submenu of the static playlists (rebuilt as playlists change) → `append_playlist_tracks`.
- [ ] Static-playlist in-place drag-reorder is deferred (the queue-drawer DnD idiom); episode/book playlist entries too (schema supports them; v1 wires tracks).

### Phase 16e — Preferences: enable/disable sections ✅ (v0.1.8)

A runtime toggle over which media tabs a launch shows, distinct from the compile-time plugin features (§2.2): those decide what is in the binary, this decides what shows.

- [x] A `[sections]` config block (`music` / `podcasts` / `audiobooks`, default all true). Disabling a section skips building its tab and starting its subsystem at the next launch: the `attach_*_view` call is gated on the flag, so a disabled section's lazy `::map` init never runs (no page, no fetch worker) — the runtime-skip "carry no code" the user asked for.
- [x] Music stays as the fallback when nothing else is enabled, so the window is never empty; the view chrome (switcher / bottom bar) installs only when more than one tab actually shows.
- [x] Preferences → General gains a Sections group of switches (Podcasts / Audiobooks appear only when compiled in); applies next launch, the config idiom.

### Phase 16f — Context-sensitive header buttons ✅ (v0.1.8)

- [x] The music-only header controls (Edit, Embed tags, Properties inspector) are stored in the imp struct and hidden on the Podcasts / Audiobooks tabs, shown only on Music, via `view_stack.connect_visible_child_notify`. Universal controls (playback, prefs, output, menu, switcher) always stay. Only compiled when a second tab exists (a music-only build never switches).

## Phase 16.5 — UX completeness pass

From the 2026-07-01 per-tab UI/UX audit (three parallel sweeps over Music, Podcasts + playback surfaces, and Audiobooks + app chrome at v0.1.8), distinct from the competitor deep-dive that produced Phases 16 to 19: this one audited the shipped GUI section by section. Roughly 45 findings, clustered as safety/feedback gaps, cross-tab parity gaps (audiobooks lag music's Phase 16 polish), the podcast subscription lifecycle (entirely CLI-only), and episode/book power UX. Nine shippable sub-phases; zero migrations and zero new dependencies (every candidate turned out to have plumbing already: `shows.last_fetched` since 0006, the `delete_show` cascade, `retention::apply` for delete-download).

Deferred out of this phase, recorded here so they are not lost: a book list-view alternative to the shelf grid, a Continue Listening row, audiobook bookmarks, a download-manager pane with retry and storage usage, and the `Ctrl+S` save-Perspective binding.

### Phase 16.5a — Destructive confirms + bulk-edit error surfacing ✅ (v0.1.9)

- [x] Delete Perspective and Delete Playlist confirm first, naming the target, with the destructive-appearance response and Cancel as the default (the Remove-from-Library idiom).
- [x] Bulk-edit parse failures surface in a dialog (they went to stderr while the dialog closed silently); the editor re-presents pre-filled with the entered values and tick states, so a fix loses nothing. The collection loop is a pure `collect_assignments` with unit tests.
- [x] The mixed-values checkbox tooltip spells out the overwrite semantics.

### Phase 16.5b — Feedback + discoverability micro-fixes ✅ (v0.1.10)

- [x] Filter-bar warnings become readable (tooltip with the actual parser warnings, not just the yellow tint); a grammar tooltip on the filter entry.
- [x] Empty-library StatusPage points at the CLI import; inspector marks itself read-only; an empty facet pane explains itself; the embed-tags tooltip says it writes into the files.
- [x] Sidebar rows get full-name tooltips; Edit/Embed header buttons follow the selection (insensitive when nothing is selected, re-derived on every leaf reload).
- [x] An About dialog (GNOME convention); the "takes effect on the next launch" label sweep (the whole config dialog is a disk-snapshot edit, so every config group carries it); streaming-vs-buffering Now-bar tooltips; opening the Now Playing drawer after the queue ends shows the idle page (the ended-snapshot guard now matches the Now-bar's). Keymap doc: the sidebar Save button is the wired save path (`Ctrl+S` stays deferred).

### Phase 16.5c — Podcast subscription lifecycle in the GUI ✅ (v0.1.11)

- [x] Subscribe from the app: a sidebar footer button opens a URL dialog; failure re-presents with the URL preserved and the error explained, success toasts and selects the show. (Introduces the GUI async-network idiom, `glib::spawn_future_local` awaiting a tokio `JoinHandle`, and a `win.toast` / `win.reload-queue` action pair tab modules fire through the widget tree.)
- [x] Unsubscribe from the per-show settings dialog, behind a destructive confirm (episodes cascade out of library and queue; downloads stay on disk; a playing episode keeps playing and the queue drawer re-reads).
- [x] OPML import and export via file dialogs; `Ctrl+Shift+O` wired at last; import chains straight into a refresh-all so episodes arrive in one step.
- [x] Refresh: a footer button + `R` (selected show, or all shows from a bucket), a pure unit-tested `summarize_refresh` toast, and a last-refreshed header caption (pure `fmt_last_refreshed`) from the existing `shows.last_fetched`. Best-effort secret-service creds, the CLI idiom.
- [x] A no-subscriptions StatusPage with Subscribe / Import OPML calls-to-action; empty buckets and shows get per-source one-liners. The sidebar is rebuilt in place (`rebuild_sidebar`, the row→source map now shared state).

### Phase 16.5d — Episode-list power ✅ (v0.1.12)

- [x] Multi-select episodes (`MultiSelection`); the triage verbs (played / star / archive / queue) act on the whole selection with batch toasts, the first selected row deciding a toggle's direction; right-click inside the selection keeps it.
- [x] Sortable episode columns (Episode / Date / Length) over the pure core `cmp_episodes` + `EpisodeSort`; the model chain is store → sort → selection, so `play_from` resolves positions through the view and the default order stays the source's.
- [x] Count badges on the sidebar: bucket totals + per-show unplayed (the new core `podcast_sidebar_counts`, mirroring `episodes_in_bucket`'s definitions, integration-tested), refreshed in place after every triage write.
- [x] `Q` (queue selection) and `I` (mark unplayed) wired, plus `Ctrl+1/2/3` bucket jumps; a "Show Settings…" context verb (`open_settings_for` now takes the show id); an in-progress episode's Length cell reads "43:10 · 29%" (pure `length_text`, unit-tested).

### Phase 16.5e — Downloads, scoped ✅ (v0.1.13)

- [x] A Download context verb for undownloaded episodes (batch over the selection, double-start guarded); `download_episode_with_progress` grows an optional per-chunk callback in `conservatory-podcasts` (the CLI path unchanged), progress lands in an `Arc<Mutex<HashMap>>` drained by a self-stopping 500 ms ticker into each row's new `download-fraction` glib property (the click-to-rate targeted-repaint idiom); the availability glyph shows a syncing icon with a live percent tooltip; completion and failure toast. Pure `download_fraction` (clamped; stale enclosure sizes never read past full), unit-tested.
- [x] A Delete Download verb behind a destructive confirm ("the episodes stay; streaming still works"), riding `retention::apply` so file deletion has exactly one codepath (file removed, `audio_path` cleared).
- [x] The manager pane, retry surface, and storage dashboard stay deferred.

### Phase 16.5f — Spoken-word playback surfaces ✅ (v0.1.14)

- [x] Skip-back/skip-forward come to both settings dialogs (the schema fields exist since Phase 6; 0 inherits the 15/30 defaults, `settings_from_form` now passes every field through); the Now-bar gains quick-seek label buttons flanking play/pause for episodes and audiobooks, amounts per show (pure `resolve_skip_amounts` + `quick_seek_target` in core, unit-tested; the clamp stops just short of EOF and the seek reads the ≤250 ms snapshot).
- [x] Show notes keep their links: the ingest sanitizer's allowlist widens to the Pango inline subset (`a href` bare via `link_rel(None)`, `b`/`strong`, `i`/`em`; schemes http/https/mailto) and the stored form stays entity-escaped, so pure `notes_to_markup` renders it clickable (`strong`/`em` map to Pango `b`/`i`; legacy plain-text rows are detected and escaped whole). Old rows heal on their next feed refresh (the episode upsert rewrites `description`). The only render point is the podcasts detail pane; the CLI never prints episode notes.
- [x] The Sound dialog says which controls apply to music and which to everything; the Now-bar settings gear appears for audiobooks too, opening the playing book's settings with live `set_spoken` apply (`open_playing_book_settings` + a music-only stub).

### Phase 16.5g — Audiobook shelf completeness ✅ (v0.1.15)

- [x] Tiles show listening progress (a thin bar, in-progress books only) and a finished badge (a cover-corner checkmark via `gtk::Overlay`); the empty shelf gets a StatusPage (fresh library → the CLI import invocation; filtered → "No matches"); a sort picker (core `ShelfSort` + `sort_shelf_by`, integration-tested; in-progress-first stays the default and re-sorts happen on the cached rows without a DB re-read).
- [x] The book bulk editor gains the Phase 16c checkbox + mixed-values treatment: `common_value` promoted to core (shared with the music editor), new pure `book_edit_commons` in `conservatory-audiobooks` (the people displays convert `", "` to the dialog's `";"` form so a ticked write round-trips `split_people`), and a pure `build_book_edit` honouring ticks; parse failures use the 16.5a error/retry dialog instead of stderr.
- [x] Book edits, re-shelve moves, and playback-settings saves toast like music edits do (the `win.toast` channel).

### Phase 16.5h — Audiobook verbs ✅ (v0.1.16)

- [x] Core grows the kind-generic queue insert (a private `insert_queue_items_at` re-expressing `insert_queue_tracks_at` and backing the new `insert_queue_books_at`) and `delete_book` (the `delete_track` twin; cascades verified against migrations 0011/0013), both worker-routed and integration-tested (`queue_insert_books_at_shifts_positions`, `delete_book_cascades_queue_playback_and_fts`).
- [x] The book context menu reaches music parity: Play Next (engine/DB lock-step, ids derived from the built items), Reveal in Files (`xdg-open` on `books.folder_path`), Mark Finished/Unfinished (batch; also a detail-pane button whose label follows the shown book; unfinishing preserves resume + overrides), and Remove from Library behind the destructive confirm (files stay on disk; the queue drawer re-reads).

### Phase 16.5i — App chrome ✅ (v0.1.17)

- [x] The window title reflects the playing item (any kind; "Title - Artist - Conservatory") and reverts when idle, via a pure unit-tested `window_title` set from `refresh_now_bar`'s item-change and clear branches.
- [x] The status bar reports per-tab: Music keeps the live aggregate; Podcasts shows the triage totals (pure `podcast_status` over `podcast_sidebar_counts`) and Audiobooks the shelf summary (pure `book_status`), recomputed in `update_header_for_view` on each switch.
- [x] The narrow-width pass: the right status label ellipsizes like the left, so neither line can force the window wide under the 550sp breakpoint.

**Phase 16.5 complete.** All nine sub-phases shipped, v0.1.9 through v0.1.17: zero migrations, zero new dependencies, roughly 45 audit findings closed or explicitly deferred (the deferred list is at the top of this phase).

## Phase 17 — Player table-stakes

From the v0.1.2 UI/UX deep-dive: the three things a serious music player has that Conservatory did not. Shuffle and repeat (absent from the engine and the UI), context-aware ReplayGain (album gain played straight through, track gain when shuffling), and the queue-vs-playlist verb clarity the research called out. Four independently shippable sub-phases, the Phase 16 / 16.5 shape; zero new dependencies. Decisions (with Brandon): shuffle is **in-place** (enabling it physically reorders the upcoming queue tail, so the queue view stays the play order, spec §6.1), and both modes **persist** across restarts on the DB-canonical `audio_state` singleton.

### Phase 17a — Repeat modes ✅ (v0.1.23)

Repeat is a pure engine-advance concern, so it ships first (no reorder).

- [x] Migration 0018 adds `repeat` (TEXT `off`/`all`/`one`) and `shuffle` (bool) to `audio_state`, both added up front so 17b needs no second migration; `AudioState` + `get_audio_state` + `set_audio_state` carry them. A new `player::mode::Repeat` enum (`as_str` / `from_stored` / `next`, the `ReplayGain` idiom) is the one place the string becomes an enum.
- [x] The engine gains `repeat` and `PlayerCommand::SetRepeat`: `One` reloads the current item at EOF instead of advancing (a one-shot stop, stop-after-current or the end-of-item sleep, still wins); `All` wraps to the top via `wrap_to_top` instead of ending (an end-of-queue sleep timer still wins); `One` is a prefetch stop-boundary (no gapless hand-off to a track we are about to replay). `PlayerSnapshot` carries the mode.
- [x] Now-bar repeat button (icon + dimmed-when-off opacity + tooltip, the distinct song-repeat glyph for `One`), cycled by the button and `Ctrl+R`, applied to the engine and persisted to `audio_state`; restored into the engine at launch (`apply_persisted_audio`). The shortcuts reference + `docs/keymap.md` gain the binding.
- [x] Tests: `Repeat` round-trips its TEXT; `audio_state` persists the modes; two engine integration tests through the null host — `One` replays the current item (play_count climbs, the queue never ends) and `All` wraps back to index 0 without ever ending.

### Phase 17b — Shuffle ✅ (v0.1.24)

In-place reorder; the DB queue and engine queue stay lock-step by applying the **same permutation** (the position-keyed invariant).

- [x] Pure `player::shuffle::shuffle_order(len, keep_prefix, seed)` (a hand-rolled SplitMix64, no new dep) leaving the played + current prefix in place and Fisher-Yates-shuffling the future, plus `apply_permutation`; `PlayerCommand::SetShuffle` / `ReorderQueue(perm)` (both length-guarded so a stale perm is ignored, not corrupting); the repeat-all wrap reshuffles when shuffle is on (`wrap_to_top`, persisting the same perm); a `reorder_queue_by_positions` worker op both the GUI toggle and the engine wrap use. `PlayerSnapshot` carries `shuffle`.
- [x] Now-bar shuffle button (left end of the transport, dimmed when off) + `Ctrl+K` (Ctrl+U is the queue); toggle-on computes one permutation and applies it to both the DB (`reorder_queue_by_positions`) and the engine (`reorder_queue`); the replace-queue Play paths (double-click / facet Play via `play_leaf_from`, and `play_playlist`) shuffle the built queue (activated item first, `playqueue::shuffle_play_order`) before writing it; the flag persists to `audio_state` and restores at launch (without reshuffling the resumed queue).
- [x] Tests: `shuffle_order` (permutation / prefix / determinism) + `apply_permutation` unit tests; `shuffle_play_order` puts the activated item first; a worker test that `reorder_queue_by_positions` applies a perm and keeps positions dense (and ignores a stale one); an engine integration test that `ReorderQueue` tracks the current index (and no-ops a stale perm).

### Phase 17c — Context-aware ReplayGain ✅ (v0.1.25)

- [x] `MusicProfile` carries both resolved gains (`rg_album` / `rg_track`, each preamp-adjusted + clamped, falling back to the other so one-gain tracks still normalize) and a pure `contextual(shuffle)` picks album-when-linear / track-when-shuffle; `replaygain_db` stays the effective field the `@rg` chain renders (so `chain.rs` is untouched), defaulting to the album/in-order gain. Track and Off modes are context-invariant; spoken word carries neither gain. The engine applies `profile.contextual(self.shuffle)` at `load_current` and `advance_into_prefetched`, and re-applies live to the current track on `SetShuffle` (the gap-free `apply_profile` path, the set_eq idiom). Unit-tested (album↔track swap, Track/Off no-op, single-gain fallback, spoken word `None`); the existing `resolve_music_profile` tests hold unchanged.

### Phase 17d — Queue-vs-playlist verb clarity ✅ (v0.1.26)

- [x] Audited the five context menus: the verbs were already consistent (Play / Play Next / Add to Queue / Remove from Queue / Clear Queue), and only the track menu carried the queue-vs-playlist ambiguity (it lumped "Add to Playlist" in with Edit / Rating). Fixed by grouping the track menu into a labeled **"Play queue"** section (Play / Play Next / Add to Queue) and a separate labeled **"Playlists"** section (Add to Playlist), so the transient queue and the saved list never read as synonyms. `docs/keymap.md` gains a "Queue vs. playlists" explainer. No engine work.

**Phase 17 complete.** All four sub-phases shipped, v0.1.23 through v0.1.26: shuffle and repeat (in-place, persisted), context-aware ReplayGain, and the queue/playlist clarity pass. Migration 0018 (two additive `audio_state` columns); zero new dependencies. The music half now has the player table-stakes it was missing.

## Milestone 0.2.0 — Grammar & columns

### Phase 18 — Grammar and column power

The power-user *data* tier (from the v0.1.2 UI/UX deep-dive). Deepens the two surfaces that separate a manager from a player: the search grammar and the browse columns. Two hard-phased sub-phases; `0.2.0` tags when 18b lands.

### Phase 18a — Accent-folding ✅ (v0.1.27)

- [x] Accent-folding, always-on for substring / quoted / fuzzy matches (`=exact` and `~regex` stay literal), so `bjork` matches `Björk`. Eval path: a pure `conservatory-search::fold` (NFD, drop combining marks, lowercase; reuses the workspace `unicode-normalization`) applied in the bare-text / substring / fuzzy matchers. SQL fast path: migration 0019 rebuilds every FTS table with the `unicode61 remove_diacritics 2` tokenizer, so bare text folds through `MATCH`, mirroring `fold` (the dual path stays consistent). Field-text on the fast path (`artist:bjork`) folds only on eval, documented. Unit + eval + FTS integration tests.
- [x] Saved-query-by-name reuse was **already** implemented as `vl:NAME` (parse-time expansion with a cycle guard); recorded in the grammar doc, no code needed.

### Phase 18b — Configurable built-in columns ✅ (v0.2.0)

- [x] A column catalog (`track_list::COLUMN_CATALOG` + `build_column`, id → column) + `[browse].columns` config (default = the pre-18b fixed set, so an unconfigured launch is unchanged; unknown / duplicate ids skipped, forgiving) + a Preferences "Browse columns" editor (a switch per catalog column, the Phase 10c idiom). Beyond today's set the catalog exposes year / track# / format / bitrate / play count / date added / last played; the numeric/date columns are fixed-width and monospace (Phase 13d `.tech`). `TrackBrief` + `facet_tracks` widen for the new fields (curated, the 50k §13 budget); `TrackSort` + `cmp_tracks` gain the seven sort keys (unit-tested). Applies next launch (the config idiom). A title-formatting mini-language stays deferred; **reorder is a follow-on** (the editor toggles visibility, the config accepts any order).
- Tests: `cmp_tracks` for the new keys; a catalog-vs-defaults consistency test; GUI smoke-launch with a 12-column custom config (every new id builds without panic).
- *Usable artifact:* add a Year / Play Count / Format column from Preferences, persisted across launch.

**Phase 18 complete → `0.2.0` tagged.** Accent-insensitive search (18a) and configurable browse columns (18b); one migration (0019, FTS re-tokenize); no new external dependency. The first capability milestone on the runway to 1.0. Next: `0.3.0` (Phase 26 de-adwaita, pulled forward; see the re-sequencing note under the version table).

## Milestone 0.3.0 — Hyprland-native design (de-adwaita)

### Phase 26 — Hyprland-native design: de-adwaita (`0.3.0`)

Portfolio direction change (Brandon, 2026-07-09): the goal moved from "runs politely under Hyprland" (Phase 25's original frame) to "fully belongs on Hyprland." Concretely: drop libadwaita, keep GTK4. GTK4 is Wayland-native and stays; what goes is the GNOME identity layer (the adwaita stylesheet, the adaptive widgets, the preference-row family, `adw::StyleManager`), replaced by plain GTK4 widgets and an application stylesheet Conservatory owns outright, styled flat and tiling-first around the Columns UI browse surface (which is already plain `gtk::Paned`s and `gtk::ColumnView`, the least adwaita-shaped UI in the portfolio). Colophon piloted the move (its roadmap, Phase 6, shipped v2.0.0 on 2026-07-10); Conservatory follows its proven patterns (widget replacements, generated owned stylesheet, the USER+1 provider-priority fix). "Never break userspace" binds in full: no feature regression, the app keeps working under GNOME, and the spec §13 memory budgets hold (a widget-and-CSS migration should be memory-neutral; anything that escalates gets its own `heaptrack`/`massif` checkpoint, per the Continuous rules).

Execution frame (2026-07-10): pulled forward as the `0.3.0` milestone (re-sequencing note under the version table). Sub-phases 26a–26n ship as `0.2.1`–`0.2.12`, then the `0.3.0` tag; order is decisions → owned widgets (rows, dialogs, status pages, toast) → shell (stack/switcher, then headerbar/toolbar-view/window parent) → owned stylesheet → the toolkit cut → verification. The owned sheet lands one commit *before* the cut (registered above the adwaita sheet, it overrides cleanly while libadwaita is still linked), so the cut commit itself is nearly empty and visually invisible.

Sub-phase ledger (updated 2026-07-10; each ships one commit, green under `cargo build`, `cargo clippy -D warnings`, `cargo test`, and `--no-default-features`, with a patchnotes entry and VERSION + workspace Cargo.toml bump):

- [x] **26a (docs, no bump):** decisions locked and renumbering; spec §2.4 authored.
- [x] **26b (`0.2.1`):** `ui/rows.rs` seed + `ui/shortcuts.rs` (hand-rolled F1 window) + `ui::close_on_escape` + `gtk::AboutDialog`.
- [x] **26c (`0.2.2`):** `ui/status_page.rs` composite; all 7 empty states converted; `now_playing_panel.rs` is adw-free.
- [x] **26d (`0.2.3`):** owned toast (gtk::Overlay + crossfade revealer, newest-wins hide cancellation); interim pill `.toast` rule in the main.rs CSS.
- [x] **26e (`0.2.4`):** `ui/dialogs.rs::Alert` (mirrors the adw::AlertDialog API; exactly-once close-response dispatch; root()-resolved transient parent); 13 window.rs sites converted.
- [x] **26f (`0.2.5`):** `rows::switch_row` / `rows::spin_row` / `rows::group` (control returned beside the row so handler bodies stay identical); the per-show and per-book playing-settings dialogs converted, completing window.rs's AlertDialog set.
- [x] **26g (`0.2.6`): podcasts.rs converts.** 4 AlertDialogs, the settings rows, and a new `rows::combo_row`; `adw::prelude` dropped.
- [x] **26h (`0.2.7`): audiobooks.rs converts.** 5 AlertDialogs, rows, the bulk-edit grid dialog; `adw::prelude` dropped.
- [x] **26i (`0.2.8`): the Preferences rebuild.** Plain modal window + text `GtkStackSwitcher` over General / Library / Sound; new `rows::entry_row` / `rows::action_row` / `Group::set_header_suffix` / `Expander` (enable switch + revealer) for the DSP trio; save-on-close via `connect_close_request`. Containers only; every EQ/DSP/output handler body untouched. The audible A/B acceptance rides the phase's hands-on pass (26n).
- [x] **26k1 (`0.2.9`): stack and switcher.** `gtk::Stack` + text-only switchers + the `size_allocate` width watcher (inclusive 550px crossing, `is_narrow()` unit-tested) + plain-box plugin hosts. Also repaired the music-only CI lane (dead-code lint, red since v0.2.2).
- [x] **26k2 (`0.2.10`): the shell cut.** `gtk::ApplicationWindow` parent, flat no-buttons titlebar, vertical-box body preserving the §2.3 stacking; spec §2.3 rewritten.
- [x] **26l (`0.2.11`): the owned stylesheet.** See the phase item above.
- [x] **26m (`0.2.12`): the toolkit cut.** libadwaita gone from Cargo/meson/CI; doc sweep done; `cargo tree` clean both feature sets.
- [x] **26n (`0.3.0` tag): verification tail + release.** `docs/hyprland.md` authored; CI green; both binaries smoke-run live; hands-on pass accepted by Brandon 2026-07-11. **Phase 26 complete → `0.3.0` tagged.**

- [x] **Go/no-go and sequencing against the pilot.** Done 2026-07-10, verdict go. Colophon's Phase 6 shipped whole (17 `adw::` types replaced, owned generated sheet, zero new deps, GNOME 49 Flatpak runtime kept). Conservatory's surface inventoried: 173 `adw::` uses across 7 files (window.rs ~127), 30 distinct types, only the GTK binary crate depends on libadwaita, no `.ui`/`.blp` templates. Every type has a mapped plain-GTK successor; the two surfaces the pilot didn't cover (24 `adw::AlertDialog`s with response styling, the SpinRow/SwitchRow/ComboRow/EntryRow/ExpanderRow family) get owned equivalents (`ui/dialogs.rs::Alert`, grown `ui/rows.rs`). Conservatory is well positioned: no GSettings (config.toml), fonts bundled, dark-only Kanagawa already forced rather than portal-followed.
- [x] **Design decisions land in spec first.** Locked 2026-07-10, recorded in spec §2.4: slim flat headerbar with **no window buttons**; tab switcher becomes `GtkStackSwitcher` over `GtkStack`, **text-only tabs** (icons dropped); the narrow-width collapse to a bottom switcher bar survives via a manual width watcher at the same 550px threshold; dialogs become owned modal `gtk::Window`s (`Alert` mirrors the `adw::AlertDialog` API so the 24 call sites convert mechanically; stock `gtk::AlertDialog` rejected: no extra child, no per-response styling, index-addressed buttons); the shortcuts reference re-skins as a hand-rolled window; the owned sheet bakes the Dragon hexes directly (single fixed palette, so CSS custom properties and the GTK 4.16 floor they need are not taken; gtk4 stays on `v4_14` and CI stays ubuntu-latest). Dark/light is the easy case: the app is unconditionally dark-Kanagawa, so no portal work is needed, only `gtk-application-prefer-dark-theme` replacing the `ForceDark` adwaita call.
- [x] **The owned stylesheet.** Shipped 26l (v0.2.11): `conservatory/src/theme.rs` generates the sheet from baked Dragon hex consts (token replacement, no CSS custom properties, gtk4 stays v4_14) and installs it at `STYLE_PROVIDER_PRIORITY_USER + 1` (outranks a user `gtk.css`, the Colophon discovery); the accent ring rides at USER + 2. Covers the base widgets adwaita used to theme plus the utility classes and the migrated app rules; deliberate exception recorded in `docs/theme.md`: the lifted cover cards keep radius + shadow (chrome flat, content imagery lifted). Sheet tests: hexes present, exactly three `font-family` rules, no `@define-color`.
- [x] **Packaging and docs follow-through.** Shipped 26m (v0.2.12): `meson.build` and both CI lanes dropped libadwaita (meson's GTK floor aligned to the real 4.14 pin); spec §2.3 widget tree, §2.4, masthead, §3.1, §11, §13, §15, `docs/theme.md`, `docs/keymap.md`, and the README all read plain-GTK. Flatpak runtime note for the Phase 20 manifest: Colophon evaluated GNOME → freedesktop and stayed (GTK4 ships in the GNOME runtime, not in org.freedesktop.Platform); expect the same call here.
- [x] **Verification tail.** Automated half (2026-07-11): all CI gates green including the music-only lane (repaired at 26k1 after being red since v0.2.2); `cargo tree` adwaita-free under both feature sets; both binaries smoke-launch and run under the live session with no errors (portal warnings only, expected without `xdg-desktop-portal`, noted in the new `docs/hyprland.md`). Hands-on half: **accepted by Brandon 2026-07-11**, releasing the `0.3.0` tag. The itemized audit checklist (tiling geometry floor, keyboard walk + seek-step decision, no-GNOME session checks, fractional-scale spectrum, `VmHWM` against `v0.2.0`) stays available above for re-runs; anything found later lands as a `0.3.x` fix, not a blocker.

*Usable artifact:* Conservatory runs the same feature set with zero `adw::` symbols in the workspace, its own flat tiling-first stylesheet on all three tabs, and no regression under GNOME; the memory budgets hold and the folded Phase 25 audit items pass against the new shell.

## Milestone 0.4.0 — Immersive & history

### Phase 19 — Immersive polish

The experience tier: the seek bar and Now Playing become immersive, and import gains a pointer path.

- [ ] A waveform seek bar (the seek bar as the track's loudness envelope), reusing the Phase 12d PipeWire tap / the offline decode; the accent-tinted scrubber precedent.
- [ ] Full-screen Now Playing (the Hermitage Codex moment at full bleed), drag-drop file import (drop audio onto the window to import through the existing pipeline), and richer navigable-credits metadata from local sources.
- *Usable artifact:* a waveform scrubber and drag-drop import. Ships alongside Phase 9 under `0.4.0`.

### Phase 9 — Listening history sync (scrobbling)

Already specified above (see "Phase 9 — Listening history sync"), now sub-phased 9a/9b/9c; it is the one remaining pre-1.0 *feature*, optional and off by default (ListenBrainz + optional Last.fm, a one-way local-first outbox). **9a shipped v0.3.1** (the outbox, the ListenBrainz client, the config, the CLI, all headless); **9b shipped v0.3.2** (the engine completion hook enqueues real plays, the GUI spawns the submitter, and a Preferences "Sync" page enables + validates ListenBrainz); 9c (Last.fm) remains. **Slotted into `0.4.0`** so the immersive tier and the optional history sync ship together. **Tags `0.4.0`.**

## Milestone 1.0.0 — Verified & packaged

### Phase 20 — The 1.0 endgame (quality + packaging, mirrors Atrium's Phase 20)

No new features: the readiness gate that earns the `1.0.0` tag, gathering the tracked pre-1.0 items (see "Tracked pre-1.0 items" under Phase 15) and the packaging the 0.1.0 gate deferred.

- [ ] **50k real-library memory gate:** confirm the spec §13 idle target (< 200 MB) on a working copy of Brandon's real ~50k-track library (the synthetic fixture tops out at 12k, extrapolating to ~215–230 MB); optimize if over before the tag.
- [ ] **Full-library move-safety pass:** run the Phase 15a move / undo / crash checks against a working copy of the real library (the 0.1.0 gate was synthetic-only).
- [ ] **Library-root decision (§16.14):** settle the `~/Music/Conservatory/Music/` stutter (accept it / `~/Conservatory` / drop the music-tree prefix) and make it the config default.
- [ ] **Flatpak + AppStream:** the `org.gnome.Conservatory` (or `io.github.virinvictus.Conservatory`) manifest, a validating `metainfo.xml` (releases tag, 16:9 screenshots, SPDX license, `appstreamcli validate` clean), the Meson packaging wrapper wired end-to-end, and GNOME Circle readiness (§12). The final icon pass (a conservatory silhouette, §15).
- *Usable artifact:* a `v1.0.0` tag the move logic, the memory budget, and a real installable Flatpak build have all earned, numbers recorded. **Tags `1.0.0`.**

## Beyond 1.0 (committed tiers)

The doors the spec left open (§16) plus the researched gaps against MusicBee / Calibre, committed as real phases rather than "considered." Each is a capability milestone; ordering is a lean, not a promise.

### Phase 21 — Metadata intelligence (`1.1.0`)

The biggest competitive gap and the most "Calibre for audio" of the lot. Online metadata + cover-art fetching that **consumes a canonical source** and does not try to out-Picard Picard on match quality (§14, §16.5).

- [ ] Music: MusicBrainz lookup (by AcoustID / existing tags) + Cover Art Archive fetch, presented as a review-then-apply step through the existing bulk-edit + write-back pipeline (never a silent auto-tag; the move-safety discipline). Resolves §16.5.
- [ ] Audiobooks: an online provider (Audnexus / Google Books, the Audiobookshelf model) behind the same review-then-apply flow. Resolves §16.10.
- [ ] A new optional dependency + network surface behind config (off by default); credentials (if any) in libsecret via the existing `oo7`.

### Phase 22 — Curation depth (`1.2.0`)

Deepening each media type's curation, mostly cashing in escape hatches the schema already anticipates.

- [ ] Genre-tree rollup: leaf tags (Synthwave) collapse to a coarse shelving parent (Electronic), the §5/§16.3 v2 escape hatch for when flat shelving churns. Re-shelving stays cheap (the `shelf_genre` + template design already supports it without a migration).
- [ ] Audiobook chapterize by silence detection on import (the m4b-tool technique) for single-file books with no markers (§16.11), opt-in.
- [ ] Audiobook bookmarks + a Continue-Listening row + a book list-view alternative to the shelf grid (the 16.5 deferrals).

### Phase 23 — Library operations (`1.3.0`)

The heaviest new-subsystem tier: moving audio in and out, and holding more than one library. All local-first.

- [ ] Format conversion / transcoding on export (shell out to `ffmpeg`, the Lattice idiom): a transcoded copy for a phone / DAP, originals never touched.
- [ ] Device / DAP sync (Calibre's send-to-device): a one-way export to a mounted device with a chosen profile.
- [ ] Multiple libraries (MusicBee's per-folder databases): switch between distinct library roots, each its own DB. Inverts the current single-root assumption, so it is deliberately late.

### Phase 24 — Long-form & background (`2.0.0`)

The far horizon; the spec caps some of these at 2.x explicitly.

- [ ] Podcast / audiobook transcripts (§14: "2.x maybe at most"): fetch/display where a feed offers them, or a local speech-to-text step.
- [ ] Portal-mediated background feed refresh (§12): periodic refresh without the app foregrounded, via the background portal (Flatpak-friendly).

### Phase 25 — Compact mode: the mini-player (`2.1.0`)

Brandon's desktop moved from GNOME Shell to Hyprland (a Wayland tiling compositor); this phase originally audited and hardened the app for a tiling, keyboard-first, GNOME-shell-free environment. **Strictly additive**: nothing here may regress the GNOME/default-desktop experience (the CLAUDE.md "never break userspace" rule).

Direction note (Brandon, 2026-07-09): the portfolio goal has since moved past "runs politely under Hyprland" to "fully belongs on Hyprland," which means dropping libadwaita while keeping GTK4; that lands as Phase 26, gated on the Colophon pilot.

Reframe (Brandon, 2026-07-10, with Phase 26 pulled forward to `0.3.0`): the audit items below (tiling geometry, small-tile sizing, keyboard coverage, session independence, Wayland integration, CSD posture, `docs/hyprland.md`) **fold into Phase 26 as its verification tail**, run once against the migrated shell instead of twice. What remains of this phase is its one genuinely new surface, the **compact / mini-player mode**, which stays deferred at `2.1.0`: it is new UI, not toolkit migration, and it deserves its own design pass. The item texts below are kept as written (their file/line references describe the pre-migration shell and will have drifted by the time this phase runs); read them as the requirements brief for the compact mode plus the original audit checklist Phase 26 executed.

- [ ] **Tiling-first geometry audit.** The Music view's Columns UI is three plain `gtk::Paned`s, not the `AdwNavigationSplitView` the widget-tree diagram in spec.md §2.3 already calls for, and none of the three tabs (`window.rs:389`/`401`, `podcasts.rs:1649`/`1656`, `audiobooks.rs:296`) use it either; a fixed-position splitter just clips content in a narrow tile instead of collapsing. Each per-pane `width_request(160)` (`facet_pane.rs:143`), multiplied by up to five configurable panes (Phase 10c), plus the Perspectives sidebar's `width_request(170)` at `window.rs:4809` with `body.set_shrink_start_child(false)` at `window.rs:405` (the sidebar can never shrink and has no hide affordance, unlike the queue drawer and inspector, which are both `gtk::Revealer`s), can add up to more horizontal space than a quarter-monitor tile provides. Measure the real floor and either convert the bodies to `AdwNavigationSplitView` (matching the spec diagram) or add a dedicated narrow-width `AdwBreakpoint` that collapses/hides the sidebar, the way the header `AdwViewSwitcher` already collapses at 550sp (`window.rs:3921`-`3928`, currently the app's only breakpoint).
- [ ] **Small-tile sizing pass.** `set_default_size(1100, 700)` (`window.rs:277`) is the only size the window declares; there is no explicit minimum, so behaviour at a half/third/quarter-monitor Hyprland tile is unverified rather than guaranteed. Check the window actually reaches a usable state at Brandon's real tiling geometries, and spot-check dialog sizing (the bulk-edit `AlertDialog`, the per-show settings dialog, `AboutDialog`, the Preferences dialogs) plus the Now-bar's two 220px-wide widgets (`now_bar.rs:148`, `232`) and the queue drawer (`queue_panel.rs:186`, 230px) and inspector (`inspector.rs:75`, 250px) revealers for overflow at the smallest tile size in daily use.
- [ ] **Compact / mini-player mode (flagship item).** No mini-player, compact-mode, or floating-window surface exists anywhere in the codebase today (confirmed by search); this is new surface, not a refactor. Design and build an explicit compact Now Playing layout (cover art, transport, seek, a short queue peek) that replaces the full Columns UI browse when active, reachable by an explicit toggle (menu item + a new keyboard shortcut, alongside the existing `Ctrl+I` "Now Playing details drawer") and, ideally, auto-engaged by a new narrow-width/narrow-height `AdwBreakpoint` so shrinking the window onto a Hyprland scratchpad workspace naturally lands in the compact layout instead of a squeezed browse view. The stable `application_id` (`"org.virinvictus.Conservatory"`, `main.rs:21`) is already fixed and needs no change to be the anchor for a Hyprland `windowrulev2` float/size/workspace rule.
- [ ] **Keyboard-first coverage audit.** `docs/keymap.md` is already substantially wired (Space, `Ctrl+`arrows, `Alt+1/2/3`, the queue keys, `F1` for the shortcuts reference), so this is a gap audit, not a rebuild: the doc itself flags bare-arrow and `Shift+`arrow seeking as deliberately unbound (arrows navigate the browse columns instead) and there is no dedicated seek-step binding at all; decide whether a keyboard-centric compositor changes that call. Confirm the shortcuts reference (`window.rs:4251`-`4257`, an `adw::PreferencesDialog` of grouped rows, not a real `gtk::ShortcutsWindow`, per the code's own comment) stays in sync with the doc, and walk the full Tab/Shift+Tab focus order (facet panes to leaf list to Now-bar) with the mouse untouched.
- [ ] **GNOME-session independence, verified.** No `gio::Settings` / GSettings / dconf call exists anywhere in the app (confirmed by search; config is the existing `config.toml` from Phase 10), and `adw::StyleManager::default().set_color_scheme(ForceDark)` (`main.rs:200`) is unconditional, so the app already has zero dependency on gnome-settings-daemon or the appearance portal for its theme; confirm a later theming pass doesn't accidentally introduce one. Fonts are already bundled (`data/fonts/`, Phase 13d), nothing to change. `oo7::Keyring::new()` (`conservatory-podcasts/src/credentials.rs:52`) and its GUI caller (`conservatory/src/ui/podcasts.rs:756`-`759`, which swallows a missing-secret-service error with `.ok()` and polls private feeds anonymously rather than crashing) already degrade gracefully; verify that holds when running under Hyprland with no `gnome-keyring-daemon` and no portal-backed Secret implementation, and record the one-line fix (starting a Secret Service provider from a Hyprland `exec-once`) in the new doc below. The suspend inhibitor rides `org.freedesktop.login1` (systemd-logind, not GNOME), so it should already work unchanged; confirm under Hyprland specifically.
- [ ] **Wayland/Hyprland integration pass.** Fractional-scaling sanity check on the spectrum visualizer (`conservatory/src/ui/spectrum.rs`): it lays out bars purely from the `DrawingArea`'s logical-pixel allocation and never reads `scale_factor` (confirmed absent workspace-wide), which should ride GTK's automatic HiDPI handling, but has never actually been eyeballed at Hyprland's 1.25x/1.5x fractional scales. The three `gtk::FileDialog` call sites (`window.rs:1359`, `podcasts.rs:812`, `1040`) are already portal-backed and need no GNOME-specific service; confirm which portal backend actually answers them under Brandon's Hyprland setup and note the required package. MPRIS metadata is already complete for a waybar-style bar module (`mpris:trackid`, `xesam:title`/`artist`/`album`, `mpris:length`, and a percent-encoded `mpris:artUrl`, all built in `conservatory-core/src/mpris.rs`'s `build_metadata`); verify end-to-end with `playerctl metadata` and a real waybar `mpris` module config rather than assuming the fields are sufficient.
- [ ] **CSD posture, confirmed.** Keep client-side decorations everywhere; no code currently reads or assumes a particular `gtk-decoration-layout` value (confirmed by search), so hiding the window buttons (a common tiling-WM convention, since Hyprland's own keybinds close/float/fullscreen windows) should already be harmless. Eyeball it with the buttons actually hidden rather than leaving it assumed.
- [ ] **New doc: `docs/hyprland.md`.** The `windowrulev2` recipe for floating and pinning the compact Now Playing window to a scratchpad workspace, the Secret Service setup one-liner for private podcast feeds, the working waybar `mpris` module config, and the required portal packages, so the environment-specific setup lives in one place rather than tribal knowledge (the `docs/` convention from the "Continuous" section at the top of this file).

*Usable artifact:* a compact Now Playing mode exists that a `windowrulev2` rule can float onto a scratchpad, with an explicit toggle and a narrow-geometry auto-engage. The GNOME/default-desktop experience is unchanged pixel-for-pixel.
