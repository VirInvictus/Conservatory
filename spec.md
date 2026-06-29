# Conservatory — Application Specification

**Version:** 0.0.2 (in development; Phase 1a/1b shipped, and the workspace is restructured around compile-time plugins, see §2.2)
**Target:** GNOME 50+, GTK4 ≥ 4.16, libadwaita ≥ 1.7
**Language:** Rust (2024 Edition)
**Build System:** Cargo workspace (`conservatory-core` + `conservatory-search` + `conservatory-podcasts` + `conservatory-audiobooks` + `conservatory-cli` + `conservatory`) / Meson wrapper for Flatpak packaging
**License:** GNU GPL v3.0 or later (forced by the GPL libraries libmpv links, the same license chain as Belfry; see §15)

> **Status note.** This is the design contract. The decisions below are settled enough to build against. As of v0.0.1 the workspace skeleton exists and Phase 1 (§17) is underway; the build is no longer deferred (the original deferral and its rationale are preserved in §16.1 and §17 for the record, since they are the thing to re-read if the concurrency with Atrium proves a mistake). Provisional detail (exact schema columns, CLI verbs, config keys) follows the established portfolio patterns from Atrium and Belfry and will firm up at implementation time. Genuinely open decisions are collected in §16, not scattered as silent guesses.

---

## 1. Mission Statement

Conservatory is **Calibre for audio**: a native GNOME library manager that *owns and organizes* your music, podcasts, and audiobooks on disk, presented through a foobar2000 Columns UI browse surface, played through a libmpv daily-driver engine that runs all three media types from a single queue.

Four commitments, in priority order:

1. **The database owns the library.** The SQLite database is the source of truth for organization and curated metadata; the application owns the on-disk layout and moves files to match it. This deliberately *inverts* the filesystem-canonical stance of Lattice and Belfry. There, the filesystem is the contract and the database is a regenerable index. Here, you hand Conservatory your files and it shelves them (`Genre / Album Artist / Album /` by default, §5.1), the way Calibre takes a book and files it under its author tree. That is a trust commitment, and §5.4 spends it carefully (dry-run, undo, embedded-tag write-back so files stay portable).

2. **Calibre-shaped, Columns UI browse.** Every list view is a queryable database. The default music surface is a faceted, hierarchical Columns UI browser (the design proven in `deadbeef-cui`, freed from being a player plugin), backed by the full Calibre-style search expression grammar (§3.4). Sortable columns, multi-select bulk actions, saved Perspectives. The browse panes filter; the grammar searches; they are the same surface.

3. **One engine, one queue, three media types.** Music tracks, podcast episodes, and audiobooks share a single libmpv engine and a single play queue. Each queued item carries its own playback profile: gapless and ReplayGain for an album track, Smart Speed and Voice Boost for a spoken-word item (episode or audiobook). A first-class, mixed listening queue is the standout feature and the reason Belfry is absorbed rather than kept separate (§6.1). Audiobooks are long-form speech, so they ride the same absorbed speech engine as podcasts (variable speed, Smart Speed, Voice Boost, sleep timer, chapters, first-class resume); they differ in being *owned and curated* rather than ephemeral (§5.7).

4. **A daily-driver player, not a previewer.** For libraries Conservatory manages, it is the place you listen, replacing deadbeef (and, for audiobooks, Cozy). Gapless, ReplayGain, EQ / DSP, output selection, MPRIS, media keys (§6). This is required, not optional: because Conservatory moves files, any external player's in-place references go stale the moment a library is re-shelved.

Reference apps:

- **Calibre** (Kovid Goyal et al.) — the library-as-database model, file ownership, and the "save to disk" path template that makes the on-disk tree a render of the database rather than a lock-in.
- **foobar2000 / Columns UI / Facets**, via **`deadbeef-cui`** (Brandon's own) — the faceted, multi-pane, metadata-first browse layout for large collections.
- **beets** (Adrian Sampson et al.) — the `lastgenre` canonicalization model (curated genre whitelist plus tree) that informs the shelf-genre normalization in §5.2.
- **Overcast** and **Castro**, via **Belfry** (Brandon's own) — the absorbed podcast engine (Smart Speed, Voice Boost) and the Inbox → Queue → Played triage model.
- **Cozy** (Julian Geywitz et al.) — the audiobook side: a GTK4 / libadwaita audiobook player for Linux. Its data model (Book → Chapter → file), import/scan, and browse surface inform Conservatory's Audiobooks tab (§3.8); its GStreamer player layer does not (Conservatory uses libmpv throughout).
- **Audiobookshelf** (advplyr et al.) — the audiobook metadata model and organization conventions: author vs narrator, series with decimal sequence, the `Author/Series/Title (Year)/` layout, and the sidecar conventions (§4.5, §5.7, §7.5).
- **Atrium** and **Viaduct** (Brandon's own) — the single-writer SQLite worker pattern and the search-expression grammar shape.
- **Hermitage** (Brandon's own) — cover art as the visual unit; per-album accent extracted from cover hue (median-cut quantizer).

**Absorbs Belfry.** Conservatory is the convergence of Brandon's music tooling (`Lattice`, `deadbeef-cui`) and his podcast project (`Belfry`) into one media app. Belfry's Phase 1 work is not discarded: `belfry-core`'s single-writer worker is the exact pattern this app needs and migrates here, and Belfry's audio engine and triage model become the Podcasts side. The one casualty is Belfry's filesystem-canonical design; in Conservatory, podcasts become app-managed downloads (§5.3), which is acceptable for ephemeral episodes in a way it would not be for a curated music collection. Belfry is **not retired until Conservatory reaches podcast parity** (§17).

Non-goals are enumerated in §14.

---

## 2. Architecture

### 2.1 Single-Writer SQLite Worker

A dedicated tokio task owns the single writable `rusqlite::Connection`. The GTK thread holds an `mpsc::Sender<Command>` and never touches the writable connection directly. Reads use a separate read-only connection pool the worker does not own. WAL mode is mandatory. This is the pattern shipped in Viaduct, Atrium, and Belfry Phase 1.

It matters acutely here because Conservatory has several independent producers writing concurrently:

- the **import / organizer** (new tracks, file moves, embedded-tag write-back),
- the **playback loop** (position persistence, play counts, listening sessions),
- the **podcast fetch loop** (new episodes, conditional-GET state, downloads),
- the **user** (metadata edits, shelf-genre changes, queue edits, triage).

```text
┌──────────────────────────────────────────────┐
│            Conservatory Engine (Rust)         │
│           (tokio multi-thread runtime)        │
├──────────────────────────────────────────────┤
│  [Import / Organizer]                         │
│   ├─ Tag reader/writer (lofty / symphonia)    │
│   ├─ Path-template engine (§5.1)              │
│   ├─ Shelf-genre resolver (§5.2)              │
│   └─ File mover (dry-run + undo journal)      │
├──────────────────────────────────────────────┤
│  [Playback Engine]                            │
│   ├─ libmpv host (libmpv2 crate)              │
│   ├─ Unified queue (PlayableItem, §6.1)       │
│   ├─ Music profile: gapless / ReplayGain      │
│   └─ Podcast profile: Smart Speed/Voice Boost │
├──────────────────────────────────────────────┤
│  [Podcast Fetch] (ported from belfry-core)    │
│   ├─ Per-show scheduler, conditional GET      │
│   └─ feed-rs + podcast: namespace handler     │
├──────────────────────────────────────────────┤
│  [Data Layer]                                 │
│   ├─ Writer task (rusqlite, WAL)              │
│   ├─ Read-only pool                           │
│   └─ FTS5 (tracks, albums, shows, episodes,   │
│            books)                             │
└──────────┬───────────────────────────────────┘
           │ (tokio mpsc + glib channel)
    ┌──────┴────────────────────┐
    │  GTK4 Main UI Thread      │
    └───────────────────────────┘
```

`LibraryChanges`, `PlaybackChanges`, and `JobProgress` are coalescing batch types delivered through a `glib::MainContext` channel. UI updates apply as deltas, never full reloads (the deadbeef-cui rebuild discipline: never scroll the facets back to the top on an unrelated event).

### 2.2 Crate Layout (the compile-time plugin model)

Six crates. **Music is the native program; podcasts and audiobooks are compile-time plugins**: feature-gated workspace crates, compiled into the binaries when their feature is on. The default build ships all three media types; `--no-default-features` is the music-only build, kept green by CI from day one. The plugin API is internal-only (first-party plugins, free to change between versions); there is no dynamic loading, no IPC, and no third-party plugin surface. The layout keeps the Belfry / Atrium discipline that every non-GUI surface stays CLI-testable:

- **`conservatory-core`** — headless data layer and the music-native engine. SQLite worker plus read pool; **all schema and migrations, including the podcast and audiobook tables** (the boundary rule below); tag read/write; the import pipeline, path-template engine, shelf-genre resolver, and file mover; the libmpv host plus **all playback profiles**, including the spoken-word Smart Speed / Voice Boost profile both plugins resolve against (it is filter-graph configuration the unified queue applies on advance, not plugin code); the unified queue model; cover-art decode plus accent extraction. GUI-free.
- **`conservatory-search`** — the Calibre-shaped search expression language (lex / parse / AST / evaluator / SQL translator), typed against Conservatory's domain (Track / Album / Artist / Show / Episode / Book). The grammar *shape* is ported from `atrium-search`; the implementation is independent so the projects evolve without coupling (the Belfry precedent; record it in `ATTRIBUTIONS.md`). Deliberately feature-free: every field, podcast and audiobook included, is always compiled in; a field over an absent or empty table matches nothing, which keeps trait-object field registration out of the hot path.
- **`conservatory-podcasts`** — plugin crate, filled at Phase 6: the absorbed Belfry subsystem (per-show fetch loop with conditional GET, `feed-rs` plus the `podcast:` namespace handler, Inbox → Queue → Played triage, OPML round-trip) and its heavy dependencies (`reqwest`, `feed-rs`, `quick-xml`, `ammonia`, `id3`, `oo7`), plus the podcast CLI verbs and the Podcasts tab.
- **`conservatory-audiobooks`** — plugin crate, filled at Phase 7: the tag + sidecar reader (§7.5), chapter resolver, book-state derivation, the audiobook CLI verbs, and the Audiobooks tab.
- **`conservatory-cli`** — headless binary: import, organize, search, tag, queue, podcast ops, stats. See §9. Defines the `podcasts` / `audiobooks` features (default on) that pull the plugin crates.
- **`conservatory`** — the GTK4 binary, with the same two features; the Podcasts and Audiobooks tabs exist only when their feature is on.

> **The boundary rule: plugins are code and dependencies, not the database.** All schema, including the podcast and audiobook tables and the unified `queue`, lives in `conservatory-core`'s single append-only migration ledger and applies in every build. Queue foreign keys therefore stay valid in all builds, `user_version` never diverges between a music-only and a full build opening the same library, and a music-only build simply has empty podcast/book tables. Plugin crates never own migrations.

> **No speculative extension traits.** Because the plugin API is internal-only, the seams between core and the plugin crates are firmed up when their consumers exist: the engine/queue seam at Phase 4, the first real plugin at Phase 6. Until then the plugin crates are dependency-isolated homes, not a trait API designed before its second consumer.

> **Library-graduation watch.** The path-template engine plus file mover (§5) is domain-light and could justify a standalone crate later, and the playback engine is now eyed by two efforts (this and Belfry's lineage). Neither is at graduation point yet; flag again if either grows a stable, reusable public surface.

### 2.3 Widget Tree

A top-level view switcher selects between **Music**, **Podcasts**, and **Audiobooks**, with a persistent Now-bar across all three.

```text
AdwApplicationWindow
├── AdwBreakpoint (narrow → split views collapse)
└── AdwToastOverlay
    └── AdwToolbarView
        ├── AdwHeaderBar (AdwViewSwitcher: Music | Podcasts | Audiobooks; search; jobs; menu)
        └── AdwViewStack
            ├── "Music"
            │   └── faceted Columns UI panes (1–5, §3.3) over a track list
            ├── "Podcasts"
            │   └── AdwNavigationSplitView (sidebar triage + episode list + detail)
            └── "Audiobooks"
                └── AdwNavigationSplitView (shelf grid + book detail + chapter list, §3.8)
        └── AdwBin (now_bar — persistent transport, the unified queue's head)
```

The Music view is the deadbeef-cui layout as a first-class window: N configurable hierarchical filter panes (default Genre → Album Artist → Album) feeding a sortable track list. The Podcasts view is Belfry's three-pane triage layout. The Audiobooks view is Cozy's shelf layout: a cover-grid library, a book detail pane with the chapter list and per-book speed / sleep controls, and the same filter bar as the other surfaces.

The Podcasts and Audiobooks tabs are plugin surfaces (§2.2): the view switcher offers only the tabs whose features are compiled in, and a music-only build opens straight into the Music view with no switcher.

The switcher follows current libadwaita idiom (1.4+): an `AdwViewSwitcher` (`policy = wide`) lives in the header bar's title-widget, and an `AdwBreakpoint` hides it and reveals a bottom `AdwViewSwitcherBar` once the window is too narrow for the header switcher (HIG: the switcher migrates to the bottom edge). `AdwViewSwitcherTitle` is deprecated and not used. Three settled details:

- **Bottom-bar stacking (an opinionated call, no GNOME precedent).** No shipping GNOME app pairs a persistent bottom transport bar with an adaptive bottom view switcher. The rule here: the Now-bar is the stable innermost bottom bar (always visible, closest to content); the `AdwViewSwitcherBar` reveals *beneath* it only at the narrow breakpoint. Locked by visual prototype when the shell is built (Phase 6b-i).
- **State retention.** `AdwViewStack` keeps each page's widget tree alive, so scroll position and selection survive switching away and back. Heavy pages (Podcasts, Audiobooks) are built lazily on their child's `::map` signal rather than eagerly at startup.
- **Keyboard.** `Alt+1` / `Alt+2` / `Alt+3` switch top-level views via a `win.view` action, mirroring `AdwTabView`'s `Alt+N` convention (GNOME has no standard for numeric view jumps; `Ctrl+N` is a browser-tab habit and is left free for the podcast triage lists, §3.7). See `docs/keymap.md`.

---

## 3. User Interface

### 3.1 Design Principles

- **Whitespace and colour with discipline.** libadwaita's restrained accent system; per-album accent extracted from cover art (the Hermitage median-cut pattern) used as an accent, not a brand colour. Desktop reading distances; AdwClamp-bounded list widths on ultrawide displays.
- **Every list is a queryable database.** The Calibre gift: filter bar with the full grammar (§3.4), sortable columns, multi-select bulk actions, saved Perspectives, on both the music and podcast surfaces.
- **Metadata-first browse for large collections.** The Columns UI facets (§3.3) are the primary music navigation, tuned for 50k-plus-track libraries the way deadbeef-cui is.
- **Every action visible and keyboard-accessible.** Framework's discipline: no hidden gestures; every swipe has a menu equivalent; keyboard-first works.

### 3.2 Layout (Music)

```text
┌─────────────────────────────────────────────────────────────┐
│ [≡]  Conservatory      ( Music | Podcasts )      [search][⚙] │
├───────────────┬───────────────┬───────────────┬─────────────┤
│ Genre         │ Album Artist  │ Album         │ Tracks      │
│ [All (842)]   │ [All (1203)]  │ [All (60)]    │ # Title  ★  │
│ Electronic    │ Boards of Can.│ Geogaddi      │ 1 ...    ●  │
│ Ambient       │ Brian Eno     │ Music Has ... │ 2 ...       │
│ Jazz          │ Aphex Twin    │ ...           │ ...         │
│ ...           │ ...           │               │             │
├───────────────┴───────────────┴───────────────┴─────────────┤
│ [▶] cover  Track — Artist     ◀◀  ▶  ▶▶   ReplayGain  3:41   │
└─────────────────────────────────────────────────────────────┘
```

Panes are configurable (1–5), each driven by a title-formatting-style field expression, exactly as deadbeef-cui configures its columns. Selecting in a pane filters the panes to its right and the track list. Multi-select aggregates (Ctrl/Shift-click). An `[All (N …)]` synthetic row tops each pane. Selection-change is debounced before downstream recompute (the deadbeef-cui invariant that keeps multi-select drags cheap on large libraries).

### 3.3 Faceted Browse (Columns UI)

The music browse model, lifted from `deadbeef-cui` and rebuilt over Conservatory's database instead of DeaDBeeF's medialib:

- Hierarchical panes; default Genre → Album Artist → Album → Tracks; user-reconfigurable order and field expressions, persisted.
- Multi-value tags split for faceting (a track tagged `Electronic; Ambient` appears under both Electronic and Ambient in the Genre pane) while the *shelf* genre that determines its file location stays single-valued (§5.2). Facets read raw tags; the filesystem reads the shelf genre. These are deliberately decoupled.
- Track counts per facet row, memoized.
- The track list is the leaf: sortable columns, multi-select, the same row affordances as Belfry's episode list (status glyph, rating, hover lift).

### 3.4 Filtering, Search, and Perspectives

One grammar, all three surfaces, the Atrium/Belfry shape. The filter bar above any list accepts the full expression language; `Ctrl+F` focuses it; there is no separate search mode.

| Field | Example |
|---|---|
| `artist:`, `albumartist:`, `album:`, `title:` | `albumartist:"Boards of Canada"` |
| `genre:`, `shelfgenre:` | `genre:ambient` (raw tag) vs `shelfgenre:Electronic` (filed-under) |
| `year:`, `added:` | `year:1998..2004`, `added:thisweek` |
| `rating:`, `bitrate:`, `duration:`, `format:` | `rating:>=4 AND format:flac` |
| `is:played`, `is:starred`, `is:queued` | `is:starred AND genre:jazz` |
| podcast fields (`show:`, `is:in_inbox`, `pub:` …) | as Belfry §3.7 |
| audiobook fields (`author:`, `narrator:`, `series:`, `is:finished`) | `author:"Brandon Sanderson" AND is:finished false` |

Boolean (`AND`/`OR`/`NOT`), match modifiers (substring / `=`exact / `~`regex / `?`fuzzy), comparison and range, date keywords, sort modifiers (`sort:KEY`, `sort:-KEY`). Forgiving parser: malformed input degrades to substring match with a yellow filter-bar tint, never an error. **Perspectives** are named saved expressions (Calibre saved searches, Atrium's term), stored as text and re-parsed on load so they inherit later grammar additions. A Perspective can target tracks, albums, or episodes and can be a queue source (§6.1).

### 3.5 Bulk Metadata Editing

Calibre's editor surface, music-shaped. Multi-select in any list, then edit fields across the selection: artist, album artist, album, year, genre (raw tags and shelf genre), rating, cover. Search-and-replace across a field. A change that alters the shelf genre or the album/artist path triggers a file move (§5.4), surfaced as a job with a dry-run preview and undo. Embedded-tag write-back (§5.5) is part of the same job.

### 3.6 Now Playing and the Queue

The unified queue (§6.1) is the spine. The Now-bar persists across Music and Podcasts; tapping it expands to a Now Playing surface (the Hermitage Codex moment: full-bleed cover, accent-tinted scrubber, queue tail peek). For episodes the surface adds chapters, show notes, and a Smart Speed indicator. For tracks it shows album context, ReplayGain state, the active EQ / DSP, and gapless status. The **sleep timer** (Belfry §3.6: 15 / 30 / 45 / 60 min, end of item, end of queue, tap-to-extend) is available for any playing item, not episodes alone (falling asleep to an album is a real use case and the engine is media-agnostic); it lives on the Now-bar as a menu, the boundary label following the playing kind ("End of track" / "End of episode" / "End of book"). The queue view itself is a single list that interleaves tracks and episodes, drag-reorderable, each row badged with its kind.

### 3.7 Podcasts Tab

Belfry's Inbox → Queue → Played triage, intact (Belfry §3.3–3.4): a sidebar of triage lists, shows, and tags; an episode list; a detail/now-playing pane. Per-show overrides for speed, Smart Speed, Voice Boost, skip, retention, and inbox policy. The only structural change from Belfry is that the **Queue is the shared unified queue**, so an episode and an album track can sit next to each other in it.

### 3.8 Audiobooks Tab

Cozy's library, rebuilt over Conservatory's database. A **shelf grid** of book covers (accent-tinted, the Hermitage unit) is the primary surface; selecting a book opens a detail pane with its **chapter list**, progress, author and narrator, series and sequence, and per-book **playback speed** and **sleep-timer** controls. The same filter bar and grammar (§3.4) apply, with the audiobook fields (`author:`, `narrator:`, `series:`, `is:finished`).

- **The book is the unit.** A book is one logical item with ordered chapters. Chapters come from either embedded M4B markers or a multi-file folder (one file per chapter); the engine treats both identically (§6.1). A book is a *single* entry in the unified queue, and chapter navigation happens *within* the playing item, not by enqueueing each chapter separately.
- **Resume is first-class.** Unlike a music track, you always resume a book where you left off, to the second, across restarts (§6.4). The shelf surfaces in-progress books first.
- **Owned, not ephemeral.** Audiobooks are curated and moved into the managed tree like music (§5.7), not treated as disposable downloads like podcast episodes. The file mover (§5.4) and embedded-tag write-back (§5.5) apply.
- **Triage is lighter than podcasts.** Books have a simple New / In progress / Finished state derived from progress, not the full Inbox → Queue → Played model; they enter the unified queue on demand.

---

## 4. Data Model

WAL mode, foreign keys on, `synchronous=NORMAL`. Single writer; read commands open read-only at the process level. FTS5 on titles. Migrations versioned via `user_version`, append-only and backwards-compatible post-1.0 (the Atrium discipline). All migrations live in `conservatory-core` and apply in every build, plugin features on or off (the §2.2 boundary rule): the podcast and audiobook tables exist, empty, even in a music-only build, so `user_version` never diverges between builds opening the same library.

### 4.1 Music (draft schema)

```sql
CREATE TABLE artists (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL,
    sort_name       TEXT NOT NULL,      -- "Beatles, The"; drives path + sort (Calibre author_sort)
    musicbrainz_id  TEXT,
    UNIQUE (sort_name)
);

CREATE TABLE albums (
    id                  INTEGER PRIMARY KEY,
    title               TEXT NOT NULL,
    album_artist_id     INTEGER REFERENCES artists(id),  -- NULL => Various Artists bucket
    shelf_genre         TEXT,           -- THE ONLY input to the genre folder level; editable (§5.2)
    year                INTEGER,
    release_date        TEXT,
    musicbrainz_release_id TEXT,
    cover_path          TEXT,
    accent_rgb          INTEGER,        -- packed RGB, median-cut from cover
    folder_path         TEXT NOT NULL,  -- managed; rendered from the template (§5.1)
    added_at            INTEGER
);

CREATE TABLE tracks (
    id              INTEGER PRIMARY KEY,
    album_id        INTEGER REFERENCES albums(id) ON DELETE CASCADE,
    artist_id       INTEGER REFERENCES artists(id),  -- track artist (may differ from album artist)
    title           TEXT NOT NULL,
    track_no        INTEGER,
    disc_no         INTEGER,
    duration        REAL,               -- seconds
    file_path       TEXT NOT NULL,      -- managed; under the album folder
    format          TEXT,               -- flac/mp3/opus/aac/...
    bitrate         INTEGER,
    sample_rate     INTEGER,
    replaygain_track REAL,
    replaygain_album REAL,
    rating          INTEGER DEFAULT 0,  -- 0–5; foobar/Lattice loanword
    play_count      INTEGER DEFAULT 0,
    last_played     INTEGER,
    starred         INTEGER DEFAULT 0,
    musicbrainz_recording_id TEXT,
    added_at        INTEGER
);

-- Raw multi-value genres, preserved untouched for facets + search. NOT the shelving input.
CREATE TABLE genres (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL);
CREATE TABLE track_genres (
    track_id INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
    genre_id INTEGER REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (track_id, genre_id)
);

-- Genre normalization (§5.2). Source of the seed map is OPEN (§16).
CREATE TABLE genre_aliases   (raw TEXT PRIMARY KEY, canonical TEXT NOT NULL);
-- User priority list, tie-breaks shelf-genre derivation when track genres disagree.
CREATE TABLE genre_priority  (genre TEXT PRIMARY KEY, rank INTEGER NOT NULL);
```

### 4.2 Podcasts

Ported from Belfry §4.1 (`shows`, `episodes`, `playback`, `show_settings`, `listening_sessions`, `chapters`, `tags`, `show_tags`), with one change: triage Queue state is represented through the unified `queue` table (§4.3) rather than a per-episode `in_queue` flag. The rest is unchanged, including the append-only `listening_sessions` discipline.

### 4.3 Unified Queue and Playback Bridge

```sql
-- One ordered queue across both media types. The bridge that makes the unified queue real.
CREATE TABLE queue (
    id          INTEGER PRIMARY KEY,
    position    INTEGER NOT NULL,       -- explicit, drag-reorderable
    kind        TEXT NOT NULL CHECK (kind IN ('track','episode','audiobook')),
    track_id    INTEGER REFERENCES tracks(id)   ON DELETE CASCADE,
    episode_id  INTEGER REFERENCES episodes(id) ON DELETE CASCADE,
    book_id     INTEGER REFERENCES books(id)    ON DELETE CASCADE,  -- audiobook = one queue entry (§3.8)
    CHECK ( (kind='track'     AND track_id   IS NOT NULL AND episode_id IS NULL AND book_id IS NULL)
         OR (kind='episode'   AND episode_id IS NOT NULL AND track_id   IS NULL AND book_id IS NULL)
         OR (kind='audiobook' AND book_id    IS NOT NULL AND track_id   IS NULL AND episode_id IS NULL) )
);
CREATE INDEX idx_queue_position ON queue(position);
```

The engine reads `queue` into an in-memory `Vec<PlayableItem>` (§6.1); position writes are debounced. A whole audiobook is a single queue entry; its chapters are navigated within the item, not enqueued one by one (§3.8). Resume position for long items (mixes, episodes, books) lives in the per-kind state tables (`tracks.last_played` / Belfry's `playback` / the `book_playback` table, §4.5).

### 4.4 FTS5

`track_fts` (title, artist, album), `album_fts` (title, album artist), plus Belfry's `episode_fts` and `show_fts`, plus `book_fts` (title, author, narrator, series). Triggers keep them in sync. Not transcripts (§14).

### 4.5 Audiobooks (draft schema)

Modeled on Audiobookshelf's relational shape and Cozy's Book → Chapter → file model. A **book** is the unit; **chapters** are ordered and come from either embedded M4B markers or one-file-per-chapter folders; **authors** and **narrators** are distinct roles (many-to-many); **series** carries a decimal sequence. Resume state is a single row per book (the podcast `playback` analogue, never lost).

```sql
CREATE TABLE book_people (              -- authors and narrators share a table, role-tagged
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    sort_name   TEXT NOT NULL,          -- "Sanderson, Brandon"; drives path + sort
    UNIQUE (sort_name)
);

CREATE TABLE series (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    UNIQUE (name)
);

CREATE TABLE books (
    id              INTEGER PRIMARY KEY,
    title           TEXT NOT NULL,
    subtitle        TEXT,
    series_id       INTEGER REFERENCES series(id),
    series_sequence REAL,               -- decimal: "Book 1.5"
    year            INTEGER,
    publisher       TEXT,
    isbn            TEXT,
    asin            TEXT,
    description     TEXT,
    language        TEXT,
    shelf_genre     TEXT,               -- same decoupling as music (§5.2); single-valued path input
    cover_path      TEXT,
    accent_rgb      INTEGER,            -- packed RGB, median-cut from cover (§7.4)
    folder_path     TEXT NOT NULL,      -- managed; rendered from the audiobook template (§5.7)
    rating          INTEGER DEFAULT 0,
    starred         INTEGER DEFAULT 0,
    added_at        INTEGER
);

-- Author / narrator links (role-tagged many-to-many).
CREATE TABLE book_authors (
    book_id   INTEGER REFERENCES books(id)        ON DELETE CASCADE,
    person_id INTEGER REFERENCES book_people(id)  ON DELETE CASCADE,
    PRIMARY KEY (book_id, person_id)
);
CREATE TABLE book_narrators (
    book_id   INTEGER REFERENCES books(id)        ON DELETE CASCADE,
    person_id INTEGER REFERENCES book_people(id)  ON DELETE CASCADE,
    PRIMARY KEY (book_id, person_id)
);

-- Ordered chapters. `file_path` + `file_offset` lets one row address either a
-- standalone per-chapter file (offset 0) or a span inside a single M4B.
CREATE TABLE book_chapters (
    id          INTEGER PRIMARY KEY,
    book_id     INTEGER REFERENCES books(id) ON DELETE CASCADE,
    idx         INTEGER NOT NULL,       -- 0-based order within the book
    title       TEXT,
    file_path   TEXT NOT NULL,          -- managed; under the book folder
    file_offset REAL NOT NULL DEFAULT 0,-- seconds into file_path where this chapter starts
    duration    REAL,                   -- seconds
    UNIQUE (book_id, idx)
);

-- First-class resume (§6.4). One row per book; never append-only (a book is one thing you resume).
CREATE TABLE book_playback (
    book_id        INTEGER PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    position       REAL NOT NULL DEFAULT 0,  -- absolute seconds across the whole book
    finished       INTEGER NOT NULL DEFAULT 0,
    last_played    INTEGER,
    speed          REAL,                     -- per-book override; NULL = global default
    smart_speed    INTEGER,                  -- per-book override; NULL = global default
    voice_boost    INTEGER                   -- per-book override; NULL = global default
);
```

A book's `format`, `bitrate`, and `sample_rate` are read per chapter file at import; the engine resolves the book's total duration by summing chapter durations. The curated layer that a re-import cannot rebuild (rating, starred, `book_playback`, shelf-genre override) is what the backup protects (§5.6).

---

## 5. Library and Filesystem Layout (the file-ownership model)

This is the section that distinguishes Conservatory from everything else Brandon has built. The application owns the on-disk layout and moves files to match the database.

### 5.1 Music On-Disk: a rendered template

**Each media type lives under its own top-level folder beneath the library root:** `Music/` (this section), `Audiobooks/` (§5.7), and `Podcasts/` (§5.3). So one library root cleanly holds all three side by side.

The database is truth; the on-disk tree is a *render* of a configurable path template, exactly as Calibre's "save to disk" template and beets' `paths:` config work. The default:

```text
<library root>/
└── Music/
    └── <Shelf Genre>/
        └── <Album Artist sort_name>/
            └── <Album> (<Year>)/
                ├── NN - <Title>.<ext>
                └── cover.jpg
```

Default template string: `Music/{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}`. Because the layout is a render of the database, re-shelving an album is a *template-or-field change*, not a lock-in. The template is user-editable; `{shelf_genre}` is the only piece that depends on the genre decision in §5.2.

> **Implemented in v0.0.23:** `DEFAULT_MUSIC_TEMPLATE` carries the `Music/` prefix. A library managed by an earlier build re-shelves into `Music/` on its next `organize` (the move is journaled + undoable like any other).

An **album is the unit that moves.** A single album resolves to exactly one path: one shelf genre and one album artist drive the directory, even when track-level genres or artists disagree. Compilations resolve their album-artist component to a **Various Artists** bucket.

### 5.2 Shelf-Genre Resolution

The riskiest design call, because genre is the least canonical, most multi-valued tag. The resolution keeps the raw tags off the filesystem entirely.

- Each album has an editable **`shelf_genre`** field (the Calibre `author_sort` trick: the shelving key is a separate field, not the raw tag). It is the only input to the genre folder level. The decision is **flat**: the normalized `shelf_genre` value is used verbatim as the top folder. (A genre *tree* with rollup, where leaf tags like Synthwave collapse to a coarse parent like Electronic, was considered and deferred to a possible v2, §16.)
- `shelf_genre` is auto-filled on import by a priority chain:
  1. a manual override, if the user has set one;
  2. else a single album-level genre tag, if present;
  3. else the most common normalized genre across the album's tracks, ties broken by the user's `genre_priority` list, then first;
  4. else an `Unknown` bucket.
- A **normalization layer** runs before the chain: split on `;` `/` `,`, case-fold, then map through `genre_aliases` (for example `IDM` → `Electronic`, `Hip Hop`/`Rap` → one canonical). The seed source for that alias map is open (§16).
- `shelf_genre` is always overridable per album and bulk-editable. Editing it moves the album's directory (§5.4). Raw `track_genres` are never touched by any of this; they exist for facets and search.

### 5.3 Podcasts On-Disk: managed downloads

Podcasts adopt the managed model: `<library root>/Podcasts/<show-slug>/<YYYY-MM-DD>--<episode-slug>/`. Belfry's filesystem-canonical guarantee ("the library survives the database") is **dropped for podcasts**, accepted because downloaded episodes are ephemeral, not curated. Streaming still works before/without download (Belfry §6.2): if the local file is absent and a URL is present, libmpv streams with range requests.

### 5.4 Import, Organize, and Move Pipeline

1. **Scan or drop** files or folders.
2. **Read tags** (lofty / symphonia, §7.1).
3. **Resolve** artists/albums/genres into the database; derive `shelf_genre` (§5.2).
4. **Render** the target path from the template (§5.1).
5. **Move or copy** into the managed tree. In-place vs copy-on-import is a per-import user choice (copy leaves the originals untouched; move consumes them).

Every operation that relocates files is a **job** with: a **dry-run preview** of exactly which files move where, an **undo journal** so the move is reversible, conflict handling (duplicate target paths, read-only sources, cross-filesystem moves), and crash safety (the journal is written before the move, replayed on restart). Moving a user's real files is a trust commitment; the dry-run and undo are not optional. **Implemented at Phase 2c** (`conservatory-core/src/mover/`, journal in migration `0002`); the journal is a SQLite ledger and recovery rolls forward (docs/mover.md).

### 5.5 Embedded-Tag Write-Back and Portability

Although the database owns organization, Conservatory **writes curated metadata back into the files' embedded tags** (a Calibre "embed metadata" analogue), so the files remain portable and self-describing outside the app. Write-back is a job, batched, and respects format capabilities (Vorbis comments, ID3, MP4 atoms). This is what keeps the file-ownership model from being a roach motel: you can always walk away with tagged files.

### 5.6 Re-Import Contract

Conservatory is not filesystem-canonical, so the Belfry rescan contract does not apply unchanged. The contract here is weaker and explicit: **the managed tree plus embedded tags can rebuild a library's tracks, albums, and artists**; the database-exclusive data that a re-import cannot recover is the *curated* layer (shelf-genre overrides, ratings, play counts, starred, Perspectives, queue, podcast triage/listening state). That curated layer is what the nightly DB backup and the JSON export protect. The integration suite verifies the rebuildable subset against a fixture library.

### 5.7 Audiobooks On-Disk: a rendered template (owned, like music)

Audiobooks are curated, not ephemeral, so they adopt the **music model, not the podcast model**: the database owns the layout and the file mover (§5.4) relocates them, with dry-run and undo. They are *not* managed downloads (§5.3). The default template, modeled on the Audiobookshelf convention:

```text
<library root>/
└── Audiobooks/
    └── <Author sort_name>/
        └── <Series, or "Standalone">/
            └── <NN. ><Title> (<Year>)/
                ├── <chapter files or book.m4b>
                └── cover.jpg
```

Default template string: `Audiobooks/{author}/{series}/{series_index2}. {title} ({year})`. A book always sits under a series level: a series book uses its series name, a standalone book uses the literal **`Standalone`** folder (so every author folder has the same two-level shape, `Author/<series-or-Standalone>/Title`). `{series_index}` collapses cleanly when there is no number (no stray `NN.` separator, the §5.1 sanitization rule). A book resolves to exactly one path: one author component (the first credited author's `sort_name`, multi-author books bucket under the primary), one series-or-Standalone level. New path tokens (`{author}`, `{narrator}`, `{series}`, `{series_index}`) are documented in `docs/path-template.md`. Single-file M4B books keep their one file inside the book folder; multi-file books keep their chapter files there. As with music, `shelf_genre` (single-valued) is available as an optional template token but is not in the default audiobook layout.

---

## 6. Playback Engine

A single libmpv instance kept alive across items (the `libmpv2` binding, property API plus filter graph).

### 6.1 PlayableItem and the Unified Queue

The engine plays a queue of:

```rust
struct PlayableItem {
    path_or_url: Source,     // local managed file, or stream URL for an undownloaded episode
    kind: MediaKind,         // Track | Episode | Audiobook
    profile: PlaybackProfile // resolved per-kind + per-show/per-album/per-book overrides
}
```

The queue (§4.3) interleaves all three kinds freely. On advance, the engine applies the item's profile (the right `af` filter chain, ReplayGain, gapless behaviour) before playing. This single abstraction is what lets one queue, one Now-bar, one MPRIS surface, and one set of media keys serve music, podcasts, and audiobooks. An `Audiobook` item spans its book's ordered chapters (§4.5): chapter advance is *internal* to the item (no gap, the chapter boundary is just a seek across files or within an M4B), and the queue advances to the next item only when the book finishes.

### 6.2 Music Profile

Resolved into a labelled `af` filter chain built once per item and tuned at runtime via `af-command` (so a slider move never tears down the graph and clicks the audio). This section settles §16.6; Phase 5.5 builds it.

- **Gapless** playback within an album: `--gapless-audio=weak` (preserves the source rate across a mixed-rate library; `audio-samplerate` / `audio-format` stay unset to avoid needless resampling). **Crossfade is deliberately not offered**: it is impossible in a single libmpv instance (the engine decodes one playlist entry at a time, so two tracks never overlap) and is mpv-maintainer-rejected. Conservatory ships gapless-only, the path real mpv-based players take.
- **ReplayGain** applied as an explicit `volume` stage at the *head* of the chain, from the `tracks.replaygain_*` values (scanned in-app via rsgain, §16.7), with a user preamp and clip-prevention. This is preferred over mpv's built-in `--replaygain`, which is applied *after* the `af` chain (a boosting EQ would defeat clip-prevention) and is not re-applied per track across a gapless boundary (the whole queue would inherit the first track's gain, mpv bug #8267). Modes off / track / album as before.
- **Equalizer**: a graphic EQ (stacked `equalizer` peaking bands at ISO centres) plus a parametric option (`anequalizer`), with named presets. The obvious-looking `superequalizer` / `firequalizer` are avoided: they carry no runtime command, so every adjustment would rebuild the graph and gap the audio.
- **DSP modules**: an optional, ordered set of chain stages — compressor (`acompressor`), brick-wall limiter, volume leveler (`dynaudnorm`, single-pass/live) — each independently toggleable. A bounded, useful chain, **not** a deadbeef-class everything. Deferred and recorded (not built in v1): exclusive/bit-perfect output, LADSPA / raw-`af` plugin hosting, and native `crossfeed` for headphones (§16.6).

### 6.3 Podcast Profile

Smart Speed (silence-skip via `silenceremove`) and Voice Boost (compression + EQ + loudness leveling), ported from Belfry §5.1–5.3, including the time-saved session accounting. Per-show overrides as in Belfry. These are **presets on the Phase 5.5 `af`-chain engine** (§6.2), not a separate path; two filter choices are validated against the Phase 5.5 findings (`docs/libmpv-profiles.md`): variable speed via mpv `--speed` + `audio-pitch-correction` (scaletempo2) rather than a chained `rubberband` at all speeds, and live single-pass `dynaudnorm` rather than two-pass/offline `loudnorm`.

**Audiobooks share this profile.** An audiobook is long-form speech, so it uses the same spoken-word filter graph (variable speed, Smart Speed, Voice Boost), resolved with per-book overrides from `book_playback` (§4.5) instead of per-show ones. The only audiobook-specific behaviour is chapter navigation within the item (§6.1) and the first-class resume in §6.4; no new filter chain is introduced.

### 6.4 State Persistence

Position written on pause, seek (debounced), item end, app quit, and every 30 s during playback (the Belfry insurance interval). Resume offset of a few seconds for context on long items. Music play counts and `last_played` update on completion; episode listening sessions are append-only (Belfry §5.4). Audiobook position is stored as an absolute offset across the whole book in `book_playback.position` (§4.5), with `finished` set on completion; a book is the canonical "resume where I left off" case, so its position write is not best-effort but the same insurance-interval discipline as everything else.

### 6.5 System Integration

MPRIS2 (`org.mpris.MediaPlayer2`) with full metadata for the current item regardless of kind; play/pause/next/previous/seek; exposure to GNOME's media overlay, lock screen, and headset buttons. PipeWire output-sink picker. Suspend inhibitor during active playback.

Output selection covers the **device** (the PipeWire picker, Phase 4c-ii) and the **backend** (`--ao=pipewire|pulse|alsa|jack`), with high-quality resampler control for the unavoidable-resample case (Phase 5.5c). An exclusive / bit-perfect mode (ALSA `hw:` + `--audio-exclusive`) is **deferred and recorded** (§16.6): it is bare-install-only and fights the Flatpak sandbox, so the PipeWire path stays the everyday default.

---

## 7. Tagging and Metadata

### 7.1 Tag Read/Write

`lofty` (broad format coverage, read + write) is the leading candidate; `symphonia` is the fallback/decoder reference. Subject to the dependency sign-off rule (§11). Reads feed import; writes feed embedded-tag write-back (§5.5).

### 7.2 Genre Normalization

The alias map and priority list of §5.2. **Settled (§16.4):** Conservatory starts empty and user-built, shipping no default vocabulary; the schema supports seeding one (beets' `lastgenre` whitelist or the MusicBrainz genre list) later without a migration.

### 7.3 MusicBrainz

Tagging from a canonical source (matching tracks to MusicBrainz, fetching authoritative metadata and cover art) is **out of the core scope by default** and tracked as an open decision (§16). It is exactly Picard's domain and a deep matching problem; Conservatory assumes files arrive reasonably tagged unless and until this is taken on.

### 7.4 Cover Art and Accent

Cover art is the visual unit (Hermitage). On import, extract or locate cover art, store at `cover.jpg` in the album (or book) folder, and compute a dominant-hue accent (median-cut quantizer) into `albums.accent_rgb` / `books.accent_rgb` for the browse and Now Playing surfaces.

### 7.5 Audiobook Metadata

Audiobook tags are notoriously sparse and inconsistent, so Conservatory reads from several local sources, in priority order, and never reaches the network in v1:

1. **Embedded tags** in the M4B / MP3 files (title, author from artist/album-artist, narrator from composer, series and sequence where present, year, publisher).
2. **Sidecar files** in the book folder, the Audiobookshelf conventions: `.opf` (parsed for the full metadata set, via the already-present `quick-xml`), `desc.txt` (description), `reader.txt` (narrator), `cover.jpg` (cover).
3. **Folder structure** as a last resort: `Author/Series/Title (Year)/` parsed for the fields the tags and sidecars did not supply.

Anything still missing is filled by hand in the detail pane; bulk editing (§3.5) applies. **Chapters** come from embedded M4B markers first, then from a one-file-per-chapter folder layout; deriving chapters by silence detection (the m4b-tool technique) is an optional, opt-in step left **open** (§16.11). **Online metadata providers** (Audible / Audnexus / Google Books, the Audiobookshelf model) are out of v1 scope and tracked as an open decision (§16.10), the audiobook analogue of the MusicBrainz question (§7.3): Conservatory assumes reasonably-tagged or reasonably-foldered files unless and until a provider is taken on.

---

## 8. Podcasts (absorbed from Belfry)

The podcast subsystem is Belfry, ported: per-show polling with conditional GET and jittered intervals; HTTP Basic auth with credentials in libsecret (`oo7`); `feed-rs` plus a hand-rolled `podcast:` namespace handler; episode identity by `(show_id, guid)`; three-source chapter precedence; OPML round-trip preserving tags and `applePodcastsID`. See Belfry `spec.md` §7 for the exhaustive contract; that detail migrates into this document as the absorption is implemented, at which point Belfry's spec is superseded.

---

## 9. CLI

`conservatory-cli` ships alongside the GUI (the Hermitage / CalibreQuarry / Belfry pattern: GUI to browse, CLI to batch).

```text
conservatory-cli import <path> [--copy|--move] [--dry-run]
conservatory-cli organize [--dry-run]          # re-render the tree from the DB
conservatory-cli search '<expression>'         # the §3.4 grammar
conservatory-cli tag set <selector> field=value...
conservatory-cli shelf-genre set <album-selector> <genre>
conservatory-cli queue add|remove|reorder|list <selector>
conservatory-cli play <selector>               # hand off to standalone mpv
conservatory-cli stats                         # library + listening stats
conservatory-cli audit [--tier ...] --root R   # health audits (Phase 8c)
conservatory-cli apestrip --root R [--apply|--undo]  # strip stray APEv2 (Phase 8c-iii)
conservatory-cli podcast add|remove|refresh|download <spec>   # Belfry verbs
conservatory-cli import-opml|export-opml
conservatory-cli audiobook import <path> [--copy|--move]      # import a book (folder or m4b)
conservatory-cli audiobook set <book-selector> field=value... # author/narrator/series/sequence/...
conservatory-cli embed-tags <selector> [--dry-run]   # write DB metadata into files
conservatory-cli backup|restore                # DB snapshot
```

Read commands open the DB read-only at the process level. Write commands spin up the worker on a current-thread runtime and shut down cleanly (the Atrium/Belfry pattern). Output: `--tsv` (default), `--json`, `--human`.

---

## 10. Configuration

`~/.config/conservatory/config.toml`, optional, sane defaults.

```toml
[library]
root = "~/Music"
path_template = "Music/{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}"
import_mode = "copy"          # "copy" | "move"
embed_tags_on_edit = true

[genre]
default_unknown = "Unknown"
# seed vocabulary source is OPEN (§16); empty until decided

[playback]
gapless = true                # --gapless-audio=weak; crossfade is not offered (§6.2)
replaygain = "album"          # "off" | "track" | "album"
replaygain_preamp = 0.0       # dB, applied at the head volume stage (§6.2)
replaygain_clip = "prevent"   # "prevent" | "allow"
# podcast Smart Speed / Voice Boost defaults inherited from Belfry config shape

[audio]                       # the DSP chain + output (§6.2, §6.5; Phase 5.5)
output_backend = "auto"       # "auto" | "pipewire" | "pulse" | "alsa" | "jack"
eq_preset = "flat"            # named EQ preset; "flat" is a no-op chain
# EQ bands and compressor / limiter / leveler settings live with the preset, not inline

[scrobble]                    # optional, off by default (§14, Phase 9)
enabled = false
service = "listenbrainz"      # "listenbrainz" | "lastfm"
# token stored in libsecret, not the config file

[podcasts]
library_subdir = "Podcasts"
max_concurrent_downloads = 3

[audiobooks]
library_subdir = "Audiobooks"
path_template = "Audiobooks/{author}/{series}/{series_index:02}. {title} ({year})"
default_speed = 1.0           # per-book overrides live in book_playback (§4.5)
smart_speed = true            # spoken-word profile shared with podcasts (§6.3)
voice_boost = false
```

---

## 11. Dependencies

Backend (Rust): `tokio`, `rusqlite` (bundled, FTS5), `libmpv2`, `lofty` (and/or `symphonia`), `reqwest` (conditional GET, Basic auth), `oo7` (libsecret), `feed-rs` + `quick-xml` (podcasts), `ammonia` (show-note sanitize), `id3` (chapter fallback), `image` (cover decode/accent), `serde`/`serde_json`/`toml`, `regex`, `unicode-normalization` (Phase 8b dedup NFKC key folding), `tracing`, `zbus` (MPRIS + inhibitor). A MusicBrainz client crate only if §7.3 is taken on.

Frontend: `gtk4` (≥ 4.16), `libadwaita` (≥ 1.7), system `libmpv` (0.36+) with the ffmpeg filter library (`silenceremove`, `rubberband`, `acompressor`, `equalizer`, `loudnorm`), `libsecret` (via `oo7`).

External tools (shelled out, not linked, ATTRIBUTIONS.md): `rsgain` (ReplayGain scan, §16.7), `ffprobe` (embedded-M4B chapters, §3.8), and `flac` + `ffmpeg` (the Phase 8a integrity audit: `flac -t` test-decodes / MD5-verifies FLAC, `ffmpeg` strict-decodes the rest to a null sink; §8). A missing tool degrades gracefully or fails with a helpful message; none is required for normal playback.

**No third-party crate or system library lands without prior sign-off.** Brandon's standing rule; doubly noted here because this list is longer than usual and still partly provisional.

---

## 12. Flatpak Distribution

Flatpak-first. App ID `org.gnome.Conservatory` if accepted into GNOME Circle, else `io.github.virinvictus.Conservatory`. Permissions kept tight: `network` (feeds, downloads, streaming), file access to the library root (via portal where possible), `pulseaudio`/PipeWire socket (playback), `org.freedesktop.secrets` (Basic-auth credentials), `org.freedesktop.portal.FileChooser` (import, OPML). Portal-mediated background execution for periodic feed refresh is post-1.0 polish.

---

## 13. Memory and Performance Targets

Conservatory carries no WebKit. It must stay responsive on large libraries (the deadbeef-cui design target is 50k-plus tracks).

- **Idle (no playback), 50k-track library warm:** < 200 MB.
- **Playback active, Now Playing open:** < 300 MB.
- **Cold start to first interactive frame, 50k-track library:** < 500 ms.
- **Facet selection-change to track-list repaint (50k tracks):** < 100 ms (the debounce plus memoized counts make this feasible; deadbeef-cui hits it in C).
- **Position-write latency (pause → committed):** < 50 ms.

GTK4 + libadwaita pull a ~150 MB C-side floor (measured in Viaduct); the targets account for it. Each phase ends with a `heaptrack` / `massif` note; features that miss budget get gated.

---

## 14. Out of Scope, Forever

- Recommendations, "discover" tabs, charts, in-app social feeds, a social graph, sharing-to-followers. (Narrow carve-out: *optional, off-by-default* listening-history scrobbling to ListenBrainz / Last.fm is allowed as a one-way sync protocol, Phase 9; with it disabled the app is unchanged and fully offline. This is a deliberate, scoped reversal of the original blanket "no social" line, kept to history submission, not a social product.)
- A built-in music or podcast directory beyond import / OPML.
- Cloud anything that is not an optional sync protocol; the app is fully functional offline.
- DRM.
- Transcripts (Belfry's stance inherited; 2.x maybe at most).
- Video as a first-class format (audio extraction only).
- Windows / macOS; GNOME-native, deliberately.
- Becoming a from-scratch metadata authority. Even if MusicBrainz tagging (§7.3) or an audiobook metadata provider (§7.5, §16.10) is taken on, Conservatory consumes a canonical source; it does not try to out-Picard Picard on match quality.
- Audiobook DRM. Audible / OverDrive de-DRM and account-linked download (Libation's domain) are out; Conservatory imports DRM-free files (restating the DRM non-goal above, for audiobooks).

---

## 15. Naming, Branding, License

**Conservatory.** A building where a collection is kept and cultivated, and a school of music; it fits the architectural-structure naming line (Atrium, Belfry, Hermitage, Framework, Lattice, Viaduct) while reading as music-coded. The icon should evoke architecture (a glasshouse or hall), not audio clichés: no waveforms, headphones, or play triangles. A conservatory silhouette in libadwaita accent does the work (the Belfry branding discipline).

App ID: `org.gnome.Conservatory` (GNOME Circle) or `io.github.virinvictus.Conservatory`.

**License: GPL-3.0-or-later.** Forced by the GPL libraries the player links, not by a call we make: libmpv links a GPL ffmpeg build (the `silenceremove` / `acompressor` / `equalizer` / `dynaudnorm` / `volume` filters the chain rides) and librubberband (GPL-2-or-later) where the build carries it. As of Phase 6c-i Conservatory no longer invokes the `rubberband` filter itself (Smart Speed is `silenceremove`, variable speed is `scaletempo2`), but the obligation flows from linking the stack, the same constraint Belfry documents. No license relaxation without an mpv/ffmpeg build stripped of its GPL components. Record the full chain in `ATTRIBUTIONS.md`.

---

## 16. Risks and Open Questions

1. **Scope.** This is the largest thing in Brandon's backlog: a music library manager, a daily-driver player, an absorbed podcast client, and an audiobook library, all sharing a unified queue. The audiobook tab (§3.8, added after the initial design) widens the scope further, mitigated by it being a thin layer over the already-absorbed spoken-word engine and landing last (Phase 7). It competes with Atrium (still pre-1.0) for the "one big project" slot, and as of v0.0.1 the build has begun *concurrently* with Atrium rather than after it. That concurrency is the risk this section originally warned against; it is now accepted by deliberate decision. The mitigation is no longer deferral but hard phasing (§17): every phase must leave a usable artifact, so attention can swing back to Atrium between phases without leaving Conservatory half-built. If the concurrency proves to be a mistake, the phasing is what makes a pause cheap.
2. **Moving the user's files.** The file-ownership model is the headline risk. A move bug damages a real library. The dry-run, undo journal, and crash-safe replay (§5.4) are release-blocking, not nice-to-have.
3. **Genre instability.** Genre-first physical shelving amplifies the least stable tag into file moves. The shelf-genre field plus rendered template (§5.1–5.2) keep raw tags off disk and make re-shelving cheap, but this is the part most likely to need revision in practice. The genre-tree rollup is the escape hatch if flat shelving churns too much.
4. **Genre vocabulary seed (settled, Phase 2b).** Start **empty and user-built**: Conservatory ships no default alias map or whitelist. `shelf_genre` derives from the raw tags as they are (case-folded, split, deduped), and `genre_aliases` / `genre_priority` are populated only as the user maps them. This avoids vendoring and maintaining a third-party list and avoids baking in someone else's genre opinions; the schema already supports seeding a vocabulary later (beets `lastgenre` or MusicBrainz) without a migration, so the door is open if curation friction demands it.
5. **MusicBrainz tagging (OPEN).** In scope or assume pre-tagged files? Default is out; revisit if curation friction demands it.
6. **EQ / DSP depth + output quality (RESOLVED, Phase 5.5).** A real but bounded chain, not a deadbeef-class everything: a graphic + parametric equalizer and an ordered set of DSP modules (compressor, limiter, `dynaudnorm` leveler), built as a labelled `af` chain mutated via `af-command`, with ReplayGain re-staged at the chain head to fix mpv's post-`af` / gapless-boundary gain bug (#8267). Output gains a backend picker and resampler control (§6.5). Deferred and recorded (roadmap Phase 5.5c): exclusive/bit-perfect output, LADSPA / raw-`af` plugin hosting, `crossfeed`. **Crossfade is dropped** (impossible in one libmpv instance). The chain engine is shared with the spoken-word profile (§6.3), which is why it lands before the Phase 6c spoken-word chain (§17): the podcast manager and triage (6a/6b) are independent of the audio engine and may ship first.
7. **ReplayGain scan vs read (settled, Phase 5c, shipped).** Conservatory **scans in-app** via **`rsgain`**, it does not only read. The scanner computes album + track gain/peak for untagged albums and writes the tags, refreshing the DB `replaygain_*` columns so the playback profile resolution (§6.2) sees them; the read-only path (Phase 4a) stays the default and the scan is opt-in maintenance. `rsgain` (an external tool, ATTRIBUTIONS.md) was chosen over the `ebur128` Rust crate: the crate measures only decoded PCM and the pure-Rust decoder (symphonia) cannot decode Opus, which is a large fraction of the library, whereas rsgain decodes every format itself and writes correct RG2.0 tags including the Opus R128 convention and album gain.
8. **Belfry absorption timing (resolved, v0.0.52).** Belfry was not retired until Conservatory reached podcast parity; `belfry-core`'s worker migrated rather than being rewritten. Parity was reached at v0.0.52 (Phase 6c complete, the sleep timer the last piece), so Belfry is now retired: its GitHub repo is archived and the `~/.gitrepos` CLAUDE.md project map carries the note. The local clone is kept frozen as reference.
9. **libmpv per-item profile switching.** Swapping filter graphs between a music track and a podcast episode mid-queue needs prototyping; gapless within an album plus profile switching at album/kind boundaries is the tricky bit. Audiobooks add no new graph (they share the spoken-word profile, §6.3), but chapter advance *within* a book must be gapless across files or M4B spans, which is the same boundary problem one level down.
10. **Audiobook metadata provider (OPEN).** Ship an online provider (Audible / Audnexus / Google Books, the Audiobookshelf model) or assume locally-tagged/foldered files plus manual edit? Default is out (§7.5), revisited only if curation friction demands it. The audiobook analogue of §7.3.
11. **Audiobook chapterize (OPEN).** For books that arrive as one long file with no chapter markers, derive chapters by silence detection (the m4b-tool technique) on import, or leave them chapterless until the user runs an explicit step? Default is opt-in only.
12. **Audiobook integration (settled).** Audiobooks land as a third tab (§3.8) reusing the absorbed Belfry spoken-word engine, with a book as one unified-queue entry and chapters as intra-item navigation (§6.1), and metadata from local sources only in v1 (§7.5). This was a deliberate decision (the alternative was a separate audiobook engine/profile and chapter-as-queue-item granularity); recorded here, with the alternatives, in case the choice needs revisiting.
13. **Plugin restructure (settled, v0.0.2).** Music is the native program; podcasts and audiobooks are compile-time plugins: feature-gated workspace crates (§2.2), on by default, internal-only API, with all schema staying in core's single migration ledger. The unified queue remains a core commitment, which is precisely why the queue, the libmpv host, and every playback profile stay in core. Alternatives considered and rejected: dynamic `.so` loading (Rust has no stable ABI; Flatpak sandbox friction; per-plugin schema versioning breaks the append-only ledger) and out-of-process plugins over D-Bus (the gapless profile swap of §16.9 and the §13 latency budgets suffer across a process boundary). Feature-gated crates can later become dynamically loaded if a third-party ecosystem is ever wanted; the reverse migration is not cheap, so this is the conservative end to start from.
14. **Default library root + the `Music/` stutter (OPEN, decide at Phase 10).** No default root exists yet; the GUI takes it as the second CLI arg (a config file + a Preferences folder-picker is Phase 10). The intent is to keep all audio under `~/Music`, so the natural default is `~/Music/Conservatory/` (giving `~/Music/Conservatory/{Music,Podcasts,Audiobooks}/...`). The wart: the music tree then reads `~/Music/Conservatory/Music/`, a triple-`Music` stutter Brandon dislikes. Candidate resolutions, to settle before 1.0: (a) accept the stutter for the sake of "everything under `~/Music`"; (b) default the root to `~/Conservatory/` (or `~/Media/`), symmetric `{Music,Podcasts,Audiobooks}/` and no stutter, but not literally under `~/Music`; (c) keep the root at `~/Music` itself and drop the `Music/` prefix only for the music tree, so music genres sit at the root while `Podcasts/` and `Audiobooks/` get subfolders, stutter-free but asymmetric. No change for now; the §5.1 `Music/` layout stands until this is decided.

---

## 17. Phasing

Hard phasing is the active discipline: each stage below must be usable on its own, so work can move between Conservatory and Atrium without stranding either. This replaced an earlier plan to defer the build entirely until Atrium reached a real shipping milestone (the reasoning being that two concurrent flagship-scale projects is the failure mode to avoid). That deferral was lifted by deliberate decision and the build has begun alongside Atrium; the original rationale is kept here, not deleted, because it is the thing to re-read if the concurrency turns out to be a mistake.

The phases below are the contract-level shape. `roadmap.md` breaks each into independently shippable sub-phases (1a/1b, 2a–2d, and so on), each with its own checklist, tests, and a usable-artifact exit; consult it for the working plan.

- **Phase 0 (done).** This spec; design. Workspace skeleton bootstrapped at v0.0.1: the four crates, portfolio docs, build files, CI scaffold. No feature code yet.
- **Phase 1.** `conservatory-core` foundation: SQLite worker + read pool + migrations + fixtures (port from `belfry-core`), tag read, the data model.
- **Phase 2.** Import + organize: path-template engine, shelf-genre resolver, file mover with dry-run + undo. The manager is usable headless via the CLI here.
- **Phase 3.** GTK browse: the Columns UI faceted view + search grammar + track list. A working library browser.
- **Phase 4.** Playback: libmpv engine, music profile, unified queue, Now-bar, MPRIS. A daily-driver music player.
- **Phase 5.** Bulk editing + embedded-tag write-back.
- **Phase 5.5.** Audio engine: the labelled `af`-chain builder, a graphic + parametric equalizer, the DSP modules (compressor / limiter / leveler), correct head-staged ReplayGain, and output backend / resampler control. Resolves §16.6. Lands before the **Phase 6c spoken-word chain** (which is built as presets on it, §6.3), not before all of Phase 6: the podcast manager and triage (Phases 6a/6b) are independent of the audio engine and shipped first. The music daily-driver feels complete here.
- **Phase 6.** Podcasts: absorb the Belfry subsystem as the `conservatory-podcasts` plugin crate (§2.2) behind the Podcasts tab, hook episodes into the unified queue. The podcast schema lands in core's ledger; the behaviour lands in the plugin. **Done at v0.0.52: podcast parity reached, Belfry retired.**
- **Phase 7.** Audiobooks: the `conservatory-audiobooks` plugin crate (§2.2), a third tab over the absorbed spoken-word engine. The book/chapter/series data model and local-source import (headless), then the Audiobooks browse tab, then playback (chapters + first-class resume) reusing the shared Phase 5.5 chain engine via the Phase 6c spoken-word presets. Placed after Phase 6 because it reuses that engine; the audiobook *manager* could in principle land earlier, but the deliberate choice is to keep it whole and post-podcast (§16.12).

The manager half (Phases 1–3) must be usable before the player half is finished, and the player must be usable before podcasts arrive. Audiobooks (Phase 7) come last because they lean on the podcast engine; each is a hard phase that leaves a usable artifact. No phase leaves the app non-functional.

---

## 18. Project Conventions

Standard portfolio layout:

- `README.md`, `spec.md` (this file), `roadmap.md`, `patchnotes.md`, `CLAUDE.md`, `ATTRIBUTIONS.md` (design lineage, dependency licenses, the GPL chain analysis).
- `VERSION` is the single source of truth; `Cargo.toml` (workspace and each member) matches.
- `LICENSE` (GPL-3.0-or-later), `logo.svg`.
- `data/` — `.ui` XML, icons, GSettings schema, AppStream metainfo, Flatpak manifest, bundled fonts (registered via fontconfig at first run; never assume host fonts).
- `conservatory-core/`, `conservatory-search/`, `conservatory-podcasts/`, `conservatory-audiobooks/`, `conservatory-cli/`, `conservatory/` — workspace members (`conservatory-podcasts` and `conservatory-audiobooks` are the compile-time plugin crates, §2.2).
- `tests/` — integration tests alongside in-crate unit tests; the file-mover dry-run/undo and the re-import contract (§5.6) get dedicated fixture-backed suites.
- `docs/` — schema, keymap, path-template reference, genre normalization notes, libmpv profile reference, search grammar. Audiobook design is folded into these (schema, path-template, libmpv-profiles, search-grammar) rather than a separate doc.

CI matches the portfolio: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` on Linux. Tests required from day one.
