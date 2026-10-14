#!/bin/zsh
# Usage: scripts/run-bundled.sh <probe|blescan> [args...]
#
# Builds the crate, wraps the binary in a signed .app (so macOS shows the Bluetooth permission
# prompt for the app itself, not for the terminal), runs it through LaunchServices and prints
# its log.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"

CRATE=${1:?usage: run-bundled.sh <probe|blescan> [args...]}
shift

cargo build -p "$CRATE" --release

APP="$PWD/target/$CRATE.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "target/release/$CRATE" "$APP/Contents/MacOS/$CRATE"
cp "crates/$CRATE/Info.plist" "$APP/Contents/Info.plist"
codesign --force --sign - "$APP" 2>/dev/null

LOG="$PWD/target/$CRATE.log"
: > "$LOG"
open -W -n "$APP" --stdout "$LOG" --stderr "$LOG" --args "$@"
cat "$LOG"
