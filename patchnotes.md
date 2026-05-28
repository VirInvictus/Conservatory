# Patch Notes

## v0.0.1

First commit. Project bootstrapped out of the design spec.

- Cargo workspace with the four planned crates (`conservatory-core`, `conservatory-search`, `conservatory-cli`, `conservatory`), all building as dependency-light skeletons.
- Portfolio scaffolding: `README.md`, `roadmap.md`, this file, `CLAUDE.md`, `ATTRIBUTIONS.md`, `VERSION`, GPL-3.0-or-later `LICENSE`, `.gitignore`, and a Meson packaging stub.
- Build deferral lifted. The spec previously parked the build behind an Atrium shipping milestone; that decision was reversed and the build now proceeds concurrently with Atrium, with hard phasing as the mitigation (spec §16.1, §17).
- Belfry retirement remains gated on podcast parity (Phase 6); nothing in Belfry has been removed.
