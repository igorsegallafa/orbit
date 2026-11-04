#!/usr/bin/env bash
# Builds orbit-native and packages it for Linux, into native/dist/:
#   orbit-native-<version>-linux-<arch>.tar.gz   always (bin/, share/, install.sh)
#   orbit-native-<version>-<arch>.AppImage       when appimagetool is found ($APPIMAGETOOL or PATH)
# The version is the root package.json's (the Tauri app's), unless ORBIT_VERSION is set.
#   scripts/package-linux.sh [--no-build]
set -euo pipefail
native=$(cd "$(dirname "$0")/.." && pwd)
root=$(dirname "$native")
icons=$root/src-tauri/icons
version=${ORBIT_VERSION:-$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$root/package.json" | head -n1)}
arch=$(uname -m)

[[ ${1:-} == --no-build ]] || cargo build --release --locked --manifest-path "$native/Cargo.toml"
bin=$native/target/release/orbit-native
[[ -x $bin ]] || { echo "missing $bin" >&2; exit 1; }

dist=$native/dist
stage=$dist/stage
rm -rf "$stage"
mkdir -p "$stage"

# Tarball: an FHS-style prefix plus an installer for ~/.local (or any prefix).
name=orbit-native-$version-linux-$arch
tree=$stage/$name
install -Dm755 "$bin" "$tree/bin/orbit-native"
install -Dm644 "$native/packaging/linux/orbit-native.desktop" "$tree/share/applications/orbit-native.desktop"
for s in 32 64 128 256; do
  install -Dm644 "$icons/${s}x${s}.png" "$tree/share/icons/hicolor/${s}x${s}/apps/orbit-native.png"
done
install -Dm644 "$icons/icon.png" "$tree/share/icons/hicolor/512x512/apps/orbit-native.png"
install -Dm755 "$native/packaging/linux/install.sh" "$tree/install.sh"
tar -C "$stage" -czf "$dist/$name.tar.gz" "$name"
echo "$dist/$name.tar.gz"

# AppImage: the same tree under usr/, with AppRun, the .desktop and the icon at the root.
tool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
if [[ -z $tool ]]; then
  echo "appimagetool not found: skipped the AppImage (set APPIMAGETOOL to build it)" >&2
  exit 0
fi
appdir=$stage/Orbit.AppDir
mkdir -p "$appdir/usr"
cp -a "$tree/bin" "$tree/share" "$appdir/usr/"
cp "$tree/share/applications/orbit-native.desktop" "$appdir/"
cp "$icons/icon.png" "$appdir/orbit-native.png"
ln -s usr/bin/orbit-native "$appdir/AppRun"
image=$dist/orbit-native-$version-$arch.AppImage
ARCH=$arch "$tool" --no-appstream "$appdir" "$image"
echo "$image"
