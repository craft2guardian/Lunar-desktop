#!/bin/sh
# Lets lunar-desktop capture window thumbnails for the Dock's minimised-window
# tiles. KWin's screenshot interface (org.kde.KWin.ScreenShot2) only answers
# programs whose desktop file asks for it, and finds that file from the path
# of the running executable, so pass the binary you actually run.
#
#   scripts/install-thumbnail-permission.sh [path/to/lunar-desktop]
#
# Afterwards restart the shell. Without this the Dock still works and shows a
# placeholder card instead of a real thumbnail.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
binary=${1:-$here/target/release/lunar-desktop}
if [ ! -x "$binary" ]; then
    echo "error: $binary is not an executable; build first (cargo build --release) or pass the path" >&2
    exit 1
fi
binary=$(readlink -f -- "$binary")

dir=${XDG_DATA_HOME:-$HOME/.local/share}/applications
mkdir -p "$dir"
cat > "$dir/dev.lunar.Desktop.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Lunar Desktop
Exec=$binary
NoDisplay=true
X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2
DESKTOP
echo "wrote $dir/dev.lunar.Desktop.desktop"

for tool in kbuildsycoca6 kbuildsycoca5; do
    if command -v "$tool" >/dev/null 2>&1; then
        "$tool" >/dev/null 2>&1 || true
        break
    fi
done
echo "done: restart lunar-desktop to apply"
