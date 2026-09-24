#!/usr/bin/env bash
# Generates the PEM cert/key bundles otter-tls's local interop tests hand to
# `openssl s_server` (which wants PEM, not the DER `otter-x509` fixtures are
# committed as) -- reusing brief M8-T4's test PKI (scripts/make-test-pki.sh)
# rather than minting a second one, per brief M8-T5. Committed once (like
# make-test-pki.sh's own output), not regenerated on every test run.
#
# Usage: scripts/tls-gen-server-pem.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKI="$ROOT/crates/otter-x509/tests/fixtures/pki"
OUT="$ROOT/crates/otter-tls/tests/fixtures/server"
OPENSSL=/opt/homebrew/bin/openssl

if [ ! -f "$PKI/leaf_exact.der" ]; then
    echo "tls-gen-server-pem: $PKI/leaf_exact.der not found; run scripts/make-test-pki.sh first" >&2
    exit 1
fi

mkdir -p "$OUT"

der_to_pem() {
    "$OPENSSL" x509 -inform der -in "$PKI/$1.der" -outform pem
}

echo "tls-gen-server-pem: intermediate.pem (shared by both leaves below)"
der_to_pem intermediate_pathlen0 >"$OUT/intermediate.pem"

echo "tls-gen-server-pem: leaf.pem/leaf.key (leaf_exact, s_server's own cert)"
der_to_pem leaf_exact >"$OUT/leaf.pem"
cp "$PKI/leaf_exact.key" "$OUT/leaf.key"

echo "tls-gen-server-pem: expired_leaf.pem/expired_leaf.key (leaf_expired)"
der_to_pem leaf_expired >"$OUT/expired_leaf.pem"
cp "$PKI/leaf_expired.key" "$OUT/expired_leaf.key"

echo "tls-gen-server-pem: wrote $(ls -1 "$OUT" | wc -l | tr -d ' ') files to $OUT"
