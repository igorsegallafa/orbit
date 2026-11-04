#!/usr/bin/env bash
# Installs Orbit from this unpacked tarball into PREFIX (default ~/.local):
# the binary, the .desktop entry and the icons. `./install.sh --uninstall` removes them.
set -euo pipefail
src=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
files=(bin/orbit share/applications/orbit.desktop)
for s in 32 64 128 256 512; do files+=("share/icons/hicolor/${s}x${s}/apps/orbit.png"); done

if [[ ${1:-} == --uninstall ]]; then
  for f in "${files[@]}"; do rm -f "$prefix/$f"; done
  echo "removed Orbit from $prefix"
  exit 0
fi

for f in "${files[@]}"; do install -Dm"$([[ $f == bin/* ]] && echo 755 || echo 644)" "$src/$f" "$prefix/$f"; done
# Launchers may not have PREFIX/bin on PATH: point the entry at the binary.
sed -i "s|^Exec=.*|Exec=$prefix/bin/orbit|" "$prefix/share/applications/orbit.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
echo "installed Orbit in $prefix (run: $prefix/bin/orbit)"
