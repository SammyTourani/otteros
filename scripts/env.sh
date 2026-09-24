#!/usr/bin/env bash
# Environment for OtterOS build/test/run commands on this Mac Mini M4
# (macOS 26.5, arm64). Usage: source scripts/env.sh
set -a
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
set +a

# SSD-first (DECISIONS D28): on a machine with the external SSD mounted, bulky build
# outputs live there. Set OTTEROS_NO_SSD=1 to opt out; other machines are unaffected.
if [ -d /Volumes/SammyDisk ] && [ -z "$OTTEROS_NO_SSD" ]; then
  export OTTEROS_BUILD_ROOT=/Volumes/SammyDisk/build/otteros
  export OTTEROS_MODEL_DIR=/Volumes/SammyDisk/models/otteros
  OTTEROS_REPO_ROOT=${OTTEROS_REPO_ROOT:-$(git rev-parse --show-toplevel 2>/dev/null)}
  [ -n "$OTTEROS_REPO_ROOT" ] && (cd "$OTTEROS_REPO_ROOT" && sh scripts/ssd-links.sh)
fi
