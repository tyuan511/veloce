#!/bin/sh
# Bundle the binary into Veloce.app.
#
# An unbundled macOS binary cannot reliably become the active application, so it
# never receives keystrokes and never shows its menu bar. Wrapping it in a bundle
# with a real Info.plist fixes activation, Dock behaviour and window ordering —
# which is also how the app has to ship.
#
# Usage: scripts/bundle-macos.sh [debug|release]

set -eu

PROFILE="${1:-release}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/target/Veloce.app"
BIN="$ROOT/target/$PROFILE/veloce"

if [ ! -x "$BIN" ]; then
    echo "error: $BIN not found. Run: cargo build --$PROFILE" >&2
    exit 1
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Veloce"
cp "$ROOT/packaging/macos/Info.plist" "$APP/Contents/Info.plist"

# Ad-hoc signature: enough for local runs and Gatekeeper-free launches on this
# machine. Distribution needs a Developer ID (see README).
codesign --force --sign - --timestamp=none "$APP" >/dev/null 2>&1 || \
    echo "warning: ad-hoc codesign failed; the bundle still runs locally" >&2

echo "$APP"
