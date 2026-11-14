#!/usr/bin/env bash
# Try lunar-desktop without leaving your current session.
#
# Opens a nested Sway compositor in a window (works from GNOME, KDE, X11, ...)
# and starts the shell inside it. Close the window, or press Super+Shift+E, to quit.
#
# Needs: sway, plus the normal build dependencies (see README.md).
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v sway >/dev/null; then
    echo "run-nested.sh: 'sway' is not installed (e.g. sudo apt install sway)" >&2
    exit 1
fi

cargo build --release

config="$(mktemp)"
trap 'rm -f "$config"' EXIT
echo "exec \"$PWD/target/release/lunar-desktop\"" > "$config"

# With no real display of its own, Sway nests itself in the current session.
exec sway --config "$config"
