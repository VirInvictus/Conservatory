# CLAUDE.md (Conservatory)

Per-project guidance. Overrides `~/.claude/CLAUDE.md` only where they conflict.

## What Conservatory is

"Calibre for audio": a native GNOME library manager for music and podcasts. The database owns the library and the app moves files to match it (the inversion of Lattice/Belfry's filesystem-canonical stance). Browse is the deadbeef-cui Columns UI as a first-class window; playback is a libmpv daily-driver engine; one unified queue interleaves music tracks and podcast episodes.

Read `spec.md` before changing semantics. Read `roadmap.md` before scoping work. Read `ATTRIBUTIONS.md` before adding deps.

Reference apps: **Calibre** (library-as-database, file ownership, save-to-disk template), **foobar2000 / Columns UI** via **deadbeef-cui** (faceted browse), **beets** (genre canonicalization), **Overcast / Castro** via **Belfry** (the absorbed podcast engine and triage model), **Atrium / Viaduct** (single-writer worker + search grammar shape), **Hermitage** (cover-as-visual-unit, accent extraction).

## Hard rules specific to Conservatory

- **Moving the user's files is the headline risk.** A move bug damages a real library. The dry-run preview, undo journal, and crash-safe replay (spec §5.4) are release-blocking, not nice-to-have. Never relocate files without them.
- **The database owns organization; embedded tags keep files portable.** Write curated metadata back into the files (spec §5.5) so the library is never a roach motel.
- **Raw tags and shelf genre are decoupled.** `track_genres` are multi-valued and untouched, for facets and search. `shelf_genre` is single-valued and is the *only* input to the genre folder level (spec §5.2). Never let raw tags reach the filesystem.
- **GPL-3-or-later is non-negotiable.** Driven by librubberband (Smart Speed), the same chain as Belfry. No proposing license relaxation without proposing a rubberband replacement.
- **Single-writer SQLite worker.** A dedicated tokio task owns the writable connection; the GTK thread holds an `mpsc::Sender<Command>`. No `RwLock<Connection>`, no second writer. Read commands open the DB read-only at the process level.
- **Every non-GUI surface stays CLI-testable.** The four-crate split exists for this; logic lives in `conservatory-core`, not the GTK binary.
- **Phase hard.** Each phase (spec §17) must leave a usable artifact. The manager (Phases 1 to 3) is usable before the player; the player before podcasts. No phase leaves the app non-functional. This is what keeps the concurrency with Atrium recoverable.

## Belfry absorption status

Conservatory absorbs Belfry. `belfry-core`'s single-writer worker migrates at Phase 1; the full podcast subsystem (fetch, Smart Speed, Voice Boost, triage, OPML) is absorbed at Phase 6. **Belfry is not retired, and the Belfry repo is not deleted, until Conservatory reaches podcast parity** (spec §16.8). The build deferral behind Atrium was lifted at v0.0.1; the retirement-at-parity rule was kept.

## Workspace

Four crates: `conservatory-core` (headless engine), `conservatory-search` (search grammar; ports the `atrium-search` shape but does not depend on it, see ATTRIBUTIONS.md), `conservatory-cli`, `conservatory` (GTK4 binary). Cargo for development; Meson is the Flatpak packaging wrapper, wired in at a later phase.

`VERSION` is the single source of truth; the workspace `Cargo.toml` matches it. Bumping a version means updating both.
