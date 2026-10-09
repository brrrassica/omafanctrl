#!/usr/bin/env bash
#
# Build an AppImage bundling the omafanctrl GUI and CLI clients.
#
# The AppImage contains the *clients* only (omafanctrl-gui, omafanctrl, and
# omafanctrl-status). The privileged daemon (omafanctrld) must be installed as a
# system service separately — see docs/install.md. The clients talk to it over
# the D-Bus system bus.
#
# Requirements: cargo, curl, and a working GTK4/libadwaita toolchain.
#
# Usage:
#   packaging/appimage/build.sh
#
# The resulting AppImage is written to the repository root.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

appdir="$root/build/AppDir"
tools="$root/build/tools"
mkdir -p "$tools"

echo "==> Building release binaries"
cargo build --release --locked --workspace

echo "==> Assembling AppDir"
rm -rf "$appdir"
mkdir -p \
  "$appdir/usr/bin" \
  "$appdir/usr/share/applications" \
  "$appdir/usr/share/icons/hicolor/scalable/apps"

install -Dm755 target/release/omafanctrl-gui "$appdir/usr/bin/omafanctrl-gui"
install -Dm755 target/release/omafanctrl "$appdir/usr/bin/omafanctrl"
install -Dm755 target/release/omafanctrl-status "$appdir/usr/bin/omafanctrl-status"
install -Dm644 data/desktop/org.omarchy.omafanctrl.desktop \
  "$appdir/usr/share/applications/org.omarchy.omafanctrl.desktop"
install -Dm644 data/icons/org.omarchy.omafanctrl.svg \
  "$appdir/usr/share/icons/hicolor/scalable/apps/org.omarchy.omafanctrl.svg"

echo "==> Fetching linuxdeploy"
if [ ! -x "$tools/linuxdeploy-x86_64.AppImage" ]; then
  curl -L -o "$tools/linuxdeploy-x86_64.AppImage" \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
  chmod +x "$tools/linuxdeploy-x86_64.AppImage"
fi
if [ ! -x "$tools/linuxdeploy-plugin-gtk.sh" ]; then
  curl -L -o "$tools/linuxdeploy-plugin-gtk.sh" \
    https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh
  chmod +x "$tools/linuxdeploy-plugin-gtk.sh"
fi

echo "==> Bundling GTK4 and producing the AppImage"
export PATH="$tools:$PATH"
export DEPLOY_GTK_VERSION=4
export LINUXDEPLOY_PLUGIN_GTK="$tools/linuxdeploy-plugin-gtk.sh"

"$tools/linuxdeploy-x86_64.AppImage" \
  --appdir "$appdir" \
  --plugin gtk \
  --desktop-file "$appdir/usr/share/applications/org.omarchy.omafanctrl.desktop" \
  --icon-file "$appdir/usr/share/icons/hicolor/scalable/apps/org.omarchy.omafanctrl.svg" \
  --output appimage

echo "==> Done"
ls -1 "$root"/omafanctrl*.AppImage 2>/dev/null || true