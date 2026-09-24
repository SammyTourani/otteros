#!/usr/bin/env bash
# Fetches the JSONTestSuite parsing corpus (github.com/nst/JSONTestSuite,
# brief M7-T1) at a pinned commit, verified by SHA-256, into
# third_party/JSONTestSuite/test_parsing/ -- not committed, like
# scripts/fetch-fonts.sh's fonts (third_party/ is gitignored).
# otter-json's tests/json_test_suite.rs needs this to have been run first
# and fails loudly (not silently) if it hasn't.
#
# Usage: scripts/fetch-jsontestsuite.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/third_party/JSONTestSuite"
CACHE="$ROOT/build/jsontestsuite-download"
mkdir -p "$CACHE"

COMMIT="1ef36fa01286573e846ac449e8683f8833c5b26a"
TARBALL_URL="https://codeload.github.com/nst/JSONTestSuite/tar.gz/$COMMIT"
TARBALL_SHA256="32ab2fb37cc26679dbd9164dbb7053bcef10134643d53cb0cef45de7680dcdcc"
TARBALL="$CACHE/JSONTestSuite-$COMMIT.tar.gz"

sha256_of() {
  shasum -a 256 "$1" | awk '{print $1}'
}

if [ -f "$TARBALL" ] && [ "$(sha256_of "$TARBALL")" = "$TARBALL_SHA256" ]; then
  echo "fetch-jsontestsuite: using cached $(basename "$TARBALL")"
else
  echo "fetch-jsontestsuite: downloading JSONTestSuite@$COMMIT"
  curl -sL --fail --max-time 60 -o "$TARBALL" "$TARBALL_URL"
  actual="$(sha256_of "$TARBALL")"
  if [ "$actual" != "$TARBALL_SHA256" ]; then
    echo "fetch-jsontestsuite: SHA-256 mismatch for $TARBALL" >&2
    echo "  expected $TARBALL_SHA256" >&2
    echo "  actual   $actual" >&2
    exit 1
  fi
fi

rm -rf "$DEST"
mkdir -p "$DEST"
tar xzf "$TARBALL" -C "$CACHE"
cp -R "$CACHE/JSONTestSuite-$COMMIT/test_parsing" "$DEST/test_parsing"
cp "$CACHE/JSONTestSuite-$COMMIT/LICENSE" "$DEST/LICENSE"

n=$(find "$DEST/test_parsing" -name '*.json' | wc -l | tr -d ' ')
echo "fetch-jsontestsuite: OK -> $DEST/test_parsing ($n files)"
