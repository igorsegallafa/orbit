#!/usr/bin/env bash
# Packages a release build of Orbit for Windows (Git Bash), into native/dist/:
#   Orbit-<version>-windows-<arch>-setup.exe   per-user NSIS installer (needs makensis)
#   Orbit-<version>-windows-<arch>.zip         Orbit.exe alone: the in-app updater's download
# The version is ORBIT_VERSION, else native/Cargo.toml's.
#   scripts/package-windows.sh   (after cargo build --release)
set -euo pipefail
native=$(cd "$(dirname "$0")/.." && pwd)
version=${ORBIT_VERSION:-$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$native/Cargo.toml" | head -n1)}
arch=x86_64
bin=$native/target/release/orbit-native.exe
[[ -f $bin ]] || { echo "missing $bin" >&2; exit 1; }

dist=$native/dist
stage=$dist/stage
rm -rf "$stage"
mkdir -p "$stage"
cp "$bin" "$stage/Orbit.exe"

zip=$dist/Orbit-$version-windows-$arch.zip
rm -f "$zip"
(cd "$stage" && 7z a -tzip -bd "$zip" Orbit.exe >/dev/null)
echo "$zip"

makensis=$(command -v makensis || echo "/c/Program Files (x86)/NSIS/makensis.exe")
setup=$dist/Orbit-$version-windows-$arch-setup.exe
"$makensis" -V2 -DVERSION="$version" -DEXE="$(cygpath -w "$stage/Orbit.exe")" \
  -DICON="$(cygpath -w "$native/packaging/icons/icon.ico")" -DOUTFILE="$(cygpath -w "$setup")" \
  "$(cygpath -w "$native/packaging/windows/installer.nsi")"
echo "$setup"
