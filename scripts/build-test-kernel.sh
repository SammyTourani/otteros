#!/usr/bin/env bash
# Builds the otteros-kernel-test binary -- a real `cargo test --no-run`
# build, so `custom_test_frameworks` harness generation actually happens
# (see kernel/src/test_main.rs's module docs for why that requires a
# genuine `--test` compilation) -- and copies the resulting ELF to a
# stable path: build/bin/otteros-kernel-test.
#
# `cargo test` doesn't support --artifact-dir/--out-dir (only `cargo build`
# does), and its own output lives under a build-script-hashed directory
# that changes across rebuilds, so we discover the real path once per
# build via --message-format=json instead of hardcoding it.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../kernel"

json=$(cargo test --bin otteros-kernel-test --no-run --message-format=json)

exe=$(printf '%s\n' "$json" \
    | jq -r 'select(.reason == "compiler-artifact" and .target.name == "otteros-kernel-test") | .executable // empty' \
    | tail -1)

if [[ -z "$exe" ]]; then
    echo "error: could not find the otteros-kernel-test executable in cargo's build output" >&2
    printf '%s\n' "$json" | tail -20 >&2
    exit 1
fi

mkdir -p ../build/bin
cp "$exe" ../build/bin/otteros-kernel-test
