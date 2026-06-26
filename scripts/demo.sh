#!/usr/bin/env bash
# Conservatory demo.
#
# Imports the local testdata albums into a throwaway library under $TMPDIR,
# subscribes to one real podcast feed, and launches the GTK app: the Music
# browse (faceted panes, sortable track list, Ctrl+F filter bar), the Podcasts
# triage tab (Phase 6b), and the Now Playing drawer (Ctrl+I) with its chapter
# list + Smart Speed indicator (Phase 6c-iii). Nothing is written to the repo or your real
# music library (testdata is copied, not moved); the throwaway library is
# removed when you close the window. Run it from anywhere:
#
#   scripts/demo.sh
#
# Set CONSERVATORY_DEMO_NO_GUI=1 to import-and-exit without launching the window:
# a headless smoke check that previews the facets, the filter-bar grammar, and
# the podcast triage. Set CONSERVATORY_DEMO_FEED=<url> to subscribe to a
# different feed (default: Cortex). The feed fetch is best-effort: offline just
# leaves the Podcasts tab empty and the music demo is unaffected.

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
  exit 0
fi

echo "demo: launching the app (close the window to clean up) ..."
echo "  views:  the header switcher (or Alt+1 / Alt+2) flips between Music and"
echo "          Podcasts. Shrink the window and the switcher drops to a bottom"
echo "          bar above the Now-bar."
echo "  browse: click a facet row to narrow; click a column header to sort;"
echo "          press Ctrl+F and type a filter, e.g. genre:ambient or rating:>=1;"
echo "          save it as a Perspective in the left sidebar, then reload it."
echo "  play:   double-click a track to play the visible list from there; the"
echo "          Now-bar at the bottom drives transport; Ctrl+U opens the queue"
echo "          (drag to reorder); the header speaker icon picks the output device."
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
echo "  details:  click the Now-bar cover/title (or Ctrl+I) to slide up the Now"
echo "          Playing drawer with the current item's full metadata. For a"
echo "          chaptered episode it also lists the chapters: click one to jump,"
echo "          and the chapter you are in highlights and follows the playhead."
echo "          Ctrl+Shift+-> / Ctrl+Shift+<- skip chapter to chapter (the Now-bar"
echo "          also grows chapter buttons). Turn on Smart Speed in a show's gear"
echo "          and the drawer shows a live 'saved' time as you listen. Chapters"
echo "          appear for shows that publish them (Podcasting 2.0 podcast:chapters);"
echo "          point CONSERVATORY_DEMO_FEED at such a feed to see them."
# The library root (second arg) is what lets the player resolve the managed
# relative track paths; without it the GUI browses but can't play (Phase 4).
"$GUI" "$DB" "$LIB"
