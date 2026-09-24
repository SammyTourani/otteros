#!/bin/sh
# One-line verification of a pure crate in crates/ (DECISIONS D27): tests, ignored count,
# clippy with tests, the no_std build and external dependencies. Usage: scripts/verify-crate.sh <crate>
set -u
c="$1"; cd "$(dirname "$0")/../crates" || exit 2
t=$(cargo test -q -p "$c" 2>&1 | awk '/^test result/{p+=$4; f+=$6; i+=$8} END{printf "tests %d passed %d failed %d ignored", p, f, i}')
cargo clippy -q -p "$c" --tests -- -D warnings >/dev/null 2>&1 && l=ok || l=FAIL
cargo build -q -p "$c" --target x86_64-unknown-none >/dev/null 2>&1 && n=ok || n=FAIL
ext=$(awk '/^\[(dev-)?dependencies\]/{s=1; next} /^\[/{s=0} s && NF && $0 !~ /path *=/' "$c/Cargo.toml" | wc -l | tr -d ' ')
echo "$c: $t | clippy $l | no_std $n | external deps $ext"
