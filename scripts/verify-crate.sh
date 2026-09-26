#!/bin/sh
# One-line verification of a pure crate in crates/ (DECISIONS D27): tests, ignored count,
# clippy with tests (native and x86_64), the no_std build and external dependencies. Usage: scripts/verify-crate.sh <crate>
set -u
c="$1"; cd "$(dirname "$0")/../crates" || exit 2
# Tests run under the memory guard (process-group phys_footprint; cargo also launches each test
# binary through it, see crates/.cargo/config.toml): a runaway test once took this 16 GB Mac to a
# kernel panic. The model crates load the 539 MB SmolLM2 f32 model in several tests at once.
case "$c" in otter-llm|otter-local) cap_default=4096 ;; *) cap_default=2048 ;; esac
cap="${OTTEROS_MEM_CAP_MB:-$cap_default}"
out=$(mktemp)
OTTEROS_MEM_CAP_MB="$cap" ../scripts/memguard.py --cap-mb "$cap" -- cargo test -q -p "$c" > "$out" 2>&1
status=$?
t=$(awk '/^test result/{p+=$4; f+=$6; i+=$8} END{printf "tests %d passed %d failed %d ignored", p, f, i}' "$out")
if [ "$status" -ne 0 ]; then
  t="$t, cargo test FAILED (exit $status)"
  grep -m 5 -E "\[memguard\]|panicked|^error" "$out" >&2
fi
rm -f "$out"
# Lint natively and for x86_64 (Rosetta) so x86-only code paths (SIMD, AES-NI) are linted too.
cargo clippy -q -p "$c" --tests -- -D warnings >/dev/null 2>&1 &&
  cargo clippy -q -p "$c" --tests --target x86_64-apple-darwin -- -D warnings >/dev/null 2>&1 && l=ok || l=FAIL
cargo build -q -p "$c" --target x86_64-unknown-none >/dev/null 2>&1 && n=ok || n=FAIL
ext=$(awk '/^\[(dev-)?dependencies\]/{s=1; next} /^\[/{s=0} s && NF && $0 !~ /path *=/' "$c/Cargo.toml" | wc -l | tr -d ' ')
echo "$c: $t | clippy $l | no_std $n | external deps $ext"
[ "$status" -eq 0 ] && [ "$l" = ok ] && [ "$n" = ok ] && [ "$ext" = 0 ]
