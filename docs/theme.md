# Visual theme (Kanagawa Dragon)

Reference for Conservatory's fixed visual identity, landed at Phase 12a and
made fully self-owned at Phase 26 (de-adwaita). The app ships the **Kanagawa
Dragon** palette (Brandon's house dark theme) baked into its own generated
stylesheet; there is no adwaita sheet underneath. The look is the spec §2.4
design language: flat, square, hard 1px borders, denser spacing than the GNOME
HIG, a slim titlebar with no window buttons. The per-album accent (extracted
per `docs/accent.md`) tints highlights only; it does not recolour the window
chrome (the Amberol "chameleon" model was considered and declined, to keep one
consistent identity across every screen).

The sheet lives in `conservatory/src/theme.rs`: palette consts spliced into a
structural template by `sheet()`, installed display-wide by `install()` at
`STYLE_PROVIDER_PRIORITY_USER + 1`. That priority is load-bearing (the
Colophon discovery): a themed `~/.config/gtk-4.0/gtk.css` loads at USER (800)
and outranks APPLICATION (600), so an app sheet below USER gets silently
half-overridden on themed systems. The runtime per-album accent ring lives in
`conservatory/src/ui/accent.rs` and registers at USER + 2, so it keeps
outranking the base sheet.

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

## Owned palette roles

The `theme.rs` consts (must stay in step with this table; each is baked into
the generated sheet by token replacement):

| Const | Hex | Dragon token | Role |
|---|---|---|---|
| `BG_WINDOW` | `#181616` | dragonBlack3 | window ground |
| `BG_VIEW` | `#12120f` | dragonBlack1 | lists / sidebar / entries ground |
| `BG_HEADER` | `#1d1c19` | dragonBlack2 | titlebar, tooltips |
| `BG_CARD` | `#1d1c19` | dragonBlack2 | cards, popovers, buttons, toasts |
| `FG` | `#c5c9c5` | dragonWhite | primary foreground |
| `FG_DIM` | `#a6a69c` | dragonGray | `.dim-label`, subtitles |
| `GRID` | `#393836` | dragonBlack5 | hairlines, 1px borders |
| `ACCENT` | `#c4746e` | dragonRed | accent, selection, `.suggested-action` |
| `ON_ACCENT` | `#12120f` | dragonBlack1 | dark text on the red fill |
| `WARN` | `#c4b28a` | dragonYellow | warnings (`.filter-warn`) |
| `ERR` | `#c4746e` | dragonRed | `.destructive-action` |
| `OK` | `#87a987` | dragonGreen | `.success` |

Dark polarity is forced with `gtk-application-prefer-dark-theme` at startup so
the stock-widget internals the sheet does not name follow dark too. Dragon is
dark-only; a light (Lotus) variant is out of scope. Custom properties
(`--c-*`, the Colophon mechanism) are deliberately not used: one fixed palette
gains nothing from them, and skipping them keeps the gtk4 crate on `v4_14`.

## What stays deliberately un-flat

The lifted cover cards keep their radius and Amberol-style drop shadow (below):
chrome is flat, content imagery stays lifted (the Hermitage
cover-as-visual-unit pattern), and the runtime accent ring layers onto that
same shadow. Everything else squared off at Phase 26: selection rectangles,
tiles, the toast, popovers, buttons, switches, sliders.

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

Three bundled OFL fonts, one per role, applied through exactly three
`font-family` rules in the generated sheet (`conservatory/src/theme.rs`,
test-enforced). Every rule carries a generic fallback so a missing font
degrades to a sane default rather than breaking text.

| Role | Font | CSS selector | Fallback |
|---|---|---|---|
| Base UI (chrome, menus, track list, facet panes, property values) | Inter | `window, popover, dropdown, tooltip` | `sans-serif` |
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
