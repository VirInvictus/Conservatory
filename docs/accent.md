# Cover-art accent extraction

Reference for the per-album (and later per-book) accent colour stored in
`albums.accent_rgb` / `books.accent_rgb` (spec §7.4). Implemented in
`conservatory-core/src/accent.rs`, landed at Phase 1c.

## Algorithm

A faithful port of Hermitage's `colors.py`. Given encoded cover bytes:

1. Decode to RGB and resize to **64x64** with a Lanczos3 filter (`SAMPLE_SIZE`).
   The downscale is both a speed win and a smoothing step; analysis runs over
   4096 pixels regardless of source resolution.
2. **Median-cut** quantize to a palette of **5 colours** (`NUM_COLORS`):
   repeatedly take the box with the widest single-channel spread, stable-sort
   its pixels along that channel, and split at the median. Stop at 5 boxes or
   when no box is splittable (a box of identical pixels, e.g. a solid-colour
   cover, yields a single colour). Each box's representative is its per-channel
   mean.
3. Rank the palette by **vibrancy** = `saturation * value` in HSV and pick the
   most vibrant. On a tie the earlier colour wins. This is Hermitage's deliberate
   choice: the accent is the most vibrant colour present, *not* the most populous,
   so a muted cover with one saturated detail still gets a lively accent.
4. Pack the winner as `0x00RRGGBB` into a `u32`.

## Determinism

The pipeline is fully deterministic: a fixed resize filter, a median split with
an index tie-break in the stable sort, and a first-wins vibrancy tie-break. The
same bytes always yield the same accent (test: `extraction_is_deterministic`).

## Fidelity caveat

This is the same algorithm *family* as Pillow's `quantize(MEDIANCUT)` that
Hermitage calls, not byte-identical: Pillow quantizes through libimagequant
internally, and our mean-of-box representative differs from its palette entries
in the low bits. Fidelity is to the approach (median-cut + vibrancy ranking), per
spec §7.4; the tests use unambiguous covers (solid colours, a vibrant region over
grey) where any correct median-cut agrees.
