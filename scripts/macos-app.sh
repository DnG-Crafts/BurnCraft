#!/bin/sh
# Builds BurnCraft.app for macOS: an app you can double-click, keep in Applications and the Dock,
# which opens without a Terminal window.
#
#   sh scripts/macos-app.sh
#
# The app ends up in target/release/BurnCraft.app. Drag it to Applications.
set -e
cd "$(dirname "$0")/.."

cargo build --release -p burncraft

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
APP=target/release/BurnCraft.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/burncraft "$APP/Contents/MacOS/burncraft"

# The icon, at every size macOS asks for.
ICONSET=target/release/BurnCraft.iconset
rm -rf "$ICONSET"
mkdir "$ICONSET"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" assets/icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" assets/icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/BurnCraft.icns"
rm -rf "$ICONSET"

# NSCameraUsageDescription is required: without it macOS closes the app when it opens a camera.
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>BurnCraft</string>
    <key>CFBundleDisplayName</key>
    <string>BurnCraft</string>
    <key>CFBundleIdentifier</key>
    <string>org.burncraft.BurnCraft</string>
    <key>CFBundleExecutable</key>
    <string>burncraft</string>
    <key>CFBundleIconFile</key>
    <string>BurnCraft</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSCameraUsageDescription</key>
    <string>BurnCraft shows the camera picture of the laser bed under your design.</string>
</dict>
</plist>
PLIST

# A local ("ad-hoc") signature, so macOS runs the app and remembers its camera permission.
codesign --force --deep --sign - "$APP"

echo "Built $APP"
echo "Drag it to Applications, or run: open \"$APP\""
