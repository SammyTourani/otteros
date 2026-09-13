#!/usr/bin/env bash
# Environment for OtterOS build/test/run commands on this Mac Mini M4
# (macOS 26.5, arm64). Usage: source scripts/env.sh
set -a
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
set +a
