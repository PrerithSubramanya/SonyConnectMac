#!/bin/zsh
# Builds XM5 Popup and wraps it in a signed .app bundle at target/XM5 Popup.app.
#
# It has to be a real bundle: macOS only shows the Bluetooth permission prompt for an app with
# NSBluetoothAlwaysUsageDescription in its Info.plist, and attributes the permission to it.
#
# Usage: scripts/bundle-app.sh [--run]
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"

cargo build -p xm5-popup --release

APP="$PWD/target/XM5 Popup.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/xm5-popup "$APP/Contents/MacOS/xm5-popup"
cp crates/app/icons/icon.icns "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>dev.local.xm5popup</string>
    <key>CFBundleName</key><string>XM5 Popup</string>
    <key>CFBundleDisplayName</key><string>XM5 Popup</string>
    <key>CFBundleExecutable</key><string>xm5-popup</string>
    <key>CFBundleIconFile</key><string>icon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
    <key>LSUIElement</key><true/>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSBluetoothAlwaysUsageDescription</key>
    <string>XM5 Popup uses Bluetooth to notice your Sony WH-1000XM5, connect it, and read and change its settings.</string>
    <key>NSBluetoothPeripheralUsageDescription</key>
    <string>XM5 Popup uses Bluetooth to notice your Sony WH-1000XM5, connect it, and read and change its settings.</string>
</dict>
</plist>
PLIST
# Ad-hoc signing changes the app's identity on every build, so macOS asked for Bluetooth
# permission again each time. A fixed requirement on the bundle id keeps the same identity.
codesign --force --sign - -r='designated => identifier "dev.local.xm5popup"' "$APP" 2>/dev/null
echo "Built: $APP"

if [[ "${1:-}" == "--run" ]]; then
  open "$APP"
  echo "Started. Log: ~/Library/Logs/XM5Popup.log"
fi
