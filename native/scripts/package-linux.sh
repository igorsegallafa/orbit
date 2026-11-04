#!/usr/bin/env bash
# Builds Orbit and packages it for Linux, into native/dist/:
#   Orbit-<version>-linux-<arch>.tar.gz   always (bin/, share/, install.sh)
#   Orbit-<version>-amd64.deb             always (package `orbit`)
#   Orbit-<version>-1.<arch>.rpm          when rpmbuild is found
#   Orbit-<version>-<arch>.AppImage       when appimagetool is found ($APPIMAGETOOL or PATH)
# The version is ORBIT_VERSION (a release sets it from the tag), else native/Cargo.toml's;
# build.rs bakes the same value into the binary.
#   scripts/package-linux.sh [--no-build]
set -euo pipefail
native=$(cd "$(dirname "$0")/.." && pwd)
icons=$native/packaging/icons
version=${ORBIT_VERSION:-$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$native/Cargo.toml" | head -n1)}
arch=$(uname -m)

root=$(dirname "$native")
[[ ${1:-} == --no-build ]] || ORBIT_VERSION=$version cargo build --release --locked -p orbit-native --manifest-path "$root/Cargo.toml"
bin=$root/target/release/orbit-native
[[ -x $bin ]] || { echo "missing $bin" >&2; exit 1; }

dist=$native/dist
stage=$dist/stage
rm -rf "$stage"
mkdir -p "$stage"

# Tarball: an FHS-style prefix plus an installer for ~/.local (or any prefix).
name=Orbit-$version-linux-$arch
tree=$stage/$name
install -Dm755 "$bin" "$tree/bin/orbit"
install -Dm644 "$native/packaging/linux/orbit.desktop" "$tree/share/applications/orbit.desktop"
for s in 32 64 128 256; do
  install -Dm644 "$icons/${s}x${s}.png" "$tree/share/icons/hicolor/${s}x${s}/apps/orbit.png"
done
install -Dm644 "$icons/icon.png" "$tree/share/icons/hicolor/512x512/apps/orbit.png"
install -Dm755 "$native/packaging/linux/install.sh" "$tree/install.sh"
tar -C "$stage" -czf "$dist/$name.tar.gz" "$name"
echo "$dist/$name.tar.gz"

# .deb and .rpm: package `orbit`, which upgrades the Tauri app's package of
# that name. /usr/bin/Orbit (the Tauri binary's path) stays as a link.
sysroot=$stage/sysroot
mkdir -p "$sysroot/usr"
cp -a "$tree/bin" "$tree/share" "$sysroot/usr/"
sed -i 's|^Exec=.*|Exec=/usr/bin/orbit|' "$sysroot/usr/share/applications/orbit.desktop"
ln -s orbit "$sysroot/usr/bin/Orbit"
description="A desktop workspace for shipping features across repositories with AI agents"
case $arch in x86_64) debarch=amd64 ;; aarch64) debarch=arm64 ;; *) debarch=$arch ;; esac

# Built with ar and tar (the .deb format), so it needs no dpkg.
debdir=$stage/deb
mkdir -p "$debdir/control"
cat >"$debdir/control/control" <<EOF
Package: orbit
Version: $version
Architecture: $debarch
Maintainer: Igor Segalla
Installed-Size: $(du -sk "$sysroot" | cut -f1)
Depends: libc6, libgcc-s1, libxcb1, libxkbcommon0, libxkbcommon-x11-0, libvulkan1
Section: devel
Priority: optional
Description: $description
EOF
echo "2.0" >"$debdir/debian-binary"
tar --owner=0 --group=0 -C "$debdir/control" -czf "$debdir/control.tar.gz" ./control
tar --owner=0 --group=0 -C "$sysroot" -czf "$debdir/data.tar.gz" ./usr
deb=$dist/Orbit-$version-$debarch.deb
rm -f "$deb"
(cd "$debdir" && ar rc "$deb" debian-binary control.tar.gz data.tar.gz)
echo "$deb"

if command -v rpmbuild >/dev/null; then
  rpmtop=$stage/rpm
  mkdir -p "$rpmtop"/{BUILD,RPMS,SPECS,BUILDROOT}
  cat >"$rpmtop/SPECS/orbit.spec" <<EOF
Name: orbit
Version: ${version//-/_}
Release: 1
Summary: $description
License: MIT
AutoReqProv: yes

%description
$description

%install
mkdir -p %{buildroot}
cp -a "$sysroot/usr" %{buildroot}/

%files
/usr/bin/orbit
/usr/bin/Orbit
/usr/share/applications/orbit.desktop
/usr/share/icons/hicolor/*/apps/orbit.png
EOF
  rpmbuild --quiet --define "_topdir $rpmtop" --define "_build_id_links none" -bb "$rpmtop/SPECS/orbit.spec"
  rpm=$dist/Orbit-$version-1.$arch.rpm
  cp "$rpmtop"/RPMS/*/*.rpm "$rpm"
  echo "$rpm"
else
  echo "rpmbuild not found: skipped the .rpm" >&2
fi

# AppImage: the same tree under usr/, with AppRun, the .desktop and the icon at the root.
tool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
if [[ -z $tool ]]; then
  echo "appimagetool not found: skipped the AppImage (set APPIMAGETOOL to build it)" >&2
  exit 0
fi
appdir=$stage/Orbit.AppDir
mkdir -p "$appdir/usr"
cp -a "$tree/bin" "$tree/share" "$appdir/usr/"
cp "$tree/share/applications/orbit.desktop" "$appdir/"
cp "$icons/icon.png" "$appdir/orbit.png"
ln -s usr/bin/orbit "$appdir/AppRun"
image=$dist/Orbit-$version-$arch.AppImage
ARCH=$arch "$tool" --no-appstream "$appdir" "$image"
echo "$image"
