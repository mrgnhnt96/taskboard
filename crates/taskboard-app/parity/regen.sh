#!/bin/sh
# Regenerate the parity goldens from the frozen web board: every gen/*.mjs writes golden/<area>.json.
# Needs node. Commit the JSON; `cargo test -p taskboard-app` only reads it.
set -eu
cd "$(dirname "$0")"
for f in gen/*.mjs; do TZ=UTC node "$f"; done
