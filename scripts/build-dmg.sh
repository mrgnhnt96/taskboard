#!/bin/bash
# Packs Taskboard.app into a DMG with the branded window: the background from
# packaging/assets/dmg, the app on the left and Applications on the right.
#
#   ./scripts/build-dmg.sh [app] [out.dmg]
#
# Defaults to the app packaging/build-app.sh makes for the workspace version
# and Taskboard.dmg. The layout lives in scripts/dmg-settings.py.
set -euo pipefail
cd "$(dirname "$0")/.."

version="$(awk '/^\[workspace.package\]/{p=1} p && /^version *=/{gsub(/[" ]/,""); sub(/version=/,""); print; exit}' Cargo.toml)"
app="${1:-dist/$version/Taskboard.app}"
out="${2:-Taskboard.dmg}"
[ -d "$app" ] || { echo "No app at $app. Build it first (packaging/build-app.sh)." >&2; exit 1; }

# dmgbuild writes Finder's layout file directly, so it works on CI without
# driving Finder. 1.6.7 is the first whose background link still works
# once the image is compressed on macOS 26, and it needs Python 3.10+.
python=""
for candidate in python3.12 python3.13 python3; do
  if command -v "$candidate" >/dev/null 2>&1 &&
    "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 10))'; then
    python="$candidate" && break
  fi
done
[ -n "$python" ] || { echo "Needs Python 3.10 or newer: brew install python@3.12" >&2; exit 1; }
venv=$(mktemp -d)
trap 'rm -rf "$venv"' EXIT
"$python" -m venv "$venv"
"$venv/bin/pip" install --quiet --disable-pip-version-check dmgbuild==1.6.7

"$venv/bin/dmgbuild" -s scripts/dmg-settings.py \
  -D app="$(cd "$(dirname "$app")" && pwd)/$(basename "$app")" -D root="$PWD" \
  Taskboard "$out"
echo "Wrote $out"
