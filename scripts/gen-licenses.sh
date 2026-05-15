#!/usr/bin/env bash
#
# Regenerate THIRD_PARTY_LICENSES.md from the Rust + web dependency
# trees. Run after every dep change. Requires `cargo-about` (see
# `cargo install cargo-about --features=cli`) and `bun`.
set -euo pipefail

cd "$(dirname "$0")/.."

OUT=THIRD_PARTY_LICENSES.md

{
  cargo about generate about.hbs
  printf '\n\n---\n\n'
  bun --bun scripts/web-licenses.ts
} > "$OUT"

echo "Wrote $OUT ($(wc -l < "$OUT") lines)"
