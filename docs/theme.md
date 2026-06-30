# Visual theme (Kanagawa Dragon)

Reference for Conservatory's fixed visual identity, landed at Phase 12a. The app
ships the **Kanagawa Dragon** palette (Brandon's house dark theme) mapped onto
libadwaita's named colours, and forces the dark colour scheme. The per-album
accent (extracted per `docs/accent.md`) tints highlights only; it does not
recolour the window chrome (the Amberol "chameleon" model was considered and
declined, to keep one consistent identity across every screen).

The CSS lives in `conservatory/src/main.rs` (the `CSS` const, injected at
`startup` by `load_css`); the runtime per-album accent ring lives in
`conservatory/src/ui/accent.rs`.

## Palette (Dragon variant)

The canonical hexes (from `kanagawa.nvim`, Dragon variant; the source of truth is
also `calibre-web-kanagawa/theme/kanagawa-dragon.css`):

| Token | Hex | Role |
|---|---|---|
| dragonBlack0 | `#0d0c0c` | deepest ground |
| dragonBlack1 | `#12120f` | view / sidebar ground |
| dragonBlack2 | `#1d1c19` | headerbar / card / popover |
| dragonBlack3 | `#181616` | window ground |
| dragonBlack4 | `#282727` | raised fill |
| dragonBlack5 | `#393836` | borders / lines |
| dragonWhite | `#c5c9c5` | primary foreground |
| dragonRed | `#c4746e` | **accent** (the warm signature) |
| dragonYellow | `#c4b28a` | warning |
| dragonGreen | `#87a987` | success |
| dragonBlue2 | `#8ba4b0` | secondary (info) |

## libadwaita named-colour mapping

`@define-color` overrides in `main.rs` (must stay in step with this table):

| libadwaita name | Hex | Dragon token |
|---|---|---|
| `window_bg_color` | `#181616` | dragonBlack3 |
| `window_fg_color` | `#c5c9c5` | dragonWhite |
| `view_bg_color` | `#12120f` | dragonBlack1 |
| `view_fg_color` | `#c5c9c5` | dragonWhite |
| `headerbar_bg_color` | `#1d1c19` | dragonBlack2 |
| `headerbar_fg_color` | `#c5c9c5` | dragonWhite |
| `sidebar_bg_color` | `#12120f` | dragonBlack1 |
| `sidebar_fg_color` | `#c5c9c5` | dragonWhite |
| `secondary_sidebar_bg_color` | `#181616` | dragonBlack3 |
| `card_bg_color` | `#1d1c19` | dragonBlack2 |
| `card_fg_color` | `#c5c9c5` | dragonWhite |
| `popover_bg_color` | `#1d1c19` | dragonBlack2 |
| `popover_fg_color` | `#c5c9c5` | dragonWhite |
| `dialog_bg_color` | `#1d1c19` | dragonBlack2 |
| `accent_color` | `#c4746e` | dragonRed |
| `accent_bg_color` | `#c4746e` | dragonRed |
| `accent_fg_color` | `#12120f` | dragonBlack1 (dark text on the red fill) |
| `warning_color` | `#c4b28a` | dragonYellow |
| `error_color` | `#c4746e` | dragonRed |
| `success_color` | `#87a987` | dragonGreen |

Forcing the dark scheme (`adw::ColorScheme::ForceDark`) is required: Dragon is a
dark palette, and without forcing it the overrides would land on whichever
variant the system prefers. A light (Lotus) variant is out of scope.

## Cover cards and the accent ring

Covers are lifted "cards": a 10px radius and an Amberol-style drop shadow
(`box-shadow: 0 2px 10px rgba(0,0,0,0.35)`), via the `.cover-art` class (and the
per-surface `.inspector-cover` / `.now-playing-cover` / `.book-cover` /
`.now-bar-cover` variants).

The **per-album accent ring** is a 2px ring layered over that drop shadow,
applied at runtime by `ui/accent.rs`: a single display-wide `CssProvider` (the
non-deprecated route to dynamic per-item colour, GTK4 having deprecated per-widget
`StyleContext` providers) injects `.cover-acc-RRGGBB { box-shadow: 0 0 0 2px
#RRGGBB, <drop>; }` and the frame carries that class. The drop shadow is re-stated
inside the accent rule because a later `box-shadow` rule replaces the property
wholesale rather than stacking, so the ring and the lift have to share one
declaration.

## Typography (Phase 13d)

Three bundled OFL fonts, one per role, applied through `font-family` rules in the
`CSS` const (`conservatory/src/main.rs`). Every rule carries a generic fallback so
a missing font degrades to a sane default rather than breaking text.

| Role | Font | CSS selector | Fallback |
|---|---|---|---|
| Base UI (chrome, menus, track list, facet panes, property values) | Inter | `window, popover, dropdown, tooltip` | `"Adwaita Sans", sans-serif` |
| Headers and titles | Fraunces | `.title-1`..`.title-4`, `.large-title`, `.heading` | `serif` |
| Technical text (paths, the status-bar tech line, MusicBrainz ids) | IBM Plex Mono | `.tech` | `monospace` |

Inter is the base because it is screen-optimized, has tabular figures by default
(so the numeric / duration / year columns align without extra OpenType setup), and
is the typeface GNOME's own Adwaita Sans derives from, so it reads native. Fraunces
is a warm display serif for headline character; IBM Plex Mono marks the genuinely
technical surfaces (the `.tech` class, applied to the path / id property rows via
`ui/fields::is_tech_field` and to the status-bar technical line).

### Bundling and registration

The fonts must never be assumed installed on the host (spec §7.2.9). The files live
in `data/fonts/` with a per-family `OFL.txt`. Pango's `add_font_file` would be the
clean loader but needs pango v1_56 and the workspace is on 0.20, so registration
goes through fontconfig the way the spec names: `register_bundled_fonts()` (in
`main.rs`, called first thing in `main()` before GTK lays out any text) writes a
small fontconfig file that `<include>`s the system config and adds the bundled
directory, then points fontconfig at it via `FONTCONFIG_FILE`. The Flatpak instead
installs the files into the data fonts dir (`meson.build`), where fontconfig finds
them automatically. A `CONSERVATORY_FONT_DIR` env var overrides the directory.

## Why no blurred cover background

GTK4 CSS has no `backdrop-filter`, and `filter: blur()` applies to a widget's own
content, not to what is behind it. An Amberol-style blurred-cover backdrop
therefore needs a `GskBlurNode` render-node widget, which is deferred (it was
explicitly left out of the Phase 12 scope).
