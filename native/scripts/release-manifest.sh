#!/usr/bin/env bash
# Writes the updater's latest.json (the Tauri updater's format) for the signed
# files in <dir>; each file needs its <file>.sig (tools/sign).
#   scripts/release-manifest.sh <version> <dir> <download base url> [notes] > latest.json
# Keys: <os>-<arch>-<kind> is what Orbit reads (appimage, tar, zip, app); the
# bare <os>-<arch> and -nsis keys are what an installed Tauri Orbit reads.
set -euo pipefail
version=${1#v}
dir=$2
base=${3%/}
notes=${4:-}

entries=()
add() { # <file> <key>...
  local file=$1
  shift
  [[ -f $dir/$file && -f $dir/$file.sig ]] || { echo "missing $dir/$file or its .sig" >&2; exit 1; }
  local sig
  sig=$(<"$dir/$file.sig")
  for key in "$@"; do
    entries+=("$(jq -n --arg k "$key" --arg u "$base/$file" --arg s "$sig" '{($k): {url: $u, signature: $s}}')")
  done
}
add "Orbit-$version-x86_64.AppImage" linux-x86_64 linux-x86_64-appimage
add "Orbit-$version-linux-x86_64.tar.gz" linux-x86_64-tar
add "Orbit-$version-windows-x86_64-setup.exe" windows-x86_64 windows-x86_64-nsis
add "Orbit-$version-windows-x86_64.zip" windows-x86_64-zip
add "Orbit-$version-macos-aarch64.app.tar.gz" darwin-aarch64 darwin-aarch64-app

printf '%s\n' "${entries[@]}" | jq -s --arg v "$version" --arg n "$notes" --arg d "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '{version: $v, notes: $n, pub_date: $d, platforms: add}'
