#!/usr/bin/env python3
"""Fetches and converts x509-limbo's test suite for otter-x509 (brief M8-T4).

Downloads `limbo.json` from the C2SP/x509-limbo GitHub repository
(<https://github.com/C2SP/x509-limbo>, Apache-2.0 test data), verifies it
against a SHA-256 pinned below, and converts the subset that applies to a
TLS server-auth client -- `validation_kind == "SERVER"` and an `id` starting
with `webpki::` or `rfc5280::` (this crate implements no CRL/OCSP checking,
and no test in that subset needs one) -- into a compact, pipe-delimited
fixture (`crates/otter-x509/tests/fixtures/x509_limbo/cases.txt`), not the
original JSON, so the Rust integration test needs no JSON parser
(DECISIONS.md D27, matching scripts/gen-crypto-vectors.py's own fixtures).

One case (`rfc5280::nc::permitted-dn-match`) has no `expected_peer_name` at
all; `otter_x509::verify_server_chain` always requires a host to check
against, so that single case does not apply and is excluded (recorded as
`excluded_no_expected_peer_name` in the summary this script prints).

Each fixture line is:
    id|expected_result|validation_time_unix|host_kind|host_value|trusted_hex,...|untrusted_hex,...|leaf_hex

The downloaded JSON itself is cached under build/x509-limbo-download/
(gitignored, like scripts/gen-crypto-vectors.py's Wycheproof cache).

Usage: scripts/fetch-x509-limbo.py
"""
import base64
import datetime
import hashlib
import json
import os
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CACHE_DIR = os.path.join(ROOT, "build", "x509-limbo-download")
OUT_DIR = os.path.join(ROOT, "crates", "otter-x509", "tests", "fixtures", "x509_limbo")

# Most applicable cases leave `validation_time` unset (`null`): their
# certificates use an effectively unbounded validity window (x509-limbo's
# own generator defaults to 1970-01-01..2969-05-03) specifically so the
# harness's choice of "current time" does not matter. Any fixed point well
# inside that window works; picked arbitrarily.
DEFAULT_VALIDATION_TIME = "2024-01-15T00:00:00+00:00"

LIMBO_URL = "https://raw.githubusercontent.com/C2SP/x509-limbo/main/limbo.json"
LIMBO_NAME = "limbo.json"
# SHA-256 of the downloaded limbo.json, pinned so a compromised mirror or an
# unannounced format change fails loudly instead of silently changing which
# certificates/outcomes this crate is graded against. Established by
# fetching directly from GitHub over TLS and recording the hash (the same
# trust-on-first-fetch method scripts/gen-crypto-vectors.py and
# scripts/fetch-roots.py use).
EXPECTED_SHA256 = "611e337b9fb477b927bae65434650692cb9d2070ff5b05f975d0403510acd4de"


def sha256_of(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def fetch_limbo_json():
    os.makedirs(CACHE_DIR, exist_ok=True)
    dest = os.path.join(CACHE_DIR, LIMBO_NAME)
    if os.path.exists(dest) and sha256_of(dest) == EXPECTED_SHA256:
        print(f"fetch-x509-limbo: using cached {LIMBO_NAME}")
    else:
        print(f"fetch-x509-limbo: downloading {LIMBO_URL}")
        with urllib.request.urlopen(LIMBO_URL, timeout=120) as resp:
            data = resp.read()
        with open(dest, "wb") as f:
            f.write(data)
        actual = sha256_of(dest)
        if actual != EXPECTED_SHA256:
            raise SystemExit(
                f"fetch-x509-limbo: SHA-256 mismatch for {LIMBO_NAME}\n"
                f"  expected {EXPECTED_SHA256}\n  actual   {actual}"
            )
    with open(dest, "r", encoding="utf-8") as f:
        return json.load(f)


def pem_list_to_hex_list(pems):
    out = []
    for pem in pems:
        lines = [l for l in pem.splitlines() if l and "-----" not in l]
        der = base64.b64decode("".join(lines))
        out.append(der.hex())
    return ",".join(out)


def pem_to_hex(pem):
    lines = [l for l in pem.splitlines() if l and "-----" not in l]
    return base64.b64decode("".join(lines)).hex()


def applies_to_server_auth(t):
    return t["validation_kind"] == "SERVER" and (t["id"].startswith("webpki::") or t["id"].startswith("rfc5280::"))


def main():
    data = fetch_limbo_json()
    testcases = data["testcases"]
    applicable = [t for t in testcases if applies_to_server_auth(t)]

    rows = []
    excluded_no_peer_name = 0
    for t in applicable:
        peer_name = t["expected_peer_name"]
        if peer_name is None:
            excluded_no_peer_name += 1
            continue
        validation_time_str = t["validation_time"] or DEFAULT_VALIDATION_TIME
        validation_time = int(datetime.datetime.fromisoformat(validation_time_str).timestamp())
        trusted_hex = pem_list_to_hex_list(t["trusted_certs"])
        untrusted_hex = pem_list_to_hex_list(t["untrusted_intermediates"])
        leaf_hex = pem_to_hex(t["peer_certificate"])
        rows.append(
            "|".join(
                [
                    t["id"],
                    t["expected_result"],
                    str(validation_time),
                    peer_name["kind"],
                    peer_name["value"],
                    trusted_hex,
                    untrusted_hex,
                    leaf_hex,
                ]
            )
        )

    os.makedirs(OUT_DIR, exist_ok=True)
    out_path = os.path.join(OUT_DIR, "cases.txt")
    with open(out_path, "w", encoding="utf-8") as f:
        f.write("# id|expected_result|validation_time_unix|host_kind|host_value|trusted_hex,...|untrusted_hex,...|leaf_hex\n")
        f.write("# Generated by scripts/fetch-x509-limbo.py -- do not hand-edit.\n")
        for row in rows:
            f.write(row + "\n")

    print(f"fetch-x509-limbo: {len(testcases)} total testcases in limbo.json")
    print(f"fetch-x509-limbo: {len(applicable)} apply (SERVER, webpki::/rfc5280::)")
    print(f"fetch-x509-limbo: excluded {excluded_no_peer_name} with no expected_peer_name")
    print(f"fetch-x509-limbo: wrote {len(rows)} cases -> {out_path}")


if __name__ == "__main__":
    main()
