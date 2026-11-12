#!/usr/bin/env sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Eric Allen
#
# Optional helper: installs Seeker for the current user (no root needed).
# The binary also runs fine on its own from any folder.
#   ~/.local/bin/seeker
#   ~/.local/share/applications/io.github.ElectricStars.Seeker.desktop
#   ~/.local/share/icons/hicolor/scalable/apps/io.github.ElectricStars.Seeker.svg
#   ~/.local/share/metainfo/io.github.ElectricStars.Seeker.metainfo.xml
# Usage: ./install.sh            install / update
#        ./install.sh --uninstall
set -eu

APP=seeker                          # binary name
ID=io.github.ElectricStars.Seeker   # desktop / icon / AppStream id
HERE=$(cd "$(dirname "$0")" && pwd)
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
APPS_DIR="$DATA_DIR/applications"
ICON_DIR="$DATA_DIR/icons/hicolor/scalable/apps"
META_DIR="$DATA_DIR/metainfo"

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS_DIR" >/dev/null 2>&1 || true
    touch "$DATA_DIR/icons/hicolor" 2>/dev/null || true
}

# Files installed by 0.1.0, which used the short name.
remove_legacy() {
    rm -f "$APPS_DIR/$APP.desktop" "$ICON_DIR/$APP.svg"
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN_DIR/$APP" "$APPS_DIR/$ID.desktop" "$ICON_DIR/$ID.svg" "$META_DIR/$ID.metainfo.xml"
    remove_legacy
    refresh
    echo "Removed $APP. Settings remain in $DATA_DIR/$APP (delete it if you like)."
    exit 0
fi

# Find the binary: release tarball layout first, then a source checkout.
for cand in "$HERE/$APP" "$HERE/target/release/$APP" \
            "$HERE/target/x86_64-unknown-linux-gnu/release/$APP"; do
    if [ -f "$cand" ] && [ -x "$cand" ]; then BIN="$cand"; break; fi
done
if [ -z "${BIN:-}" ]; then
    echo "Could not find the $APP binary. Build it first:  cargo build --release" >&2
    exit 1
fi

# Data files live next to this script (tarball) or in data/ (source checkout).
DATA="$HERE"
[ -f "$DATA/$ID.desktop" ] || DATA="$HERE/data"

mkdir -p "$BIN_DIR" "$APPS_DIR" "$ICON_DIR" "$META_DIR"
remove_legacy
install -m 755 "$BIN" "$BIN_DIR/$APP"
install -m 644 "$DATA/$ID.svg" "$ICON_DIR/$ID.svg"
[ -f "$DATA/$ID.metainfo.xml" ] && install -m 644 "$DATA/$ID.metainfo.xml" "$META_DIR/$ID.metainfo.xml"
# Use the absolute path so the menu entry works even if ~/.local/bin is not on PATH.
sed "s|^Exec=$APP |Exec=$BIN_DIR/$APP |" "$DATA/$ID.desktop" > "$APPS_DIR/$ID.desktop"
chmod 644 "$APPS_DIR/$ID.desktop"
refresh

echo "Installed $APP to $BIN_DIR/$APP"
echo "It should appear in your applications menu as \"$(sed -n 's/^Name=//p' "$DATA/$ID.desktop")\"."
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: $BIN_DIR is not on your PATH; add it to run '$APP' from a terminal." ;;
esac
