#!/usr/bin/env bash
# Conservatory demo.
#
# Builds a throwaway library under $TMPDIR populated across all three tabs, then
# launches the GTK app:
#
#   Music       the testdata albums, imported into the managed tree (faceted
#               browse panes, sortable track list, Ctrl+F filter bar, the status
#               bar footer, the Ctrl+P properties inspector).
#   Podcasts    one real subscribed feed and its episodes (Phase 6 triage tab).
#   Audiobooks  the testdata audiobooks, imported into the managed tree (Phase 7
#               shelf, resume, per-book Smart Speed / Voice Boost).
#
# The unified queue interleaves all three, and the Now Playing drawer (Ctrl+I)
# adapts its surface to the playing item: a full-bleed accent cover and scrubber
# for everything; per-track / per-episode / per-book metadata; the EQ/DSP/gapless
# line for music; the chapter list, Smart Speed indicator, and sleep timer for
# spoken word.
#
# Nothing is written to the repo or your real libraries (testdata is copied, not
# moved); the throwaway library is removed when you close the window. Run it from
# anywhere:
#
#   scripts/demo.sh
#
# Set CONSERVATORY_DEMO_NO_GUI=1 to import-and-exit without launching the window:
# a headless smoke check that previews the facets, the filter-bar grammar, the
# podcast triage, and the audiobook shelf. Set CONSERVATORY_DEMO_FEED=<url> to
# subscribe to a different feed (default: Cortex). The feed fetch and the
# audiobook import are best-effort: a missing feed or absent testdata just leaves
# that tab empty and the rest of the demo is unaffected.

set -euo pipefail

# Resolve the repo root from this script's location, so it runs from any cwd.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ALBUMS="$ROOT/testdata/albums"
if [ ! -d "$ALBUMS" ]; then
  echo "demo: $ALBUMS not found." >&2
  echo "  It is gitignored local test data. Copy a couple of album folders into" >&2
  echo "  testdata/albums/ (one folder per album) and run again." >&2
  exit 1
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/conservatory-demo.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
DB="$WORK/library.db"
LIB="$WORK/library"
mkdir -p "$LIB"

echo "demo: building conservatory + conservatory-cli ..."
cargo build -p conservatory -p conservatory-cli

CLI="$ROOT/target/debug/conservatory-cli"
GUI="$ROOT/target/debug/conservatory"

echo "demo: importing albums into a throwaway library at $WORK ..."
found=0
for album in "$ALBUMS"/*/; do
  [ -d "$album" ] || continue
  "$CLI" import "$DB" "$album" "$LIB" --format human
  found=1
done
if [ "$found" -eq 0 ]; then
  echo "demo: no album folders under $ALBUMS" >&2
  exit 1
fi

# Subscribe to one real podcast feed so the Podcasts tab has something to browse
# (Phase 6b). `podcast add` fetches the feed and upserts its episodes, so this
# needs the network; it is best-effort, so offline (or a 429) just leaves the
# Podcasts tab empty without failing the music demo. Override with
# CONSERVATORY_DEMO_FEED. The feed lands in the unified queue alongside music.
FEED="${CONSERVATORY_DEMO_FEED:-https://www.relay.fm/cortex/feed}"
echo "demo: subscribing to a podcast feed ($FEED) ..."
if ! "$CLI" podcast add "$DB" "$FEED" --format human; then
  echo "demo: podcast add failed (offline, or the feed throttled us); the" >&2
  echo "      Podcasts tab will be empty. The music demo is unaffected." >&2
fi

# Import the testdata audiobooks into the managed Audiobooks/ tree (Phase 7).
# Like the music import this copies (the source files are left in place); each
# book is one `audiobook import` call (a multi-part M4B folder or a folder of
# chapter files counts as one book). The testdata is gitignored and local, so
# this is best-effort: an absent or empty testdata/audiobooks just leaves the
# Audiobooks tab empty without failing the rest of the demo. We discover one
# book per directory that directly holds audio files, so the Author/Book and
# Author/Series/Book layouts both resolve correctly.
ABOOKS="$ROOT/testdata/audiobooks"
if [ -d "$ABOOKS" ]; then
  echo "demo: importing audiobooks into the throwaway library ..."
  book_dirs="$(find "$ABOOKS" -type f \
    \( -iname '*.m4b' -o -iname '*.m4a' -o -iname '*.mp3' -o -iname '*.ogg' \) \
    -printf '%h\n' 2>/dev/null | sort -u)"
  if [ -n "$book_dirs" ]; then
    while IFS= read -r book; do
      [ -n "$book" ] || continue
      if ! "$CLI" audiobook import "$DB" "$book" "$LIB" --format human; then
        echo "demo: audiobook import failed for $book (continuing)." >&2
      fi
    done <<< "$book_dirs"
  else
    echo "demo: testdata/audiobooks holds no audio files; Audiobooks tab empty." >&2
  fi
else
  echo "demo: no testdata/audiobooks (gitignored local data); Audiobooks tab" >&2
  echo "      will be empty. Drop a book folder or .m4b there to demo Phase 7." >&2
fi

if [ "${CONSERVATORY_DEMO_NO_GUI:-0}" = "1" ]; then
  echo "demo: CONSERVATORY_DEMO_NO_GUI set; previewing headless, skipping the window."
  echo
  echo "=== facets (the browse panes) ==="
  "$CLI" debug-facets "$DB"
  echo
  echo "=== filter-bar grammar (spec §3.4; the engine behind the GUI filter bar) ==="
  for q in 'genre:ambient' 'format:mp3' 'duration:>240 sort:-duration'; do
    echo "--- search '$q' ---"
    "$CLI" search "$DB" "$q" --format human
    echo
  done
  echo "=== metadata editing (Phase 5a; the engine behind the GUI Ctrl+E dialog) ==="
  echo "--- tag set: bump a rating across a selection (no file move) ---"
  "$CLI" tag set "$DB" 'format:mp3' rating=5
  "$CLI" search "$DB" 'rating:5' --format human
  echo "--- tag set: a path-affecting year edit previews the move (dry-run) ---"
  "$CLI" tag set "$DB" 'format:opus' year=1992 --root "$LIB"
  echo
  echo "=== podcasts (Phase 6b; the engine behind the Podcasts tab) ==="
  echo "--- inbox episodes across subscriptions, first 12 (newest first) ---"
  # Capture the full output first, then trim: piping the CLI straight into
  # `head` closes its stdout early (SIGPIPE), which trips `set -o pipefail`.
  inbox="$("$CLI" podcast episodes "$DB" --bucket inbox --format human)"
  printf '%s\n' "$inbox" | head -n 12
  echo
  echo "=== audiobooks (Phase 7; the engine behind the Audiobooks tab) ==="
  echo "--- the shelf: every book with author / narrator / series + state ---"
  "$CLI" audiobook list "$DB" --format human
  echo "--- filter the shelf by author (the §3.4 grammar over book fields) ---"
  "$CLI" audiobook list "$DB" 'author:gaiman' --format human
  exit 0
fi

echo "demo: launching the app (close the window to clean up) ..."
echo "  views:  the header switcher (or Alt+1 / Alt+2 / Alt+3) flips between"
echo "          Music, Podcasts, and Audiobooks. Shrink the window and the"
echo "          switcher drops to a bottom bar above the Now-bar."
echo "  browse: click a facet row to narrow; click a column header to sort;"
echo "          press Ctrl+F and type a filter, e.g. genre:ambient or rating:>=1;"
echo "          save it as a Perspective in the left sidebar, then reload it. The"
echo "          footer status bar shows the playing track's format/rate/channels"
echo "          on the left and 'N tracks - playtime' (or the selection) on the"
echo "          right; Ctrl+P opens the properties inspector for the selected row."
echo "  play:   double-click a track to play the visible list from there; the"
echo "          leftmost column marks the playing row with a play/pause glyph. The"
echo "          Now-bar drives transport; Ctrl+U opens the queue (drag to reorder);"
echo "          the header speaker icon picks the output device. The header menu"
echo "          (or Ctrl+M) toggles 'stop after current'; Ctrl+J jumps the list to"
echo "          the playing track."
echo "  edit:   select track(s) and press Ctrl+E (or the header pencil) to bulk-edit"
echo "          fields; a year/album/genre change previews the file move before applying."
echo "  sound:  the header speaker-card button (or Ctrl+,) opens the Sound dialog:"
echo "          a 10-band equalizer. While a track plays, drag a band and it changes"
echo "          live; pick a preset, or Save as… your own. The EQ persists and"
echo "          applies from the next launch."
echo "  podcasts: Alt+2 opens the Podcasts tab: a sidebar of triage buckets"
echo "          (Inbox / Queue / Played), shows, and tags; an episode list; and a"
echo "          detail pane. Double-click an episode to play it in the SAME queue"
echo "          as music (streamed if not downloaded); the detail buttons mark it"
echo "          played/archived or star it. With a show selected, the gear opens"
echo "          its per-show settings (speed, Smart Speed, inbox policy). The show"
echo "          notes are cleaned to plain text on import."
echo "  books:    Alt+3 opens the Audiobooks tab: a cover-grid shelf of every"
echo "          imported book with its derived state (New / In progress / Finished),"
echo "          in-progress first. Play one and it joins the SAME unified queue;"
echo "          a book is one queue entry that plays its parts in order and resumes"
echo "          where you left off. Smart Speed and Voice Boost are spoken-word"
echo "          features, so they apply to books too (per-book, like a podcast's"
echo "          per-show settings); they are off for music by design. Ctrl+E"
echo "          bulk-edits a selection (a path-affecting change re-shelves the files)."
echo "  details:  click the Now-bar cover/title (or Ctrl+I) to slide up the Now"
echo "          Playing drawer. Its surface adapts to the playing item: a full-bleed"
echo "          accent cover and a draggable scrubber for everything, an 'Up next'"
echo "          peek at the queue tail, and the right metadata per kind. For music"
echo "          it adds the EQ/DSP/gapless line; for a podcast or audiobook it adds"
echo "          the chapter list (click one to jump; the current chapter follows the"
echo "          playhead), a live Smart Speed 'saved' time, and the sleep timer."
echo "          Ctrl+Shift+-> / Ctrl+Shift+<- skip chapter to chapter (the Now-bar"
echo "          also grows chapter buttons). Podcast chapters appear for feeds that"
echo "          publish them (Podcasting 2.0 podcast:chapters); point"
echo "          CONSERVATORY_DEMO_FEED at such a feed to see them. The S key opens"
echo "          the Now-bar sleep timer, available for any playing item."
# The library root (second arg) is what lets the player resolve the managed
# relative track paths; without it the GUI browses but can't play (Phase 4).
"$GUI" "$DB" "$LIB"
