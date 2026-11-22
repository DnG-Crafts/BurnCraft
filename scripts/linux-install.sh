#!/bin/sh
# Installs BurnCraft for the current user on Linux: the program, a menu entry and its icon, so it
# opens from the applications menu and shows its own icon in the dock or task bar (on Wayland as
# well as X11).
#
#   sh scripts/linux-install.sh            build and install
#   sh scripts/linux-install.sh --remove   take it out again
#
# Nothing is installed system-wide: the files go under ~/.local.
set -e
cd "$(dirname "$0")/.."

# The app id BurnCraft gives its window: the menu entry must be named after it for the desktop to
# match the window to its icon.
APP_ID=org.burncraft.BurnCraft
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
BIN="$HOME/.local/bin"
DESKTOP="$DATA/applications/$APP_ID.desktop"
ICON="$DATA/icons/hicolor/256x256/apps/$APP_ID.png"

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DATA/applications" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" || true
}

if [ "$1" = "--remove" ]; then
    rm -f "$BIN/burncraft" "$DESKTOP" "$ICON"
    refresh
    echo "BurnCraft removed (your settings in $DATA/burncraft were kept)."
    exit 0
fi

cargo build --release -p burncraft

mkdir -p "$BIN" "$(dirname "$DESKTOP")" "$(dirname "$ICON")"
cp target/release/burncraft "$BIN/burncraft"
cp assets/icon.png "$ICON"

cat > "$DESKTOP" <<DESKTOP_ENTRY
[Desktop Entry]
Type=Application
Name=BurnCraft
GenericName=Laser Cutter Software
Comment=Design and send jobs to laser cutters and engravers
Exec=$BIN/burncraft %F
Icon=$APP_ID
Terminal=false
Categories=Graphics;VectorGraphics;Engineering;
Keywords=laser;engraver;cutter;cnc;grbl;ruida;galvo;
StartupWMClass=$APP_ID
DESKTOP_ENTRY

refresh
echo "BurnCraft installed: $BIN/burncraft, with a menu entry and icon."
echo "If the menu doesn't show it yet, log out and back in."
