#!/bin/sh
# Builds installer/Install-Lunar.sh: the launcher followed by lunar-installer.py,
# as one executable file people can download and run.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
out=${1:-$here/Install-Lunar.sh}
cat "$here/launcher-header.sh" "$here/lunar-installer.py" >"$out"
chmod 755 "$out"
echo "wrote $out"
