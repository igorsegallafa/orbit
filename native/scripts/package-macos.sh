#!/usr/bin/env bash
# Packages a release build of Orbit for macOS, into native/dist/:
#   Orbit-<version>-macos-<arch>.app.tar.gz   Orbit.app: the in-app updater's download
#   Orbit-<version>-macos-<arch>.dmg          for a first install
# The bundle is ad-hoc signed (no Developer ID yet): the first launch needs
# right-click → Open (or "Open Anyway"); updates don't.
# The version is ORBIT_VERSION, else native/Cargo.toml's.
#   scripts/package-macos.sh   (after `cargo build --release -p orbit-native` at the root)
set -euo pipefail
native=$(cd "$(dirname "$0")/.." && pwd)
version=${ORBIT_VERSION:-$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$native/Cargo.toml" | head -n1)}
arch=$(uname -m)
[[ $arch == arm64 ]] && arch=aarch64
bin=$(dirname "$native")/target/release/orbit-native
[[ -x $bin ]] || { echo "missing $bin" >&2; exit 1; }

dist=$native/dist
stage=$dist/stage
rm -rf "$stage"
app=$stage/Orbit.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/Orbit"
cp "$native/packaging/icons/icon.icns" "$app/Contents/Resources/icon.icns"
cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Orbit</string>
  <key>CFBundleDisplayName</key><string>Orbit</string>
  <key>CFBundleIdentifier</key><string>com.nsx001220.orbit</string>
  <key>CFBundleExecutable</key><string>Orbit</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
codesign --force --deep --sign - "$app"

tarball=$dist/Orbit-$version-macos-$arch.app.tar.gz
COPYFILE_DISABLE=1 tar -C "$stage" -czf "$tarball" Orbit.app
echo "$tarball"

dmgdir=$stage/dmg
mkdir -p "$dmgdir"
cp -R "$app" "$dmgdir/"
ln -s /Applications "$dmgdir/Applications"
dmg=$dist/Orbit-$version-macos-$arch.dmg
hdiutil create -volname Orbit -srcfolder "$dmgdir" -ov -format UDZO "$dmg" >/dev/null
echo "$dmg"
