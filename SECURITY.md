# Security policy

Conservatory is a desktop library manager that reads untrusted audio files and
network podcast feeds, and moves the user's files on disk. The surfaces that
matter most: anything that could delete or corrupt library files (the mover's
journal and recovery), anything that could corrupt the SQLite database, and the
parsing of untrusted input (tags, feeds, OPML, chapters JSON).

## Supported versions

One developer, one supported line: the latest tagged release only. There are no
LTS branches.

## Reporting a vulnerability

Please use GitHub's **private vulnerability reporting** (Security -> Report a
vulnerability on this repository) rather than a public issue. Include the
Conservatory version, the OS, and the smallest reproduction you can manage; for
a file-parsing issue, attach or describe the file shape (never your whole
library).

You should hear back within a few days. Fixes land in the next release; there
is no patched-backport machinery.
