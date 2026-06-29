# Keymap (draft proposal)

> **Status: provisional, partly wired.** Live so far: `Ctrl+F` (filter bar, 3c); **double-click / Enter** on a track plays the visible list, **`Ctrl+Enter`** appends the selection, and the **Now-bar transport buttons** work (4b-ii-a / c); the **queue drawer** (`Ctrl+U` to toggle) with `Alt+↑/↓` reorder, `Delete`, `Ctrl+Shift+C`, and drag-and-drop (4b-ii-b); the saved queue resumes paused on launch (4b-ii-c); **media keys / headset buttons** work via MPRIS2 (4c-i); **`Ctrl+E`** opens the bulk-edit dialog over the selection (5a-ii); **`Alt+1` / `Alt+2`** switch between the Music and Podcasts views (6b-i; the Podcasts triage browse + playback + per-show settings shipped through 6b-ii). The **Now Playing drawer** (`Ctrl+I`, or click the Now-bar cover/title) shows the current item's metadata (v0.0.38). **`Ctrl+Shift+→/←`** (and the Now-bar chapter buttons, shown only for a chaptered item) skip between an episode's chapters (6c-iii-b). **`S`** pops the Now-bar sleep-timer menu (6c-iii-d; available for any playing item, the boundary row labelled by kind). Not yet wired: the in-window keyboard *playback* bindings below (`Space`, `Ctrl+→/←`, etc.). This stays a proposed keymap, finalized as those surfaces land, not a full description of current behaviour. It encodes spec §3.1's principle: **every action is keyboard-accessible, no hidden gestures, every swipe has a menu equivalent.** GNOME/libadwaita conventions are followed where one exists.

## Global

| Key | Action |
|---|---|
| `Ctrl+F` | Focus the filter bar (the full search grammar; no separate search mode, spec §3.4) |
| `Ctrl+L` | Clear the filter bar |
| `Ctrl+,` | Preferences (`AdwPreferencesDialog`) |
| `Ctrl+Q` | Quit |
| `F1` | Keyboard shortcuts window |
| `Alt+1` / `Alt+2` / `Alt+3` | Switch top-level view: Music / Podcasts / Audiobooks (the `AdwTabView` `Alt+N` convention; a global shortcut switching the `AdwViewStack`, Phase 6b-i. `Alt+3` is inert until the Audiobooks tab, 7b) |
| `Ctrl+J` | Open the jobs / activity surface (imports, moves, fetches) |

## Browse (Music)

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Move focus between facet panes and the track list |
| `↑` / `↓` | Move within the focused pane or list |
| `Ctrl+Click` / `Shift+Click` | Multi-select (aggregate facets; range-select rows) |
| `Enter` | Play the selection (replace queue) |
| `Ctrl+Enter` | Append the selection to the queue |
| `Ctrl+S` | Save the current filter as a Perspective |
| `Ctrl+E` | Edit metadata for the selection (bulk editor, Phase 5) |
| `Delete` | Remove from library (with confirmation; never deletes files without the move/undo job) |

## Playback

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `Ctrl+→` / `Ctrl+←` | Next / previous item |
| `Ctrl+Shift+→` / `Ctrl+Shift+←` | Next / previous chapter (episodes and audiobooks; a no-op without chapters) |
| `→` / `←` | Seek forward / back (small step) |
| `Shift+→` / `Shift+←` | Seek forward / back (large step) |
| `Ctrl+↑` / `Ctrl+↓` | Volume up / down |
| `Ctrl+M` | Mute |
| Media keys | Play/pause/next/previous via MPRIS2 (spec §6.5) |

## Queue

| Key | Action |
|---|---|
| `Ctrl+U` | Show / focus the queue |
| `Ctrl+P` | Show / hide the track properties inspector (v0.0.72; also the header properties button) |
| `Ctrl+I` | Show / hide the Now Playing details drawer (v0.0.38; also opened by clicking the Now-bar cover/title) |
| `Ctrl+,` | Open the Sound preferences (the equalizer; v0.0.41; also the header speaker-card button) |
| `Alt+↑` / `Alt+↓` | Move the selected queue item up / down |
| `Delete` | Remove the selected item from the queue |
| `Ctrl+Shift+C` | Clear the queue |

## Podcasts (Phase 6)

| Key | Action |
|---|---|
| `Ctrl+1` / `Ctrl+2` / `Ctrl+3` | Triage lists: Inbox / Queue / Played (within the Podcasts view) |
| `I` | Move episode to Inbox |
| `Q` | Add episode to the (unified) Queue |
| `R` | Refresh the focused show |
| `Ctrl+Shift+O` | Import OPML |

## Now Playing

| Key | Action |
|---|---|
| `Ctrl+Return` | Expand the Now-bar to the full Now Playing surface |
| `Esc` | Collapse Now Playing back to the Now-bar |
| `S` | Sleep timer menu (any playing item; the "end of item" row reads "End of track" / "End of episode" / "End of book" by kind) |

The earlier `Ctrl+1`/`Ctrl+2` top-level-switch overlap with the podcast triage lists is resolved: top-level view switching moved to `Alt+1/2/3` (the `AdwTabView` convention), leaving `Ctrl+1/2/3` to the triage lists within the Podcasts view.
