#!/usr/bin/env bash
# OtterOS milestone gate (brief M1-T7): runs every acceptance target for
# the current milestone in sequence, prints a one-line PASS/FAIL table,
# and exits non-zero if any of them failed.
#
# Every target is run to completion regardless of an earlier failure
# (this deliberately does *not* use `set -e`/`make -k`): a later target's
# result still matters even if an earlier one failed, and the table at the
# end should show the true state of every target, not just the first one
# that broke.

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

TARGETS=(
    build
    lint
    test
    bios-test
    fault-test
    df-test
    stackoverflow-test
    thread-stackoverflow-test
    pmm-fault-tests
    heap-fault-tests
    panic-test
    shot
)

results=()
overall=0

for target in "${TARGETS[@]}"; do
    echo "[check] ---- gmake ${target} ----"
    if gmake "$target"; then
        results+=("PASS  ${target}")
    else
        results+=("FAIL  ${target}")
        overall=1
    fi
done

echo
echo "[check] ---- milestone gate results ----"
for row in "${results[@]}"; do
    echo "[check] ${row}"
done

exit "$overall"
