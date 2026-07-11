# Running Conservatory under Hyprland

Environment-specific setup for a Hyprland (or any wlroots-style, GNOME-free)
session, collected here per the Phase 26 verification tail. Conservatory is
plain GTK4 with its own stylesheet (spec §2.4), so nothing below changes how
the app looks or behaves; these notes cover the session services GTK expects
and the integration niceties a tiling setup wants.

## Window rules

The app id is stable: `org.virinvictus.Conservatory` (`main.rs`, unchanged
across versions, so rules never break on update). The main window tiles
cleanly; dialogs (Preferences, the alert prompts, the shortcuts reference)
are real windows since Phase 26 and follow normal modal-float behaviour.

```conf
# ~/.config/hypr/hyprland.conf
# Float the dialogs (they are modal transients; most compositors float them
# already, this makes it explicit):
windowrulev2 = float, class:^(org\.virinvictus\.Conservatory)$, title:^(Preferences|Keyboard Shortcuts)$

# Example: pin the whole app to a workspace
windowrulev2 = workspace 9, class:^(org\.virinvictus\.Conservatory)$
```

A dedicated compact / mini-player window that a scratchpad rule can float is
a planned later phase (roadmap Phase 25, "Compact mode"); until it lands the
smallest useful surface is the main window at a narrow tile (the tab switcher
migrates to the bottom edge below 550px width).

## Portals

GTK's file dialogs and the settings read go through `xdg-desktop-portal`.
Install a backend and make sure the portal service starts with the session:

```sh
sudo dnf install xdg-desktop-portal-hyprland xdg-desktop-portal-gtk
```

`xdg-desktop-portal-hyprland` covers screencast/screenshot;
`xdg-desktop-portal-gtk` answers the FileChooser portal Conservatory's
import / OPML / library-root pickers use. Without a running portal the app
still works; you get harmless `Cannot get portal ... version` warnings on
stderr and the file dialogs fall back to GTK's built-in implementation.

## Secret Service (private podcast feeds)

Basic-auth feed credentials store through `oo7`, which needs a Secret Service
provider on the bus. GNOME sessions start one implicitly; on Hyprland add:

```conf
exec-once = gnome-keyring-daemon --start --components=secrets
```

Without it Conservatory degrades gracefully: private feeds poll anonymously
instead of crashing, and credentials simply cannot be saved.

## Suspend inhibitor

The playing-audio suspend inhibitor rides `org.freedesktop.login1`
(systemd-logind), not GNOME, so it works unchanged under Hyprland.

## Waybar

MPRIS metadata is complete (`mpris:trackid`, `xesam:title` / `artist` /
`album`, `mpris:length`, percent-encoded `mpris:artUrl`), so the stock waybar
`mpris` module works as-is:

```json
"mpris": {
  "format": "{player_icon} {artist} - {title}",
  "format-paused": "{status_icon} {artist} - {title}",
  "player-icons": { "default": "▶" },
  "status-icons": { "paused": "⏸" },
  "player": "conservatory"
}
```

`playerctl -p conservatory metadata` is the quick way to confirm the fields
from a terminal.

## Fonts and theme

Nothing to set up: the three UI fonts are bundled and registered through
fontconfig at startup, and the Kanagawa Dragon stylesheet is generated and
installed by the app itself above user-CSS priority, so a themed
`~/.config/gtk-4.0/gtk.css` cannot half-override it (docs/theme.md).
