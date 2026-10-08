#!/bin/bash
# Build release binaries and assemble a signed Taskboard.app.
#
#   packaging/build-app.sh [options]
#     --version V         version for the bundle (default: workspace Cargo version)
#     --out DIR           output directory (default: dist/<version>); the app is DIR/<name>.app
#     --adhoc             ad-hoc sign even when a Developer ID certificate is available
#     --no-build          reuse the binaries from the last build (same --target-dir)
#     --target-dir DIR    cargo target dir (default: target/package, kept apart from dev builds)
#     --bundle-id ID      CFBundleIdentifier (default com.mrgnhnt.taskboard)
#     --label LABEL       LaunchAgent label (default <bundle id>.daemon)
#     --name NAME         CFBundleName / app file name (default Taskboard)
#     --icon FILE         .icns to ship (default packaging/assets/Taskboard.icns)
#     --scheme NAME       URL scheme the app opens (default taskboard: taskboard://task/T12)
#     --env K=V           add K=V to the app's LSEnvironment AND the daemon's EnvironmentVariables
#                         (e.g. TASKBOARD_DATA=/tmp/x, TASKBOARD_PORT=18792); repeatable
#
# The bundle holds taskboard-app (the window), taskboardd (the board, run by launchd through the
# LaunchAgent in Contents/Library/LaunchAgents, registered by the app with SMAppService) and
# tb (the agent CLI; the app links ~/.local/bin/tb to it).
#
# Env: TASKBOARD_SIGN_IDENTITY (codesign identity; default the first "Developer ID Application" one),
#      TASKBOARD_SIGN_TIMESTAMP=0 (sign without Apple's timestamp server, e.g. offline).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="" OUT="" ADHOC=0 BUILD=1 TARGET_DIR="$ROOT/target/package"
BUNDLE_ID="com.mrgnhnt.taskboard" LABEL="" NAME="Taskboard" ICON="" SCHEME="taskboard" EXTRA_ENV=()
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --adhoc) ADHOC=1; shift ;;
    --no-build) BUILD=0; shift ;;
    --target-dir) TARGET_DIR="$2"; shift 2 ;;
    --bundle-id) BUNDLE_ID="$2"; shift 2 ;;
    --label) LABEL="$2"; shift 2 ;;
    --name) NAME="$2"; shift 2 ;;
    --icon) ICON="$2"; shift 2 ;;
    --scheme) SCHEME="$2"; shift 2 ;;
    --env) EXTRA_ENV+=("$2"); shift 2 ;;
    -h|--help) sed -n '2,23p' "$0"; exit 0 ;;
    *) echo "build-app.sh: unknown option $1" >&2; exit 2 ;;
  esac
done

LABEL="${LABEL:-$BUNDLE_ID.daemon}"
if [ -z "$VERSION" ]; then
  VERSION="$(awk '/^\[workspace.package\]/{p=1} p && /^version *=/{gsub(/[" ]/,"",$0); sub(/version=/,""); print; exit}' Cargo.toml)"
fi
OUT="${OUT:-$ROOT/dist/$VERSION}"
case "$OUT" in /*) ;; *) OUT="$ROOT/$OUT" ;; esac
APP="$OUT/$NAME.app"

# ---------------------------------------------------------------- build
BIN="$TARGET_DIR/release"
if [ "$BUILD" = 1 ]; then
  echo "==> cargo build --release (taskboard $VERSION, target $TARGET_DIR)"
  MACOSX_DEPLOYMENT_TARGET=13.0 CARGO_TARGET_DIR="$TARGET_DIR" \
    cargo build --release -p taskboard-app -p taskboardd -p taskboard-cli --bin taskboard-app --bin taskboardd --bin tb
fi
for b in taskboard-app taskboardd tb; do [ -x "$BIN/$b" ] || { echo "missing $BIN/$b" >&2; exit 1; }; done

# ---------------------------------------------------------------- assemble
echo "==> assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/Fonts" "$APP/Contents/Library/LaunchAgents"
cp "$BIN/taskboard-app" "$BIN/taskboardd" "$BIN/tb" "$APP/Contents/MacOS/"
if [ -z "$ICON" ]; then
  ICON=packaging/assets/Taskboard.icns
  [ -f "$ICON" ] || python3 packaging/make-icon.py "$ICON"
fi
cp "$ICON" "$APP/Contents/Resources/Taskboard.icns"
# The fonts are compiled into taskboard-app (include_bytes!); their OFL licenses travel with it.
cp crates/taskboard-app/assets/fonts/OFL-*.txt "$APP/Contents/Resources/Fonts/"
# The Claude Code plugin (hooks, skill, tb shim). The status bar's "Install hooks" points Claude
# Code's taskboard marketplace here; Claude loads it in place, so updating the app updates it.
ditto plugin "$APP/Contents/Resources/plugin"

env_dict() {  # <dict> body for EXTRA_ENV (+ any fixed pairs given as args)
  local kv
  for kv in "$@" ${EXTRA_ENV[@]+"${EXTRA_ENV[@]}"}; do
    printf '    <key>%s</key><string>%s</string>\n' "${kv%%=*}" "${kv#*=}"
  done
}

LSENV=""
if [ ${#EXTRA_ENV[@]} -gt 0 ]; then
  LSENV="  <key>LSEnvironment</key>
  <dict>
$(env_dict)
  </dict>"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleExecutable</key><string>taskboard-app</string>
  <key>CFBundleIconFile</key><string>Taskboard</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>$BUNDLE_ID</string>
      <key>CFBundleURLSchemes</key><array><string>$SCHEME</string></array>
    </dict>
  </array>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key><string>© Morgan Hunt</string>
  <key>NSAppleEventsUsageDescription</key><string>The task board asks Midna to open, focus and close Claude terminals.</string>
  <key>TaskboardDaemonLabel</key><string>$LABEL</string>
$LSENV
</dict>
</plist>
PLIST

# SMAppService runs the bundle's taskboardd (BundleProgram). KeepAlive restarts it after a crash
# but not after a clean exit (e.g. "something already listens on the port").
cat > "$APP/Contents/Library/LaunchAgents/$LABEL.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>BundleProgram</key><string>Contents/MacOS/taskboardd</string>
  <key>ProgramArguments</key>
  <array><string>Contents/MacOS/taskboardd</string><string>serve</string></array>
  <key>AssociatedBundleIdentifiers</key><array><string>$BUNDLE_ID</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Interactive</string>
  <key>EnvironmentVariables</key>
  <dict>
$(env_dict)
  </dict>
</dict>
</plist>
PLIST
plutil -lint -s "$APP/Contents/Info.plist" "$APP/Contents/Library/LaunchAgents/$LABEL.plist"

# ---------------------------------------------------------------- sign (inside out)
IDENTITY="${TASKBOARD_SIGN_IDENTITY:-}"
if [ -z "$IDENTITY" ] && [ "$ADHOC" = 0 ]; then
  IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)"
fi
[ -n "$IDENTITY" ] || IDENTITY="-"
SIGN=(/usr/bin/codesign --force --sign "$IDENTITY" --options runtime --entitlements packaging/entitlements.plist)
if [ "$IDENTITY" != "-" ] && [ "${TASKBOARD_SIGN_TIMESTAMP:-1}" != 0 ]; then SIGN+=(--timestamp); else SIGN+=(--timestamp=none); fi
echo "==> signing as: $( [ "$IDENTITY" = "-" ] && echo "ad-hoc (no Developer ID certificate)" || echo "$IDENTITY")"
"${SIGN[@]}" -i "$BUNDLE_ID.daemon" "$APP/Contents/MacOS/taskboardd"
"${SIGN[@]}" -i "$BUNDLE_ID.tb" "$APP/Contents/MacOS/tb"
"${SIGN[@]}" "$APP"
/usr/bin/codesign --verify --deep --strict "$APP"
echo "==> built $APP ($VERSION, $(du -sh "$APP" | cut -f1))"
