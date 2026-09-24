#!/bin/sh
# Keep bulky, rebuildable outputs on the external SSD when one is configured (DECISIONS D28).
# Idempotent; sourced via scripts/env.sh. Does nothing unless OTTEROS_BUILD_ROOT is set.
[ -n "$OTTEROS_BUILD_ROOT" ] || return 0 2>/dev/null || exit 0
repo=$(cd "$(dirname "$0")/.." 2>/dev/null && pwd)
[ -f "$repo/GNUmakefile" ] || repo=$(pwd)
for pair in build:build kernel/target:kernel-target user/target:user-target crates/target:crates-target tools/otter-convert/target:otter-convert-target third_party:third_party; do
  src="$repo/${pair%%:*}"; dst="$OTTEROS_BUILD_ROOT/${pair##*:}"
  mkdir -p "$dst"
  if [ -L "$src" ]; then continue; fi
  if [ -d "$src" ]; then
    # Move existing contents onto the SSD when the destination is empty, else drop the rebuildable copy.
    if [ -z "$(ls -A "$dst" 2>/dev/null)" ]; then cp -R "$src"/. "$dst"/ && rm -rf "$src"; else rm -rf "$src"; fi
  fi
  ln -s "$dst" "$src"
done
