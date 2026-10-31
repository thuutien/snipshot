#!/bin/sh
# Installs Snipshot for the current user: the binary, the app icon and an
# app-menu entry. Run it from an unpacked release, or from the project folder
# after `cargo build --release`.
set -eu
cd "$(dirname "$0")/.."

bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"

# From a source checkout, or from an unpacked release archive.
if [ -f target/release/snipshot ]; then src=target/release/snipshot; else src=snipshot; fi
install -Dm755 "$src" "$bin/snipshot"
install -Dm644 assets/snipshot.svg "$data/icons/hicolor/scalable/apps/snipshot.svg"
install -Dm644 assets/snipshot-256.png "$data/icons/hicolor/256x256/apps/snipshot.png"
# Point the launcher at the installed binary so it works even when
# ~/.local/bin isn't on the PATH of the desktop session.
mkdir -p "$data/applications"
sed "s|^Exec=snipshot|Exec=$bin/snipshot|" packaging/snipshot.desktop > "$data/applications/snipshot.desktop"
chmod 644 "$data/applications/snipshot.desktop"

gtk-update-icon-cache -q "$data/icons/hicolor" 2>/dev/null || true
update-desktop-database -q "$data/applications" 2>/dev/null || true

echo "Installed Snipshot to $bin/snipshot"
case ":$PATH:" in
  *":$bin:"*) ;;
  *) echo "Note: $bin is not on your PATH; add it to run 'snipshot' from a terminal." ;;
esac
