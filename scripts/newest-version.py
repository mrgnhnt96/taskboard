#!/usr/bin/env python3
"""Print the newest of the versions given, ordered like semver.

    ./scripts/newest-version.py 0.7.0-beta.2 0.6.2 0.7.0   ->  0.7.0

Versions are major.minor.patch, optionally -beta.N. A beta comes before
its release (0.7.0-beta.2 < 0.7.0), as the app's updater orders them.
"""
import re
import sys

PATTERN = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?$")


def key(version):
    match = PATTERN.match(version)
    if not match:
        sys.exit(f"Not a version: {version}")
    major, minor, patch, beta = match.groups()
    stable = beta is None
    return (int(major), int(minor), int(patch), stable, int(beta or 0))


if len(sys.argv) < 2:
    sys.exit(f"Usage: {sys.argv[0]} <version>...")
print(max(sys.argv[1:], key=key))
