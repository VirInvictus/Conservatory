# Keymap

> **Status: substantially wired.** The in-window playback and navigation keys are live as of Phase 13e (v0.0.86 to v0.0.88): **`Space`** plays / pauses (everywhere except while typing in the filter, the foobar2000 rule), **`Ctrl+→/←`** skip next / previous, **`Ctrl+↑/↓`** change volume, **`Ctrl+0`** mutes, **`Ctrl+L`** clears the filter, **`Ctrl+Q`** quits, and **`F1`** opens the shortcuts reference (also in the header menu). **Double-click / Enter** now plays a track *or* a facet value (Phase 13e-i). Earlier-wired: `Ctrl+F` (filter), `Ctrl+Enter` (append), `Ctrl+U/I/P` (panels), `Ctrl+,` (preferences), `Ctrl+E` (edit), `Ctrl+M` (stop-after), `Ctrl+J` (jump), `Ctrl+Shift+→/←` (chapters), `S` (sleep), `Alt+1/2/3` (views), the queue keys (`Alt+↑/↓`, `Delete`, `Ctrl+Shift+C`), and media keys via MPRIS2. **Deliberately deferred** (marked below): bare `→/←` and `Shift+→/←` seek (the arrows navigate the browse columns, so they are intentionally unbound, as in deadbeef-cui), the `Ctrl+S` and `Delete` *bindings* (their actions exist elsewhere: the sidebar save button saves a Perspective, and the right-click menu removes from library behind a confirm since 16a), and `Ctrl+Shift+J` (a jobs surface that does not exist yet). The keymap encodes spec §3.1's principle: **every action is keyboard-accessible, no hidden gestures, every swipe has a menu equivalent.** GNOME/libadwaita conventions are followed where one exists.

## Global

| Key | Action |
|---|---|
| `Ctrl+F` | Focus the filter bar (the full search grammar; no separate search mode, spec §3.4) |
| `Ctrl+L` | Clear the filter bar |
| `Ctrl+,` | Preferences (`AdwPreferencesDialog`) |
| `Ctrl+Q` | Quit |
| `F1` | Keyboard shortcuts window |
| `Alt+1` / `Alt+2` / `Alt+3` | Switch top-level view: Music / Podcasts / Audiobooks (the `AdwTabView` `Alt+N` convention; a global shortcut switching the `AdwViewStack`, Phase 6b-i. `Alt+3` is inert until the Audiobooks tab, 7b) |
| `Ctrl+Shift+J` | Open the jobs / activity surface (imports, moves, fetches) (proposed; `Ctrl+J` now jumps to the playing track, Phase 11d) |

## Browse (Music)

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | Move focus between facet panes and the track list |
| `↑` / `↓` | Move within the focused pane or list |
| `Ctrl+Click` / `Shift+Click` | Multi-select (aggregate facets; range-select rows) |
| `Double-click` / `Enter` | Play the track, or the facet value under the cursor, replacing the queue (the deadbeef-cui activate-to-play, Phase 13e-i) |
| `Ctrl+Enter` | Append the selection to the queue |
| `Ctrl+S` | Save the current filter as a Perspective (the binding is deferred; the sidebar's save button is the wired path, Phase 3c) |
| `Ctrl+E` | Edit metadata for the selection (bulk editor, Phase 5) |
| `Delete` | Remove from library (the key stays unbound; the right-click menu's Remove from Library is the confirmed path since Phase 16a, and files always stay on disk) |

## Playback

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `Ctrl+→` / `Ctrl+←` | Next / previous item |
| `Ctrl+Shift+→` / `Ctrl+Shift+←` | Next / previous chapter (episodes and audiobooks; a no-op without chapters) |
| `→` / `←` | Seek forward / back (small step) (proposed; not wired, the arrows navigate the browse) |
| `Shift+→` / `Shift+←` | Seek forward / back (large step) (proposed; not wired) |
| `Ctrl+↑` / `Ctrl+↓` | Volume up / down |
| `Ctrl+0` | Mute / unmute |
| `Ctrl+M` | Stop after current: finish the current item, then pause at the boundary (v0.0.76; also the header menu) |
| `Ctrl+J` | Jump to the playing track: select and scroll to it in the browse list (v0.0.76; also the header menu) |
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
| `I` | Move episode to Inbox (not yet wired; arrives with the batch-triage pass, 16.5d) |
| `Q` | Add episode to the (unified) Queue (not yet wired; arrives with 16.5d) |
| `R` | Refresh the selected show, or all shows when a bucket is selected (16.5c; also the sidebar footer button) |
| `Ctrl+Shift+O` | Import OPML (16.5c; also the sidebar footer menu, next to Export OPML) |

## Now Playing

| Key | Action |
|---|---|
| `Ctrl+Return` | Expand the Now-bar to the full Now Playing surface |
| `Esc` | Collapse Now Playing back to the Now-bar |
| `S` | Sleep timer menu (any playing item; the "end of item" row reads "End of track" / "End of episode" / "End of book" by kind) |

The earlier `Ctrl+1`/`Ctrl+2` top-level-switch overlap with the podcast triage lists is resolved: top-level view switching moved to `Alt+1/2/3` (the `AdwTabView` convention), leaving `Ctrl+1/2/3` to the triage lists within the Podcasts view.
