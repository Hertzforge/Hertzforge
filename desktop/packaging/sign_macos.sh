#!/usr/bin/env bash
# Sign and notarize the macOS app, then make, sign and notarize its disk image.
#
#     desktop/packaging/sign_macos.sh <Hertzforge.app> <output .dmg>
#
# Needs a "Developer ID Application" identity in a keychain (ci_signing.sh puts one
# there on a runner; MACOS_SIGNING_IDENTITY names it, or the first one found is
# used). To notarize, notarytool is told who we are the first way that answers:
#
# 1. NOTARY_APPLE_ID and NOTARY_PASSWORD (an app-specific password) from the
#    environment, as the desktop workflow fills them from its secrets;
# 2. a login-keychain item NOTARY_ITEM (default vd-notary-password) holding the
#    app-specific password with the Apple ID as its account, the item Visual
#    Dynamics' builds on the same developer's Mac already use; a script reads it
#    with no prompt while the login keychain is unlocked;
# 3. the notarytool profile NOTARY_PROFILE (default vd-notary), made once with
#    `xcrun notarytool store-credentials`. It lives in the data-protection
#    keychain, which macOS opens only to an unlocked user session, so from a
#    script it can be unreachable for a while; VD learned that and keeps item 2.
#
# The team is NOTARY_TEAM_ID, or else the one the identity's name ends with.
# Without credentials that answer, the app and image are signed and not
# notarized, and the script says so.
#
# Adapted from the signing half of Visual Dynamics' packaging/build_macos.sh (MIT,
# Brandon Zwink). What carries over, each learned there: sign inside out, every
# Mach-O found by content rather than by name (Python's extension modules and the
# libraries in its wheels have assorted names, and Apple refuses a bundle with one
# unsigned); the hardened runtime on all of it, with entitlements.plist; notarize
# and staple the app before the image is made, so a copied app opens offline; then
# sign, notarize and staple the image.
#
# Run on the developer's Mac after a build (first on November 3, 2026, notarized on
# the first try), and the step the desktop workflow runs when HERTZFORGE_SIGN_MACOS
# is "true", which it is not yet (docs/desktop-app.md).
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

TEAM="${NOTARY_TEAM_ID:-$(sed -n 's/.*(\([A-Z0-9]\{10\}\))$/\1/p' <<<"$SIGN")}"
NOTARY_ITEM="${NOTARY_ITEM:-vd-notary-password}"
NOTARY_PROFILE="${NOTARY_PROFILE:-vd-notary}"
NOTARY=()
if [[ -n ${NOTARY_APPLE_ID:-} && -n ${NOTARY_PASSWORD:-} ]]; then
  NOTARY=(--apple-id "$NOTARY_APPLE_ID" --team-id "$TEAM" --password "$NOTARY_PASSWORD")
  echo "sign_macos.sh: notarizing as the Apple ID in the environment"
elif password=$(security find-generic-password -s "$NOTARY_ITEM" -w 2>/dev/null) && [[ -n $password ]]; then
  account=$(security find-generic-password -s "$NOTARY_ITEM" 2>/dev/null |
            sed -n 's/.*"acct"<blob>="\(.*\)"/\1/p')
  NOTARY=(--apple-id "$account" --team-id "$TEAM" --password "$password")
  echo "sign_macos.sh: notarizing with the login-keychain item $NOTARY_ITEM"
else
  NOTARY=(--keychain-profile "$NOTARY_PROFILE")
fi

notarize() {
  # asking for the submission history first is the one check that the
  # credentials still answer, before minutes of uploading
  xcrun notarytool history "${NOTARY[@]}" >/dev/null 2>&1 || return 1
  local said id
  said=$(xcrun notarytool submit "$1" "${NOTARY[@]}" --wait)
  echo "$said" | tail -3
  # --wait exits 0 whatever Apple decided: the status says, and Apple's log says why
  if ! grep -q 'status: Accepted' <<<"$said"; then
    id=$(sed -n 's/^ *id: //p' <<<"$said" | head -1)
    [[ -n $id ]] && xcrun notarytool log "$id" "${NOTARY[@]}" >&2
    echo "sign_macos.sh: Apple did not accept $(basename "$1")" >&2
    exit 1
  fi
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
