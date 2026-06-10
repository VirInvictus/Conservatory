# Keymap (draft proposal)

> **Status: provisional.** No GUI exists yet (the GTK binary arrives at roadmap Phase 3). This is a proposed keymap to be finalized as the browse (Phase 3) and playback (Phase 4) surfaces land, not a description of current behaviour. It encodes spec §3.1's principle: **every action is keyboard-accessible, no hidden gestures, every swipe has a menu equivalent.** GNOME/libadwaita conventions are followed where one exists.

## Global

| Key | Action |
|---|---|
| `Ctrl+F` | Focus the filter bar (the full search grammar; no separate search mode, spec §3.4) |
| `Ctrl+L` | Clear the filter bar |
| `Ctrl+,` | Preferences (`AdwPreferencesDialog`) |
| `Ctrl+Q` | Quit |
| `F1` | Keyboard shortcuts window |
| `Ctrl+1` / `Ctrl+2` | Switch view: Music / Podcasts |
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
| `→` / `←` | Seek forward / back (small step) |
| `Shift+→` / `Shift+←` | Seek forward / back (large step) |
| `Ctrl+↑` / `Ctrl+↓` | Volume up / down |
| `Ctrl+M` | Mute |
| Media keys | Play/pause/next/previous via MPRIS2 (spec §6.5) |

## Queue

| Key | Action |
|---|---|
| `Ctrl+U` | Show / focus the queue |
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
| `S` | Sleep timer (episodes) |

Conflicts to resolve at implementation: `Ctrl+1`/`Ctrl+2` are listed for both the top-level view switch and the podcast triage lists; the triage bindings only apply when the Podcasts view holds focus, but this overlap should be reviewed before it ships.
