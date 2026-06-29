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

## Why no blurred cover background

GTK4 CSS has no `backdrop-filter`, and `filter: blur()` applies to a widget's own
content, not to what is behind it. An Amberol-style blurred-cover backdrop
therefore needs a `GskBlurNode` render-node widget, which is deferred (it was
explicitly left out of the Phase 12 scope).
