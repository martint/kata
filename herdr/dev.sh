#!/bin/sh
# Links herdr/kata into Herdr's mods and reloads it on save.
# Herdr doesn't see edits behind a symlinked mod folder; touching the link itself makes it reload.
set -eu

source_dir="$(cd "$(dirname "$0")/kata" && pwd)"
mods_dir="${HERDR_APP_HOME:-$HOME/.config/herdr-app}/mods"
link="$mods_dir/kata"

if [ -e "$link" ] && [ ! -L "$link" ]; then
    echo "$link exists and isn't a symlink; not touching it" >&2
    exit 1
fi
ln -sfn "$source_dir" "$link"
echo "linked $link -> $source_dir"

snapshot() {
    find "$source_dir" -type f \( -name '*.js' -o -name '*.json' -o -name '*.svg' \) -not -name jsconfig.json -exec stat -f '%m %N' {} + | sort
}

last="$(snapshot)"
while sleep 1; do
    now="$(snapshot)"
    if [ "$now" != "$last" ]; then
        last="$now"
        touch -h "$link"
        if herdr-app check "$link" >/dev/null 2>&1; then
            echo "$(date +%T) reloaded"
        else
            echo "$(date +%T) check failed:"
            herdr-app check "$link" 2>&1 | sed 's/^/  /'
        fi
    fi
done
