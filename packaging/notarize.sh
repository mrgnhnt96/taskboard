#!/bin/bash
# OPT-IN: notarize and staple a built Taskboard.app. This UPLOADS the app to Apple, so nothing
# runs it automatically (not build-app.sh, not install-app.sh).
#
#   packaging/notarize.sh dist/1.0.0/Taskboard.app
#
# One-time setup (stores an app-specific password in the keychain under a profile name):
#   xcrun notarytool store-credentials taskboard-notary --apple-id <you@example.com> --team-id U2G2XV3688
# Env: TASKBOARD_NOTARY_PROFILE (default taskboard-notary).
# CI (.github/workflows/release.yml) uses an App Store Connect API key instead of a profile:
#   TASKBOARD_NOTARY_KEY (path to the .p8), TASKBOARD_NOTARY_KEY_ID, TASKBOARD_NOTARY_ISSUER.
# Order for a release: build-app.sh -> notarize.sh -> build-dmg.sh.
set -euo pipefail
APP="${1:?usage: notarize.sh path/to/Taskboard.app}"
PROFILE="${TASKBOARD_NOTARY_PROFILE:-taskboard-notary}"
XCRUN=/usr/bin/xcrun   # not the toolchain shim from env.sh
/usr/bin/codesign --verify --deep --strict "$APP"
if /usr/bin/codesign -dv "$APP" 2>&1 | grep -q 'Signature=adhoc'; then
  echo "notarize.sh: $APP is ad-hoc signed; notarization needs a Developer ID signature" >&2; exit 1
fi
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
ZIP="$TMP/Taskboard.zip"
/usr/bin/ditto -c -k --keepParent "$APP" "$ZIP"
echo "==> submitting to Apple notary service; this uploads the app"
if [ -n "${TASKBOARD_NOTARY_KEY:-}" ]; then
  AUTH=(--key "$TASKBOARD_NOTARY_KEY" --key-id "${TASKBOARD_NOTARY_KEY_ID:?}" --issuer "${TASKBOARD_NOTARY_ISSUER:?}")
else
  AUTH=(--keychain-profile "$PROFILE")
fi
"$XCRUN" notarytool submit "$ZIP" "${AUTH[@]}" --wait
"$XCRUN" stapler staple "$APP"
/usr/sbin/spctl -a -t exec -vv "$APP"
echo "==> notarized and stapled $APP"
