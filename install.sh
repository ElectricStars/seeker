#!/usr/bin/env sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Eric Allen
# Installs Seeker for the current user (no root needed):
#   ~/.local/bin/seeker
#   ~/.local/share/applications/seeker.desktop
#   ~/.local/share/icons/hicolor/scalable/apps/seeker.svg
# Usage: ./install.sh            install / update
#        ./install.sh --uninstall
set -eu

APP=seeker
HERE=$(cd "$(dirname "$0")" && pwd)
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
APPS_DIR="$DATA_DIR/applications"
ICON_DIR="$DATA_DIR/icons/hicolor/scalable/apps"

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS_DIR" >/dev/null 2>&1 || true
    touch "$DATA_DIR/icons/hicolor" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN_DIR/$APP" "$APPS_DIR/$APP.desktop" "$ICON_DIR/$APP.svg"
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

# Assets live next to this script (tarball) or in assets/ (source checkout).
ASSETS="$HERE"
[ -f "$ASSETS/$APP.desktop" ] || ASSETS="$HERE/assets"

mkdir -p "$BIN_DIR" "$APPS_DIR" "$ICON_DIR"
install -m 755 "$BIN" "$BIN_DIR/$APP"
install -m 644 "$ASSETS/$APP.svg" "$ICON_DIR/$APP.svg"
# Use the absolute path so the menu entry works even if ~/.local/bin is not on PATH.
sed "s|^Exec=$APP |Exec=$BIN_DIR/$APP |" "$ASSETS/$APP.desktop" > "$APPS_DIR/$APP.desktop"
chmod 644 "$APPS_DIR/$APP.desktop"
refresh

echo "Installed $APP to $BIN_DIR/$APP"
echo "It should appear in your applications menu as \"$(sed -n 's/^Name=//p' "$ASSETS/$APP.desktop")\"."
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: $BIN_DIR is not on your PATH; add it to run '$APP' from a terminal." ;;
esac
