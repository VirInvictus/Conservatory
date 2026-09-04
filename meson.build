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
# packaging and distribution layout: it invokes the cargo release build and
# installs the binaries plus the GNOME-shaped data files (GSettings-era schema
# compile, AppStream metainfo, the .desktop file, fonts, icon). Direct
# development still uses cargo directly; meson test runs the two packaging
# validators so a file that stops validating fails at build time.

# Plain GTK4 only since Phase 26 (spec §2.4); 4.14 matches the Cargo v4_14 pin.
dependency('gtk4', version: '>= 4.14')
dependency('mpv', version: '>= 0.36')
dependency('sqlite3', version: '>= 3.38')

# The Rust build (Phase 20): a thin orchestration layer around
# `cargo build --release`, the Atrium pattern. Cargo is the source of truth for
# compilation; meson handles the GNOME-shaped install layout (binaries into
# $bindir, fonts / desktop entry / metainfo / icon into $datadir) so the
# project ships through Flatpak and any distro that expects a meson buildsystem.
# `build_always_stale` lets cargo handle its own incremental logic instead of
# meson tracking the Rust source tree; CARGO_TARGET_DIR keeps the Rust
# artifacts inside the meson build dir so a DESTDIR stage never sees them.
cargo = find_program('cargo')
cargo_options = ['--manifest-path', meson.project_source_root() / 'Cargo.toml']
cargo_options += ['--release']
cargo_options += ['--workspace']

cargo_build = custom_target(
    'cargo-build',
    build_by_default: true,
    build_always_stale: true,
    output: ['conservatory', 'conservatory-cli'],
    console: true,
    install: true,
    install_dir: [get_option('bindir'), get_option('bindir')],
    command: [
        'env',
        'CARGO_TARGET_DIR=' + meson.project_build_root() / 'cargo-target',
        cargo, 'build', cargo_options,
        '&&', 'cp', meson.project_build_root() / 'cargo-target' / 'release' / 'conservatory', '@OUTPUT0@',
        '&&', 'cp', meson.project_build_root() / 'cargo-target' / 'release' / 'conservatory-cli', '@OUTPUT1@',
    ],
)

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
