#!/usr/bin/env bash
# Conservatory browse demo.
#
# Imports the local testdata albums into a throwaway library under $TMPDIR and
# launches the GTK browse window. Nothing is written to the repo or your real
# music library (testdata is copied, not moved); the throwaway library is removed
# when you close the window. Run it from anywhere:
#
#   scripts/demo.sh
#
# Set CONSERVATORY_DEMO_NO_GUI=1 to import-and-exit without launching the window
# (used for a headless smoke check).

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
  echo "demo: CONSERVATORY_DEMO_NO_GUI set; facets preview, skipping the window:"
  "$CLI" debug-facets "$DB"
  exit 0
fi

echo "demo: launching the browse window (close it to clean up) ..."
"$GUI" "$DB"
