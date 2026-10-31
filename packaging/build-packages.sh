#!/bin/sh
# Builds the release packages into dist/:
#   snipshot-<version>-x86_64-linux.tar.gz   binary + install script
#   snipshot_<version>_amd64.deb             Debian/Ubuntu/Mint package
#   snipshot-<version>-x86_64.flatpak        Flatpak bundle (Freedesktop runtime)
#   SHA256SUMS
# Needs dpkg-deb, flatpak and the org.freedesktop.Platform//25.08 runtime:
#   flatpak install --user flathub org.freedesktop.Platform//25.08
set -eu
cd "$(dirname "$0")/.."

app_id=io.github.thuutien.Snipshot
runtime_version=25.08
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
maintainer="Bryan Tran <17092189+thuutien@users.noreply.github.com>"

cargo build --release
rm -rf dist
mkdir -p dist
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# --- tarball -----------------------------------------------------------------
name="snipshot-$version-x86_64-linux"
mkdir -p "$work/$name/packaging"
cp target/release/snipshot README.md "$work/$name/"
cp -r assets "$work/$name/"
cp packaging/install.sh packaging/snipshot.desktop "$work/$name/packaging/"
tar -C "$work" -czf "dist/$name.tar.gz" "$name"

# --- .deb --------------------------------------------------------------------
deb="$work/deb"
install -Dm755 target/release/snipshot "$deb/usr/bin/snipshot"
install -Dm644 packaging/snipshot.desktop "$deb/usr/share/applications/snipshot.desktop"
install -Dm644 assets/snipshot.svg "$deb/usr/share/icons/hicolor/scalable/apps/snipshot.svg"
install -Dm644 assets/snipshot-256.png "$deb/usr/share/icons/hicolor/256x256/apps/snipshot.png"
install -Dm644 README.md "$deb/usr/share/doc/snipshot/README.md"
mkdir -p "$deb/DEBIAN"
cat > "$deb/DEBIAN/control" <<EOF
Package: snipshot
Version: $version
Architecture: amd64
Maintainer: $maintainer
Installed-Size: $(du -sk "$deb/usr" | cut -f1)
Depends: libc6 (>= 2.39), libgcc-s1, libegl1, libx11-6, libx11-xcb1, libxcursor1, libxi6, libxkbcommon-x11-0, libwayland-client0
Recommends: xdg-desktop-portal, xdg-utils
Section: graphics
Priority: optional
Homepage: https://github.com/thuutien/snipshot
Description: Snipping tool: snip part of your screen and mark it up
 Snipshot is a snipping tool for Linux, modeled on the Windows Snipping
 Tool. Snip a rectangle, a window, a freeform shape or the whole screen,
 then mark it up with pen, highlighter, shapes and text, and copy or save it.
EOF
dpkg-deb --root-owner-group --build "$deb" "dist/snipshot_${version}_amd64.deb" >/dev/null

# --- Flatpak -----------------------------------------------------------------
fp="$work/flatpak"
flatpak build-init "$fp" "$app_id" org.freedesktop.Platform org.freedesktop.Platform "$runtime_version"
install -Dm755 target/release/snipshot "$fp/files/bin/snipshot"
sed "s/^Icon=snipshot/Icon=$app_id/" packaging/snipshot.desktop \
  > "$work/$app_id.desktop"
install -Dm644 "$work/$app_id.desktop" "$fp/files/share/applications/$app_id.desktop"
install -Dm644 assets/snipshot.svg "$fp/files/share/icons/hicolor/scalable/apps/$app_id.svg"
install -Dm644 assets/snipshot-256.png "$fp/files/share/icons/hicolor/256x256/apps/$app_id.png"
install -Dm644 "packaging/$app_id.metainfo.xml" "$fp/files/share/metainfo/$app_id.metainfo.xml"
flatpak build-finish "$fp" --command=snipshot \
  --share=ipc --share=network \
  --socket=wayland --socket=fallback-x11 --device=dri \
  --filesystem=xdg-pictures >/dev/null
flatpak build-export "$work/repo" "$fp" stable >/dev/null
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$work/repo" "dist/snipshot-$version-x86_64.flatpak" "$app_id" stable

(cd dist && sha256sum -- * > SHA256SUMS)
ls -l dist
