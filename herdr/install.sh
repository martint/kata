#!/bin/sh
# Installs the kata mod into Herdr: a link to this checkout by default (it follows the repo), or a copy with --copy.
# Herdr loads it on its next start, or right away if it's running.
set -eu

source_dir="$(cd "$(dirname "$0")/kata" && pwd)"
home="${HERDR_APP_HOME:-$HOME/.config/herdr-app}"
target="$home/mods/kata"

mode=link
case "${1:-}" in
    --copy) mode=copy ;;
    "") ;;
    *) echo "usage: $0 [--copy]" >&2; exit 2 ;;
esac

mkdir -p "$home/mods"
if [ -e "$target" ] && [ ! -L "$target" ]; then
    if [ "$mode" = link ]; then
        echo "$target is a copy; remove it first, or run with --copy to replace it" >&2
        exit 1
    fi
    rm -rf "$target"
fi

if [ "$mode" = copy ]; then
    rm -f "$target"
    mkdir -p "$target"
    (cd "$source_dir" && tar cf - --exclude jsconfig.json --exclude gpui-kit.d.ts .) | (cd "$target" && tar xf -)
    echo "copied the kata mod to $target"
else
    ln -sfn "$source_dir" "$target"
    # Herdr doesn't notice changes behind a link; touching the link itself reloads a running window.
    touch -h "$target"
    echo "linked $target -> $source_dir"
fi

echo "The mod talks to kata at http://127.0.0.1:7878. For another address, write"
echo "  {\"url\": \"http://host:port\"}"
echo "to $home/data/kata/config.json."
