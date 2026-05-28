project(
    'conservatory',
    version: '0.0.1',
    license: 'GPL-3.0-or-later',
    meson_version: '>= 1.4.0',
)

# Conservatory is built with Cargo. Meson is the wrapper for Flatpak
# packaging and distribution layout: it handles GSettings schema
# compilation, AppStream metainfo, the .desktop file, and icon install.
# Direct development uses cargo. Real Cargo integration and the data/
# install targets ship with the GTK phases (Phase 3+, see roadmap.md);
# this scaffold establishes the directory layout and dependency checks.

dependency('gtk4', version: '>= 4.16')
dependency('libadwaita-1', version: '>= 1.7')
dependency('mpv', version: '>= 0.36')
dependency('sqlite3', version: '>= 3.38')
