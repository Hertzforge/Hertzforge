#!/usr/bin/env bash
# Make "Hertzforge Dev", a Mac launcher that runs scripts/dev_app.sh from this checkout.
#
#     scripts/make_dev_launcher.sh [folder]
#
# The launcher goes in the folder named, ~/Applications by default. Opening it runs the
# development app in a Terminal window, which shows the build and the services' output;
# quitting the app ends it. Its icon is the app's with a DEV band, so it is not mistaken
# for an installed Hertzforge. Run this again after moving the checkout.
set -euo pipefail
[[ "$(uname)" == Darwin ]] || { echo "make_dev_launcher.sh: the launcher is a Mac applet" >&2; exit 1; }
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${1:-$HOME/Applications}"
APP="$DEST/Hertzforge Dev.app"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$DEST"

# the checkout's path as an AppleScript string, for the shell to quote
escaped="${ROOT//\\/\\\\}"
escaped="${escaped//\"/\\\"}"
rm -rf "$APP"
osacompile -o "$APP" -e "tell application \"Terminal\"
	activate
	do script \"cd \" & quoted form of \"$escaped\" & \" && scripts/dev_app.sh\"
end tell"

# the app's icon with an orange DEV band across its foot, kept inside the tile's shape
cat >"$WORK/band.swift" <<'SWIFT'
import AppKit
let args = CommandLine.arguments
guard let source = NSImage(contentsOfFile: args[1]),
      let icon = source.cgImage(forProposedRect: nil, context: nil, hints: nil) else { exit(1) }
let size = 1024
let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
                        space: CGColorSpaceCreateDeviceRGB(),
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
let whole = CGRect(x: 0, y: 0, width: size, height: size)
context.draw(icon, in: whole)
context.saveGState()
context.clip(to: whole, mask: icon)
let band = CGRect(x: 100, y: 170, width: 824, height: 140)
context.setFillColor(CGColor(red: 0.91, green: 0.47, blue: 0.13, alpha: 0.96))
context.fill(band)
context.restoreGState()
NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
let text = NSAttributedString(string: "DEV", attributes: [
    .font: NSFont.systemFont(ofSize: 112, weight: .bold), .foregroundColor: NSColor.white])
let measured = text.size()
text.draw(at: NSPoint(x: band.midX - measured.width / 2, y: band.midY - measured.height / 2))
let rep = NSBitmapImageRep(cgImage: context.makeImage()!)
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))
SWIFT
swift "$WORK/band.swift" "$ROOT/desktop/icon.png" "$WORK/dev.png"
mkdir "$WORK/dev.iconset"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$WORK/dev.png" --out "$WORK/dev.iconset/icon_${s}x${s}.png" >/dev/null
  sips -z $((2 * s)) $((2 * s)) "$WORK/dev.png" --out "$WORK/dev.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$WORK/dev.iconset" -o "$APP/Contents/Resources/applet.icns"

/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier org.hertzforge.dev-launcher" "$APP/Contents/Info.plist" 2>/dev/null \
  || /usr/libexec/PlistBuddy -c "Add :CFBundleIdentifier string org.hertzforge.dev-launcher" "$APP/Contents/Info.plist"
codesign --force --sign - "$APP" 2>/dev/null
touch "$APP"
echo "make_dev_launcher.sh: $APP runs $ROOT/scripts/dev_app.sh"
