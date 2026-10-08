#!/bin/bash
# Build "Taskboard Dev" from this checkout, install it to ~/Applications/Taskboard Dev.app and
# (re)start it. It runs side by side with the real Taskboard and never touches it: its own bundle
# id (com.mrgnhnt.taskboard.dev), its own daemon (label com.mrgnhnt.taskboard.dev.daemon) on its
# own port (18792), its own data folder, a purple icon with a yellow "DEV" band, and it leaves ~/.local/bin/tb
# alone. Its data folder isn't the default one, so its runner is off: it never opens, messages
# or closes Midna terminals (set TASKBOARD_RUNNER=1 in its environment to change that).
#
#   scripts/dev-app.sh [--test] [--no-build] [--seed]
#     --test       run `cargo test --workspace` first and stop if it fails
#     --no-build   skip the build; just reinstall and restart from the last dev build
#     --seed       fill an empty dev data folder with sample data first
#   scripts/dev-app.sh --uninstall   quit it, unregister its daemon, delete the app
#                                    (its data stays; delete it by hand to start fresh)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ID="com.mrgnhnt.taskboard.dev"
LABEL="$ID.daemon"
PORT=18792
DATA="$HOME/Library/Application Support/$ID"
DEST="$HOME/Applications/Taskboard Dev.app"
OUT="$ROOT/dist/dev"

TEST=0 BUILD=1 UNINSTALL=0 SEED=0
for a in "$@"; do
  case "$a" in
    --test) TEST=1 ;;
    --no-build) BUILD=0 ;;
    --seed) SEED=1 ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "dev-app.sh: unknown option $a" >&2; exit 2 ;;
  esac
done

quit_dev_app() {
  local pid
  pid="$(pgrep -f "$DEST/Contents/MacOS/taskboard-app" | head -1 || true)"
  [ -n "$pid" ] || return 0
  echo "==> quit Taskboard Dev (pid $pid)"
  osascript -e "tell application id \"$ID\" to quit" >/dev/null
  for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || return 0; sleep 0.5; done
  echo "Taskboard Dev didn't quit; leaving it running" >&2
  return 1
}

if [ "$UNINSTALL" = 1 ]; then
  quit_dev_app
  if [ -x "$DEST/Contents/MacOS/taskboard-app" ]; then
    "$DEST/Contents/MacOS/taskboard-app" --uninstall || true
  fi
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
  rm -rf "$DEST"
  echo "==> removed $DEST (data kept: $DATA)"
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
  echo "==> build Taskboard Dev"
  # Own target dir, so dev-app and release builds don't rebuild each other.
  packaging/build-app.sh --out "$OUT" --target-dir "$ROOT/target/dev-app" --adhoc \
    --bundle-id "$ID" --label "$LABEL" --name "Taskboard Dev" --icon packaging/assets/TaskboardDev.icns --scheme taskboard-dev \
    --env "TASKBOARD_PAGE_URL=taskboard-dev://" --env "TASKBOARD_DATA=$DATA" --env "TASKBOARD_PORT=$PORT" --env "TASKBOARD_NO_TB_LINK=1" 2>&1 | grep -E "==> built|^error|warning: unused" || true
fi
[ -d "$OUT/Taskboard Dev.app" ] || { echo "no app at $OUT/Taskboard Dev.app (run without --no-build)" >&2; exit 1; }

if [ "$SEED" = 1 ]; then
  mkdir -p "$DATA"
  TASKBOARD_DATA="$DATA" "$OUT/Taskboard Dev.app/Contents/MacOS/taskboardd" seed --data "$DATA" || true
fi

# Copy next to the destination first, so the running copy is only quit once the new one is ready.
mkdir -p "$(dirname "$DEST")"
NEW="$(dirname "$DEST")/.Taskboard Dev.new.app" OLD="$(dirname "$DEST")/.Taskboard Dev.old.app"
rm -rf "$NEW" "$OLD"
ditto "$OUT/Taskboard Dev.app" "$NEW"
quit_dev_app || { rm -rf "$NEW"; exit 1; }
# Each ad-hoc build has a new signature, and launchd won't spawn it under the old registration
# (EX_CONFIG, "needs LWCR update"): unregister the old daemon so the new app registers its own.
if launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1; then
  echo "==> unregister the old daemon"
  [ -x "$DEST/Contents/MacOS/taskboard-app" ] && "$DEST/Contents/MacOS/taskboard-app" --uninstall >/dev/null 2>&1 || true
  launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
fi
[ -e "$DEST" ] && mv "$DEST" "$OLD"
mv "$NEW" "$DEST"
rm -rf "$OLD"

echo "==> open $DEST"
open "$DEST"
for _ in $(seq 1 20); do
  sleep 0.5
  if curl -fsS "http://127.0.0.1:$PORT/tasks/api/state" >/dev/null 2>&1; then
    echo "    daemon answering on :$PORT"
    break
  fi
done
curl -fsS "http://127.0.0.1:$PORT/tasks/api/state" >/dev/null 2>&1 || echo "    its daemon isn't answering on :$PORT yet (launchctl print gui/$(id -u)/$LABEL)" >&2
echo "==> done"
