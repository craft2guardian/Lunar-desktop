#!/bin/sh
# Installs (or removes) the Lunar session so it shows up at the login screen.
#
#   cargo build --release
#   sudo session/install.sh [--prefix /usr/local]      install
#   sudo session/install.sh --uninstall                remove
set -eu

prefix=/usr/local
sessions_dir=${LUNAR_SESSIONS_DIR:-/usr/share/wayland-sessions}
uninstall=0
while [ $# -gt 0 ]; do
    case $1 in
        --prefix) prefix=$2; shift 2 ;;
        --uninstall) uninstall=1; shift ;;
        *) echo "unknown option: $1" >&2; exit 1 ;;
    esac
done

here=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

if [ "$uninstall" -eq 1 ]; then
    rm -f "$prefix/bin/lunar-desktop" "$prefix/bin/lunar-session" \
        "$prefix/bin/lunar-session-start" "$sessions_dir/lunar.desktop"
    rm -rf "$prefix/share/lunar-desktop"
    echo "Lunar session removed."
    exit 0
fi

binary=$here/target/release/lunar-desktop
if [ ! -x "$binary" ]; then
    echo "error: $binary not found; run 'cargo build --release' first" >&2
    exit 1
fi

install -Dm755 "$binary" "$prefix/bin/lunar-desktop"
install -Dm755 "$here/session/lunar-session" "$prefix/bin/lunar-session"
install -Dm755 "$here/session/lunar-session-start" "$prefix/bin/lunar-session-start"
install -Dm644 "$here/config/dock.toml" "$prefix/share/lunar-desktop/config/dock.toml"
mkdir -p "$prefix/share/lunar-desktop/assets"
cp -R "$here/assets/dock-icons" "$prefix/share/lunar-desktop/assets/"

mkdir -p "$sessions_dir"
sed "s|@BINDIR@|$prefix/bin|" "$here/session/lunar.desktop.in" > "$sessions_dir/lunar.desktop"
chmod 644 "$sessions_dir/lunar.desktop"

echo "Installed. Log out and choose \"Lunar\" in the session menu at the login screen."
echo "To go back, choose \"Plasma\" there. Session log: ~/.local/state/lunar-desktop/session.log"
