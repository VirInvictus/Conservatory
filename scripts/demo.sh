#!/usr/bin/env bash
# Conservatory browse demo.
#
# Imports the local testdata albums into a throwaway library under $TMPDIR and
# launches the GTK browse window: faceted panes, a sortable track list, and the
# Ctrl+F filter bar (Phase 3c). Nothing is written to the repo or your real music
# library (testdata is copied, not moved); the throwaway library is removed when
# you close the window. Run it from anywhere:
#
#   scripts/demo.sh
#
# Set CONSERVATORY_DEMO_NO_GUI=1 to import-and-exit without launching the window:
# a headless smoke check that previews the facets and the filter-bar grammar.

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
  exit 0
fi

echo "demo: launching the browse window (close it to clean up) ..."
echo "  browse: click a facet row to narrow; click a column header to sort;"
echo "          press Ctrl+F and type a filter, e.g. genre:ambient or rating:>=1;"
echo "          save it as a Perspective in the left sidebar, then reload it."
echo "  play:   double-click a track to play the visible list from there; the"
echo "          Now-bar at the bottom drives transport; Ctrl+U opens the queue"
echo "          (drag to reorder); the header speaker icon picks the output device."
echo "  edit:   select track(s) and press Ctrl+E (or the header pencil) to bulk-edit"
echo "          fields; a year/album/genre change previews the file move before applying."
# The library root (second arg) is what lets the player resolve the managed
# relative track paths; without it the GUI browses but can't play (Phase 4).
"$GUI" "$DB" "$LIB"
