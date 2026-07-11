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

# Plain GTK4 only since Phase 26 (spec §2.4); 4.14 matches the Cargo v4_14 pin.
dependency('gtk4', version: '>= 4.14')
dependency('mpv', version: '>= 0.36')
dependency('sqlite3', version: '>= 3.38')

# Bundled OFL fonts (Phase 13d): Inter (body), Fraunces (headers), IBM Plex Mono
# (technical text). Installed into the data fonts dir so the packaged build's
# fontconfig finds them, never assuming a host font (spec §7.2.9). The native /
# dev build registers data/fonts/ at runtime instead (see register_bundled_fonts
# in conservatory/src/main.rs); this target is the packaging half of the same rule.
install_data(
    [
        'data/fonts/Inter.ttf',
        'data/fonts/Fraunces.ttf',
        'data/fonts/IBMPlexMono-Regular.ttf',
        'data/fonts/IBMPlexMono-Medium.ttf',
    ],
    install_dir: get_option('datadir') / 'fonts' / 'conservatory',
)
