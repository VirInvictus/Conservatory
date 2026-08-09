project(
    'conservatory',
    # Read from the VERSION file so meson cannot drift from it. This said
    # '0.0.1' while VERSION said 0.3.13, which is exactly the drift the
    # single-source rule exists to prevent; hard-coding it again would just
    # reset the clock on the same bug.
    version: files('VERSION'),
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

application_id = 'io.github.virinvictus.conservatory'
datadir = get_option('datadir')

# The desktop entry. Deliberately NOT named .desktop.in (Atrium's convention):
# that suffix promises an i18n.merge_file substitution step, and Conservatory has
# no po/ yet, so the suffix would be a lie. It also breaks validation outright,
# since desktop-file-validate refuses any filename not ending in .desktop.
# Rename it back to .in the day translations land and merge_file is real.
install_data(
    'data' / application_id + '.desktop',
    install_dir: datadir / 'applications',
)

# AppStream metainfo, read by gnome-software and KDE Discover, and required by
# Flathub. The version and release date are substituted from the project version
# (i.e. from VERSION) so a release cannot ship metadata describing the previous
# one.
metainfo_conf = configuration_data()
metainfo_conf.set('VERSION', meson.project_version())
metainfo_conf.set('RELEASE_DATE', run_command(
    'date', '-u', '+%Y-%m-%d', check: true).stdout().strip())
metainfo_file = configure_file(
    input: 'data' / application_id + '.metainfo.xml.in',
    output: application_id + '.metainfo.xml',
    configuration: metainfo_conf,
    install: true,
    install_dir: datadir / 'metainfo',
)

# Validate at build time rather than trusting a hand-run check: a metainfo file
# that stops validating is a Flathub submission blocker, and the cheapest place
# to learn that is here.
appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
    test('metainfo-validates',
         appstreamcli,
         args: ['validate', '--no-net', '--pedantic', metainfo_file])
endif

desktop_file_validate = find_program('desktop-file-validate', required: false)
if desktop_file_validate.found()
    test('desktop-entry-validates',
         desktop_file_validate,
         args: [meson.current_source_dir() / 'data' / application_id + '.desktop'])
endif

# Scalable app icon. Currently the placeholder glasshouse from logo.svg; the
# final conservatory-silhouette mark is Brandon's call (spec §15), and the
# metainfo screenshots stay commented out until it lands, because a screenshot
# showing a placeholder icon is worse than no screenshot.
install_data(
    'data/icons/hicolor/scalable/apps/' + application_id + '.svg',
    install_dir: datadir / 'icons' / 'hicolor' / 'scalable' / 'apps',
)
