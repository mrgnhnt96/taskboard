#!/bin/bash
# Screenshot or drive this checkout's taskboard-app hidden, with Slate: the window opens below the
# desktop, never takes focus, and the app never becomes the frontmost app, so nothing shows on
# your screen and the app you're using keeps the keyboard.
#
#   scripts/slate.sh shoot --out shot.png [--size 1360x860] [--wait 2000]
#   scripts/slate.sh drive --steps steps.json --out shots/ [--size 1360x860] [--wait 2000]
#   scripts/slate.sh --url http://127.0.0.1:18792 shoot …   # against a temp daemon, not the sample board
#
# Everything after the subcommand goes to `slate` (its README has the steps file); the app is
# added after `--`. The app runs on the in-process sample board (TASKBOARD_BACKEND=fake) unless
# --url names a daemon you started yourself (README, Developing); the real board's daemon is
# refused, since a drive clicks as the owner. The app's other screenshot variables
# (TASKBOARD_PAGE, TASKBOARD_OPEN, TASKBOARD_THEME, TASKBOARD_FAKE_*, …) pass through.
# TASKBOARD_PAGE=settings shoots or drives the Settings window instead of the main one.
#
# It builds taskboard-app with the `snapshot` feature, so the app renders its own shots (no
# Screen Recording permission, no native traffic lights), and installs the `slate` command at the
# rev crates/taskboard-app/Cargo.toml pins into <target>/slate.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

usage() {
  sed -n '5,8p' "$0" | sed 's/^# *//' >&2
  exit 2
}

URL=""
if [ "${1:-}" = "--url" ]; then
  [ $# -ge 2 ] || usage
  URL="$2"
  shift 2
fi
case "${1:-}" in shoot | drive) ;; *) usage ;; esac

if [ -n "$URL" ]; then
  port=$(printf '%s' "$URL" | sed -n 's|^https\{0,1\}://[^/:]*:\([0-9]*\).*|\1|p')
  [ -n "$port" ] || { echo "slate.sh: give --url with a port, like http://127.0.0.1:18792" >&2; exit 1; }
  real=$(sed -n 's/^port *= *\([0-9]*\).*/\1/p' "$HOME/.config/taskboard/config.toml" 2>/dev/null | head -1)
  if [ "$port" = 8792 ] || [ "$port" = "${real:-8792}" ]; then
    echo "slate.sh: $URL is the real board; start a temp daemon (README, Developing)" >&2
    exit 1
  fi
fi

# This checkout's own target dir: the terminal may point cargo at another checkout's.
export CARGO_TARGET_DIR="$ROOT/target"
echo "slate.sh: building taskboard-app" >&2
cargo build --quiet -p taskboard-app --features snapshot
app="$CARGO_TARGET_DIR/debug/taskboard-app"

rev=$(sed -n 's/^slate = {.*rev = "\([0-9a-f]*\)".*/\1/p' crates/taskboard-app/Cargo.toml)
[ -n "$rev" ] || { echo "slate.sh: can't find slate's rev in crates/taskboard-app/Cargo.toml" >&2; exit 1; }
# cargo skips the install when that rev is already there.
cargo install --quiet --locked --git https://github.com/mrgnhnt96/slate --rev "$rev" --root "$CARGO_TARGET_DIR/slate" slate-cli
slate="$CARGO_TARGET_DIR/slate/bin/slate"

if [ -n "$URL" ]; then
  export TASKBOARD_URL="$URL"
  unset TASKBOARD_BACKEND
else
  export TASKBOARD_BACKEND=fake
  unset TASKBOARD_URL
fi
# Dev mode: no install, login item or update check, even from a bundle.
export TASKBOARD_DEV=1
"$slate" "$@" -- "$app"
