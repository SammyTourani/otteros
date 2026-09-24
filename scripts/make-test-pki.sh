#!/usr/bin/env bash
# Generates otter-x509's test PKI fixtures (brief M8-T4), using the Homebrew
# OpenSSL 3.x CLI's modern `req -x509 -CA/-CAkey` one-shot signing (no
# intermediate CSR files, no extfile: extensions come straight from
# `-addext`). Every certificate is committed as DER under
# tests/fixtures/pki/; every private key is committed alongside as PEM
# (these are throwaway test keys with no security value -- reused, per the
# brief, by M8-T5's local TLS server).
#
# Hierarchy (all "otter-test.example", RFC 2606 has no reserved TLD for this,
# but it is never resolved -- only used as SAN/subject strings):
#
#   root (P-256, self-signed CA)
#   +-- intermediate_pathlen0 (RSA-2048, CA, pathLen 0)
#   |   +-- leaf_exact              SAN DNS:leaf.otter-test.example
#   |   +-- leaf_wildcard           SAN DNS:*.wild.otter-test.example
#   |   +-- leaf_ip                 SAN IP:203.0.113.42
#   |   +-- leaf_wrong_eku          EKU clientAuth only (no serverAuth)
#   |   +-- leaf_unknown_critical_ext   an unrecognized extension marked critical
#   |   +-- leaf_expired            validity 2020-01-01..2020-02-01
#   |   +-- sub_intermediate (RSA-2048, CA, no pathLen of its own)
#   |       +-- leaf_under_sub_intermediate   (violates intermediate_pathlen0's pathLen:0)
#   +-- non_ca (RSA-2048, CA:false)
#   |   +-- leaf_via_non_ca         signed by a non-CA "issuer"
#   +-- intermediate_name_constrained (P-384, CA, nameConstraints permitted DNS:nc.otter-test.example)
#       +-- leaf_nc_ok               SAN DNS:host.nc.otter-test.example
#       +-- leaf_nc_violation        SAN DNS:host.other-domain.example
#
#   self_signed_leaf (RSA-2048, self-signed, CA:false, not in any trust store)
#
# Usage: scripts/make-test-pki.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/crates/otter-x509/tests/fixtures/pki"
OPENSSL=/opt/homebrew/bin/openssl

mkdir -p "$OUT"
cd "$OUT"

NORMAL_NOT_BEFORE=20240101000000Z
NORMAL_NOT_AFTER=20340101000000Z
EXPIRED_NOT_BEFORE=20200101000000Z
EXPIRED_NOT_AFTER=20200201000000Z

# Silences the (expected, harmless) "Not using -key or -newkey for signing
# since -CA option is given" warning `openssl req -x509 -CA` prints on every
# CA-signed (non-self-signed) call below.
req() {
    "$OPENSSL" req -new -x509 "$@" 2>&1 | grep -v "Not using -key or -newkey" || true
}

to_der() {
    local name=$1
    "$OPENSSL" x509 -in "$name.pem" -outform der -out "$name.der"
    rm -f "$name.pem"
}

echo "make-test-pki: root (P-256, self-signed)"
"$OPENSSL" ecparam -name prime256v1 -genkey -noout -out root.key
req -key root.key -subj "/CN=Otter Test Root CA" \
    -addext "basicConstraints=critical,CA:true" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -not_before "$NORMAL_NOT_BEFORE" -not_after 20440101000000Z \
    -sha256 -out root.pem
to_der root

echo "make-test-pki: intermediate_pathlen0 (RSA-2048, pathLen 0)"
"$OPENSSL" genrsa -out intermediate_pathlen0.key 2048 2>/dev/null
req -key intermediate_pathlen0.key -subj "/CN=Otter Test Intermediate PathLen0" \
    -CA root.der -CAkey root.key \
    -addext "basicConstraints=critical,CA:true,pathlen:0" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -not_before "$NORMAL_NOT_BEFORE" -not_after "$NORMAL_NOT_AFTER" \
    -sha256 -out intermediate_pathlen0.pem
to_der intermediate_pathlen0

leaf_under_int0() {
    local name=$1 subj=$2 not_before=$3 not_after=$4
    shift 4
    "$OPENSSL" genrsa -out "$name.key" 2048 2>/dev/null
    req -key "$name.key" -subj "$subj" \
        -CA intermediate_pathlen0.der -CAkey intermediate_pathlen0.key \
        -addext "basicConstraints=critical,CA:false" \
        "$@" \
        -not_before "$not_before" -not_after "$not_after" \
        -sha256 -out "$name.pem"
    to_der "$name"
}

echo "make-test-pki: leaf_exact"
leaf_under_int0 leaf_exact "/CN=leaf.otter-test.example" "$NORMAL_NOT_BEFORE" "$NORMAL_NOT_AFTER" \
    -addext "keyUsage=critical,digitalSignature,keyEncipherment" \
    -addext "extendedKeyUsage=serverAuth" \
    -addext "subjectAltName=DNS:leaf.otter-test.example"

echo "make-test-pki: leaf_wildcard"
leaf_under_int0 leaf_wildcard "/CN=wild.otter-test.example" "$NORMAL_NOT_BEFORE" "$NORMAL_NOT_AFTER" \
    -addext "extendedKeyUsage=serverAuth" \
    -addext "subjectAltName=DNS:*.wild.otter-test.example"

echo "make-test-pki: leaf_ip"
leaf_under_int0 leaf_ip "/CN=leaf-ip.otter-test.example" "$NORMAL_NOT_BEFORE" "$NORMAL_NOT_AFTER" \
    -addext "extendedKeyUsage=serverAuth" \
    -addext "subjectAltName=IP:203.0.113.42"

echo "make-test-pki: leaf_wrong_eku (clientAuth only, no serverAuth)"
leaf_under_int0 leaf_wrong_eku "/CN=wrongeku.otter-test.example" "$NORMAL_NOT_BEFORE" "$NORMAL_NOT_AFTER" \
    -addext "extendedKeyUsage=clientAuth" \
    -addext "subjectAltName=DNS:wrongeku.otter-test.example"

echo "make-test-pki: leaf_unknown_critical_ext (an unrecognized critical extension)"
leaf_under_int0 leaf_unknown_critical_ext "/CN=unknownext.otter-test.example" "$NORMAL_NOT_BEFORE" "$NORMAL_NOT_AFTER" \
    -addext "extendedKeyUsage=serverAuth" \
    -addext "subjectAltName=DNS:unknownext.otter-test.example" \
    -addext "1.2.3.4.5.6.7=critical,DER:0500"

echo "make-test-pki: leaf_expired (validity 2020-01-01..2020-02-01)"
leaf_under_int0 leaf_expired "/CN=expired.otter-test.example" "$EXPIRED_NOT_BEFORE" "$EXPIRED_NOT_AFTER" \
    -addext "extendedKeyUsage=serverAuth" \
    -addext "subjectAltName=DNS:expired.otter-test.example"

echo "make-test-pki: sub_intermediate (under intermediate_pathlen0; violates its pathLen:0)"
"$OPENSSL" genrsa -out sub_intermediate.key 2048 2>/dev/null
req -key sub_intermediate.key -subj "/CN=Otter Test Sub-Intermediate" \
    -CA intermediate_pathlen0.der -CAkey intermediate_pathlen0.key \
    -addext "basicConstraints=critical,CA:true" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -not_before "$NORMAL_NOT_BEFORE" -not_after "$NORMAL_NOT_AFTER" \
    -sha256 -out sub_intermediate.pem
to_der sub_intermediate

echo "make-test-pki: leaf_under_sub_intermediate"
"$OPENSSL" genrsa -out leaf_under_sub_intermediate.key 2048 2>/dev/null
req -key leaf_under_sub_intermediate.key -subj "/CN=pathlenviolation.otter-test.example" \
    -CA sub_intermediate.der -CAkey sub_intermediate.key \
    -addext "basicConstraints=critical,CA:false" \
    -addext "extendedKeyUsage=serverAuth" \
    -addext "subjectAltName=DNS:pathlenviolation.otter-test.example" \
    -not_before "$NORMAL_NOT_BEFORE" -not_after "$NORMAL_NOT_AFTER" \
    -sha256 -out leaf_under_sub_intermediate.pem
to_der leaf_under_sub_intermediate

echo "make-test-pki: non_ca (CA:false, used to sign a leaf anyway)"
"$OPENSSL" genrsa -out non_ca.key 2048 2>/dev/null
req -key non_ca.key -subj "/CN=Otter Test Non-CA" \
    -CA root.der -CAkey root.key \
    -addext "basicConstraints=critical,CA:false" \
    -not_before "$NORMAL_NOT_BEFORE" -not_after "$NORMAL_NOT_AFTER" \
    -sha256 -out non_ca.pem
to_der non_ca

echo "make-test-pki: leaf_via_non_ca"
"$OPENSSL" genrsa -out leaf_via_non_ca.key 2048 2>/dev/null
req -key leaf_via_non_ca.key -subj "/CN=leaf-via-non-ca.otter-test.example" \
    -CA non_ca.der -CAkey non_ca.key \
    -addext "basicConstraints=critical,CA:false" \
    -addext "extendedKeyUsage=serverAuth" \
    -addext "subjectAltName=DNS:leaf-via-non-ca.otter-test.example" \
    -not_before "$NORMAL_NOT_BEFORE" -not_after "$NORMAL_NOT_AFTER" \
    -sha256 -out leaf_via_non_ca.pem
to_der leaf_via_non_ca

echo "make-test-pki: intermediate_name_constrained (P-384, permitted DNS:nc.otter-test.example)"
"$OPENSSL" ecparam -name secp384r1 -genkey -noout -out intermediate_name_constrained.key
req -key intermediate_name_constrained.key -subj "/CN=Otter Test Intermediate NameConstrained" \
    -CA root.der -CAkey root.key \
    -addext "basicConstraints=critical,CA:true" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -addext "nameConstraints=critical,permitted;DNS:nc.otter-test.example" \
    -not_before "$NORMAL_NOT_BEFORE" -not_after "$NORMAL_NOT_AFTER" \
    -sha256 -out intermediate_name_constrained.pem
to_der intermediate_name_constrained

leaf_under_nc() {
    local name=$1 subj=$2 san=$3
    "$OPENSSL" genrsa -out "$name.key" 2048 2>/dev/null
    req -key "$name.key" -subj "$subj" \
        -CA intermediate_name_constrained.der -CAkey intermediate_name_constrained.key \
        -addext "basicConstraints=critical,CA:false" \
        -addext "extendedKeyUsage=serverAuth" \
        -addext "subjectAltName=DNS:$san" \
        -not_before "$NORMAL_NOT_BEFORE" -not_after "$NORMAL_NOT_AFTER" \
        -sha384 -out "$name.pem"
    to_der "$name"
}

echo "make-test-pki: leaf_nc_ok (within the permitted subtree)"
leaf_under_nc leaf_nc_ok "/CN=host.nc.otter-test.example" "host.nc.otter-test.example"

echo "make-test-pki: leaf_nc_violation (outside the permitted subtree)"
leaf_under_nc leaf_nc_violation "/CN=host.other-domain.example" "host.other-domain.example"

echo "make-test-pki: self_signed_leaf (self-signed, not in any trust store)"
"$OPENSSL" genrsa -out self_signed_leaf.key 2048 2>/dev/null
req -key self_signed_leaf.key -subj "/CN=selfsigned.otter-test.example" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "extendedKeyUsage=serverAuth" \
    -addext "subjectAltName=DNS:selfsigned.otter-test.example" \
    -not_before "$NORMAL_NOT_BEFORE" -not_after "$NORMAL_NOT_AFTER" \
    -sha256 -out self_signed_leaf.pem
to_der self_signed_leaf

rm -f *.srl
echo "make-test-pki: wrote $(ls -1 "$OUT"/*.der | wc -l | tr -d ' ') certificates + $(ls -1 "$OUT"/*.key | wc -l | tr -d ' ') keys to $OUT"
