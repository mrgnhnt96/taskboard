#!/bin/bash
# Build Taskboard.app from this checkout, install it to /Applications/Taskboard.app and (re)start
# it. On launch the app registers its daemon (label com.mrgnhnt.taskboard.daemon, port 8792,
# data in ~/.config/taskboard) as a login item and links ~/.local/bin/tb to its CLI. This is the
# real board: its runner is on, so queued tasks open and drive Midna terminals.
# For trying changes, use scripts/dev-app.sh (Taskboard Dev) instead.
#
#   scripts/install-app.sh [--test] [--no-build]
#     --test       run `cargo test --workspace` first and stop if it fails
#     --no-build   skip the build; just reinstall and restart from the last release build
#   scripts/install-app.sh --uninstall   quit it, unregister its daemon, remove the tb link and
#                                        delete the app (data in ~/.config/taskboard stays)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ID="com.mrgnhnt.taskboard"
LABEL="$ID.daemon"
PORT=8792
DEST="/Applications/Taskboard.app"
VERSION="$(awk '/^\[workspace.package\]/{p=1} p && /^version *=/{gsub(/[" ]/,"",$0); sub(/version=/,""); print; exit}' Cargo.toml)"
OUT="$ROOT/dist/$VERSION"

TEST=0 BUILD=1 UNINSTALL=0
for a in "$@"; do
  case "$a" in
    --test) TEST=1 ;;
    --no-build) BUILD=0 ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "install-app.sh: unknown option $a" >&2; exit 2 ;;
  esac
done

quit_app() {
  local pid
  pid="$(pgrep -f "$DEST/Contents/MacOS/taskboard-app" | head -1 || true)"
  [ -n "$pid" ] || return 0
  echo "==> quit Taskboard (pid $pid)"
  osascript -e "tell application id \"$ID\" to quit" >/dev/null
  for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || return 0; sleep 0.5; done
  echo "Taskboard didn't quit; leaving it running" >&2
  return 1
}

if [ "$UNINSTALL" = 1 ]; then
  quit_app
  if [ -x "$DEST/Contents/MacOS/taskboard-app" ]; then
    "$DEST/Contents/MacOS/taskboard-app" --uninstall || true
  fi
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
  rm -rf "$DEST"
  echo "==> removed $DEST (data kept: ~/.config/taskboard)"
  exit 0
fi

if [ "$TEST" = 1 ]; then
  echo "==> cargo test"
  if ! cargo test --workspace; then
    echo "tests failed; not installing" >&2
    exit 1
  fi
fi

if [ "$BUILD" = 1 ]; then
  echo "==> build Taskboard $VERSION"
  packaging/build-app.sh --out "$OUT" 2>&1 | grep -E "==> built|^error|warning: unused" || true
fi
[ -d "$OUT/Taskboard.app" ] || { echo "no app at $OUT/Taskboard.app (run without --no-build)" >&2; exit 1; }

# Copy next to the destination first, so the running copy is only quit once the new one is ready.
NEW="/Applications/.Taskboard.new.app" OLD="/Applications/.Taskboard.old.app"
rm -rf "$NEW" "$OLD"
ditto "$OUT/Taskboard.app" "$NEW"
quit_app || { rm -rf "$NEW"; exit 1; }
[ -e "$DEST" ] && mv "$DEST" "$OLD"
mv "$NEW" "$DEST"
rm -rf "$OLD"

# The daemon still runs the old binary: restart it (it's registered on first launch otherwise).
if launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1; then
  echo "==> restart its daemon"
  launchctl kickstart -k "gui/$(id -u)/$LABEL" || true
fi

echo "==> open $DEST"
open "$DEST"
for _ in $(seq 1 20); do
  sleep 0.5
  if curl -fsS "http://127.0.0.1:$PORT/tasks/api/state" >/dev/null 2>&1; then
    echo "    daemon answering on :$PORT"
    break
  fi
done
if [ -L "$HOME/.local/bin/tb" ]; then
  echo "    tb -> $(readlink "$HOME/.local/bin/tb")"
fi
echo "==> done"
