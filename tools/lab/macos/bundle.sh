#!/bin/bash
# A light "Mister Download Manager.app" of a build's programs, for the macOS runner's checks and
# pictures (macos-showcase.yml): macOS names the app in its menu bar, Dock and menu bar item by
# the bundle, as it does the release's. The release's own bundle (build-macos.sh) adds the Safari
# extension, the browsers' extension files and the signatures; dmg-check.sh checks that one.
#
#   bundle.sh <folder of the build's programs> <out .app>
set -euo pipefail
bin=$1; app=$2
root="$(cd "$(dirname "$0")/../../.." && pwd)"
version="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$root/Cargo.toml" | head -1)"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
for p in misterdownloadmanager misterdownloadmanager-cli misterdownloadmanager-nmhost; do
  [ -x "$bin/$p" ] && cp "$bin/$p" "$app/Contents/MacOS/$p"
done
set_dir="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$set_dir"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$root/art/Mister Download Manager.png" --out "$set_dir/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$root/art/Mister Download Manager.png" --out "$set_dir/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$set_dir" -o "$app/Contents/Resources/AppIcon.icns"
cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Mister Download Manager</string>
  <key>CFBundleDisplayName</key><string>Mister Download Manager</string>
  <key>CFBundleIdentifier</key><string>io.github.anefzaoui.misterdownloadmanager</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>misterdownloadmanager</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
plutil -lint "$app/Contents/Info.plist" >/dev/null
codesign --force --sign - "$app" >/dev/null 2>&1 || true
echo "$app ($version)"
