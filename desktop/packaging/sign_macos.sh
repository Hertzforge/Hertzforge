#!/usr/bin/env bash
# Sign and notarize the macOS app, then make, sign and notarize its disk image.
#
#     desktop/packaging/sign_macos.sh <Hertzforge.app> <output .dmg>
#
# Needs a "Developer ID Application" identity in a keychain (ci_signing.sh puts one
# there on a runner; MACOS_SIGNING_IDENTITY names it, or the first one found is
# used) and, to notarize, NOTARY_APPLE_ID, NOTARY_PASSWORD (an app-specific
# password) and NOTARY_TEAM_ID. Without the notary values the app and image are
# signed and not notarized, and the script says so.
#
# Adapted from the signing half of Visual Dynamics' packaging/build_macos.sh (MIT,
# Brandon Zwink). What carries over, each learned there: sign inside out, every
# Mach-O found by content rather than by name (Python's extension modules and the
# libraries in its wheels have assorted names, and Apple refuses a bundle with one
# unsigned); the hardened runtime on all of it, with entitlements.plist; notarize
# and staple the app before the image is made, so a copied app opens offline; then
# sign, notarize and staple the image.
#
# Not run yet: no Developer ID is configured for this project. It is the step the
# desktop workflow runs when HERTZFORGE_SIGN_MACOS is "true" (docs/desktop-app.md).
set -euo pipefail
trap 'echo "sign_macos.sh: stopped at line $LINENO (exit $?)" >&2' ERR
APP="${1:?usage: sign_macos.sh <app> <dmg>}"
DMG="${2:?usage: sign_macos.sh <app> <dmg>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
SIGN="${MACOS_SIGNING_IDENTITY:-}"
if [[ -z $SIGN ]]; then
  SIGN=$(security find-identity -v -p codesigning 2>/dev/null |
         sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)
fi
[[ -n $SIGN ]] || { echo "sign_macos.sh: no Developer ID Application identity" >&2; exit 1; }
flags=(--force --timestamp --options runtime --entitlements "$HERE/entitlements.plist" --sign "$SIGN")

# Deepest files first, then the app.
find "$APP/Contents" -type f -print0 | xargs -0 file |
  grep -E ': *Mach-O' | cut -d: -f1 |
  awk '{print gsub("/", "/") "\t" $0}' | sort -rn | cut -f2- |
  tr '\n' '\0' | xargs -0 -n 40 codesign "${flags[@]}" 2>&1 |
  grep -v 'replacing existing signature' || true
codesign "${flags[@]}" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP" 2>&1 | tail -2
echo "sign_macos.sh: signed as $SIGN"

notarize() {
  if [[ -n ${NOTARY_APPLE_ID:-} && -n ${NOTARY_PASSWORD:-} && -n ${NOTARY_TEAM_ID:-} ]]; then
    xcrun notarytool submit "$1" --apple-id "$NOTARY_APPLE_ID" \
      --team-id "$NOTARY_TEAM_ID" --password "$NOTARY_PASSWORD" --wait | tail -3
    return 0
  fi
  return 1
}

ZIP="$(dirname "$DMG")/notarize-$$.zip"
ditto -c -k --keepParent "$APP" "$ZIP"
if notarize "$ZIP"; then
  xcrun stapler staple -q "$APP"
  spctl --assess --type execute -v "$APP"
  notarized=1
else
  echo "sign_macos.sh: warning: no notary credentials; signed but not notarized" >&2
  notarized=
fi
rm -f "$ZIP"

STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "$(basename "$APP" .app)" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
codesign --force --timestamp --sign "$SIGN" "$DMG"
if [[ -n $notarized ]] && notarize "$DMG"; then
  xcrun stapler staple -q "$DMG"
  spctl --assess --type open --context context:primary-signature -v "$DMG"
fi
echo "sign_macos.sh: $DMG ($(du -sh "$DMG" | cut -f1))"
