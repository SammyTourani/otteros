#!/usr/bin/env python3
"""Generates otter-crypto's host-verified test fixtures (briefs M8-T1/T2/T3).

Two kinds of fixture, both written to crates/otter-crypto/tests/vectors/ as
compact, pipe-delimited text (not the original JSON, so the Rust test code
needs no JSON parser -- DECISIONS.md D22/D27: this crate stays dependency-free
even in its tests):

  - Project Wycheproof vectors, downloaded from the C2SP/wycheproof GitHub
    repository's `testvectors_v1/` directory (the actively-maintained schema;
    the older top-level `testvectors/` layout is gone from that repo). Every
    downloaded file is checked against a SHA-256 pinned below before it is
    used, and a local copy is cached under build/ (like scripts/fetch-fonts.sh
    does for fonts) so re-runs are offline unless a hash actually changes.
    ChaCha20-Poly1305's file has ten test groups; only the one using the
    standard 96-bit nonce / 256-bit key / 128-bit tag is converted, since the
    other nine exist to test *parameter-size* validation that a Rust API with
    fixed-size array types (`&[u8; 12]`, `&[u8; 32]`) rejects at compile time
    rather than at runtime. AES-GCM's file (brief M8-T2) is the same shape and
    gets the same treatment, split into two fixtures (one per key size this
    crate implements, 128/256 -- not 192) since only the two groups with a
    96-bit IV, a 128- or 256-bit key and a 128-bit tag apply (of 45 groups
    total; the rest test IV/key/tag *sizes* this crate's fixed-size API again
    rejects at compile time). X25519's file (brief M8-T2) has a single test
    group of (private, public, shared) triples with no size variation to filter.
    Brief M8-T3 adds RSA (`rsa_signature_{2048,3072,4096}_sha256_test`,
    `rsa_pss_2048_sha256_mgf1_32_test`, `rsa_pss_4096_sha512_mgf1_64_test` --
    each converted whole, one fixture per file, key material and all, since
    unlike the AEAD files above there are no parameter-size groups to filter)
    and ECDSA (`ecdsa_secp256r1_sha256_test`, `ecdsa_secp384r1_sha384_test`,
    likewise converted whole).

  - Host-generated cross-checks, computed directly rather than downloaded
    (not a "crypto crate" in the DECISIONS.md D2 sense -- host-side test
    tooling, exactly like gen-gfx-fixtures.py using `zlib` to make PNG
    fixtures): hashlib cross-checks (SHA-256/384/512 of a seeded-random
    message at every length 0..=1100 bytes -- this is what catches
    padding-boundary bugs, since message *content* diversity does not matter
    for that, only hitting every length relative to the 64-byte (SHA-256) and
    128-byte (SHA-384/512) block size), and (brief M8-T3) a big-integer
    cross-check against Python's own arbitrary-precision integers: random
    2048- and 4096-bit moduli, with multiplication, Montgomery-constant and
    modular-exponentiation cases against each.

Usage: scripts/gen-crypto-vectors.py
"""
import hashlib
import json
import os
import random
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
VECTORS_DIR = os.path.join(ROOT, "crates", "otter-crypto", "tests", "vectors")
CACHE_DIR = os.path.join(ROOT, "build", "crypto-vector-downloads")

WYCHEPROOF_BASE_URL = "https://raw.githubusercontent.com/C2SP/wycheproof/main/testvectors_v1"

# SHA-256 of each raw downloaded JSON file, so a compromised mirror or a
# format change upstream fails loudly instead of silently feeding different
# test data into the fixtures below.
WYCHEPROOF_FILES = {
    "chacha20_poly1305_test.json": "fe61d25f90e1bde4461d00eafe61049e5f29bd999f36b766df9cda90906ad53d",
    "hmac_sha256_test.json": "2d201cfa61d1bf95e6f5d07d96634b4a348b31e8eaa277ad7c8d09677b7a743f",
    "hmac_sha384_test.json": "28b9776e979dd755d852ca471043ea6cedce8b15f7a28abdf6ea9efd982b43c0",
    "hmac_sha512_test.json": "b6c90477bdb4a6fc8ee3d1f7b2c0b69a8dfffab34718abaa6cabd71cc2ba1207",
    "hkdf_sha256_test.json": "bb2b462a38b251cb52a2aede706d6d4b62b26864f4e80c95497507ddb07c5f1e",
    "hkdf_sha384_test.json": "69ff6ea3657bb9c1b8cdffbbb4e7832353d08fd15c0d9997b03f7a6b180e3678",
    "hkdf_sha512_test.json": "bb9a21f4e86041caf5d7792b030349f8ff289087f195b2fbc0fc0afc39deca6f",
    "aes_gcm_test.json": "985e5ecc172e181eaf49e89508b9470dcf478002eb7e8559c707eb42dc97dfe7",
    "x25519_test.json": "35c3f5231cf25cc640b524d403461deee9e49441d5d915a3a25b2c8ff5adbe7d",
    # Brief M8-T3: RSA and ECDSA signature verification.
    "rsa_signature_2048_sha256_test.json": "94a917b01ff50fb874cfc05bf29b4af44868d944a6558201cf18380da93fb393",
    "rsa_signature_3072_sha256_test.json": "0f5f18cabfaad3e2792e82f7e9882f8999049b456714de924b8a5e202f61ca43",
    "rsa_signature_4096_sha256_test.json": "957aca128e30bd02c8982f8ca482d6521d16683f4bbc3f193cdfd4c8f04bc667",
    "rsa_pss_2048_sha256_mgf1_32_test.json": "7f6efafc160f4816b96cbf1c12188a31051d7e3f001e27505d9edb5f2a0e325c",
    "rsa_pss_4096_sha512_mgf1_64_test.json": "c93ceaa56a190c9fd4707441c5c6a75839f108202d88aec891697f9623042547",
    "ecdsa_secp256r1_sha256_test.json": "182db4f3e230f6f9fa9f800d2a614dede30284b8e8438bbfe1171905402e9332",
    "ecdsa_secp384r1_sha384_test.json": "8a5b3ae1760975143414811f13588c24d951d9d8c904195087ba327591dfe9cc",
}

# Reproducible across runs/machines: the hashlib cross-check fixture is
# regenerated identically every time this script runs.
CROSS_CHECK_SEED = 0x0DDBA11
CROSS_CHECK_MAX_LEN = 1100


def sha256_of(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def fetch_wycheproof(name, expected_sha256):
    os.makedirs(CACHE_DIR, exist_ok=True)
    dest = os.path.join(CACHE_DIR, name)
    if os.path.exists(dest) and sha256_of(dest) == expected_sha256:
        print(f"gen-crypto-vectors: using cached {name}")
    else:
        url = f"{WYCHEPROOF_BASE_URL}/{name}"
        print(f"gen-crypto-vectors: downloading {url}")
        with urllib.request.urlopen(url, timeout=60) as resp:
            data = resp.read()
        with open(dest, "wb") as f:
            f.write(data)
        actual = sha256_of(dest)
        if actual != expected_sha256:
            raise SystemExit(
                f"gen-crypto-vectors: SHA-256 mismatch for {name}\n"
                f"  expected {expected_sha256}\n  actual   {actual}"
            )
    with open(dest, "r", encoding="utf-8") as f:
        return json.load(f)


def write_fixture(name, header, rows):
    os.makedirs(VECTORS_DIR, exist_ok=True)
    path = os.path.join(VECTORS_DIR, name)
    with open(path, "w", encoding="utf-8") as f:
        f.write(f"# {header}\n")
        f.write("# Generated by scripts/gen-crypto-vectors.py -- do not hand-edit.\n")
        for row in rows:
            f.write("|".join(row) + "\n")
    print(f"gen-crypto-vectors: wrote {len(rows)} rows -> {path}")
    return len(rows)


# --------------------------------------------------------------------------
# HMAC (Wycheproof MacTest)
# --------------------------------------------------------------------------

def gen_hmac_fixture(hash_name, wycheproof_name):
    doc = fetch_wycheproof(wycheproof_name, WYCHEPROOF_FILES[wycheproof_name])
    rows = []
    for group in doc["testGroups"]:
        for t in group["tests"]:
            rows.append((str(t["tcId"]), t["result"], t["key"], t["msg"], t["tag"]))
    return write_fixture(f"hmac_{hash_name}_wycheproof.txt", f"tcid|result|key_hex|msg_hex|tag_hex ({hash_name})", rows)


# --------------------------------------------------------------------------
# HKDF (Wycheproof HkdfTest)
# --------------------------------------------------------------------------

def gen_hkdf_fixture(hash_name, wycheproof_name):
    doc = fetch_wycheproof(wycheproof_name, WYCHEPROOF_FILES[wycheproof_name])
    rows = []
    for group in doc["testGroups"]:
        for t in group["tests"]:
            rows.append(
                (str(t["tcId"]), t["result"], t["ikm"], t["salt"], t["info"], str(t["size"]), t.get("okm", ""))
            )
    return write_fixture(
        f"hkdf_{hash_name}_wycheproof.txt",
        f"tcid|result|ikm_hex|salt_hex|info_hex|size|okm_hex ({hash_name})",
        rows,
    )


# --------------------------------------------------------------------------
# ChaCha20-Poly1305 (Wycheproof AeadTest)
# --------------------------------------------------------------------------

def gen_chacha20_poly1305_fixture():
    doc = fetch_wycheproof("chacha20_poly1305_test.json", WYCHEPROOF_FILES["chacha20_poly1305_test.json"])
    rows = []
    skipped_groups = 0
    for group in doc["testGroups"]:
        if (group["ivSize"], group["keySize"], group["tagSize"]) != (96, 256, 128):
            # Tests a nonce/key/tag size our fixed-size Rust API (`&[u8; 12]`,
            # `&[u8; 32]`, 16-byte tag) rejects at compile time -- see module
            # docstring.
            skipped_groups += 1
            continue
        for t in group["tests"]:
            rows.append(
                (str(t["tcId"]), t["result"], t["key"], t["iv"], t["aad"], t["msg"], t["ct"], t["tag"])
            )
    print(f"gen-crypto-vectors: chacha20_poly1305: skipped {skipped_groups} non-standard-size test group(s)")
    return write_fixture(
        "chacha20_poly1305_wycheproof.txt",
        "tcid|result|key_hex|iv_hex|aad_hex|msg_hex|ct_hex|tag_hex",
        rows,
    )


# --------------------------------------------------------------------------
# AES-GCM (Wycheproof AeadTest, brief M8-T2)
# --------------------------------------------------------------------------

def gen_aes_gcm_fixture(key_size_bits):
    doc = fetch_wycheproof("aes_gcm_test.json", WYCHEPROOF_FILES["aes_gcm_test.json"])
    rows = []
    skipped_groups = 0
    for group in doc["testGroups"]:
        if (group["ivSize"], group["keySize"], group["tagSize"]) != (96, key_size_bits, 128):
            # Non-96-bit IV groups test the variable-IV GHASH-based derivation
            # this crate does not implement (96-bit nonces only, like the
            # ChaCha20-Poly1305 fixture above); other key sizes (192, and the
            # matching key size's own non-96-bit-IV groups) are simply not
            # this fixture's key size.
            skipped_groups += 1
            continue
        for t in group["tests"]:
            rows.append(
                (str(t["tcId"]), t["result"], t["key"], t["iv"], t["aad"], t["msg"], t["ct"], t["tag"])
            )
    print(f"gen-crypto-vectors: aes_gcm (key={key_size_bits}): skipped {skipped_groups} non-matching group(s)")
    return write_fixture(
        f"aes{key_size_bits}_gcm_wycheproof.txt",
        f"tcid|result|key_hex|iv_hex|aad_hex|msg_hex|ct_hex|tag_hex (AES-{key_size_bits}-GCM)",
        rows,
    )


# --------------------------------------------------------------------------
# X25519 (Wycheproof XdhComp, brief M8-T2)
# --------------------------------------------------------------------------

def gen_x25519_fixture():
    doc = fetch_wycheproof("x25519_test.json", WYCHEPROOF_FILES["x25519_test.json"])
    rows = []
    for group in doc["testGroups"]:
        assert group["curve"] == "curve25519", f"unexpected curve {group['curve']!r}"
        for t in group["tests"]:
            flags = ",".join(t.get("flags", [])) or "-"
            rows.append(
                (str(t["tcId"]), t["result"], flags, t["private"], t["public"], t["shared"])
            )
    return write_fixture(
        "x25519_wycheproof.txt",
        "tcid|result|flags|private_hex|public_hex|shared_hex",
        rows,
    )


# --------------------------------------------------------------------------
# RSA PKCS#1 v1.5 and PSS signature verification (Wycheproof, brief M8-T3)
# --------------------------------------------------------------------------

def gen_rsa_pkcs1v15_fixture(key_size_bits):
    name = f"rsa_signature_{key_size_bits}_sha256_test.json"
    doc = fetch_wycheproof(name, WYCHEPROOF_FILES[name])
    rows = []
    for group in doc["testGroups"]:
        n = group["publicKey"]["modulus"]
        e = group["publicKey"]["publicExponent"]
        for t in group["tests"]:
            rows.append((str(t["tcId"]), t["result"], n, e, t["msg"], t["sig"]))
    return write_fixture(
        f"rsa_pkcs1v15_{key_size_bits}_sha256_wycheproof.txt",
        f"tcid|result|n_hex|e_hex|msg_hex|sig_hex (RSASSA-PKCS1-v1_5, SHA-256, {key_size_bits}-bit modulus)",
        rows,
    )


def gen_rsa_pss_fixture(wycheproof_name, out_name, hash_name):
    doc = fetch_wycheproof(wycheproof_name, WYCHEPROOF_FILES[wycheproof_name])
    rows = []
    for group in doc["testGroups"]:
        assert group["sha"] == hash_name and group["mgfSha"] == hash_name, (
            f"{wycheproof_name}: expected {hash_name} throughout, got sha={group['sha']} mgfSha={group['mgfSha']}"
        )
        n = group["publicKey"]["modulus"]
        e = group["publicKey"]["publicExponent"]
        for t in group["tests"]:
            rows.append((str(t["tcId"]), t["result"], n, e, t["msg"], t["sig"]))
    return write_fixture(
        out_name,
        f"tcid|result|n_hex|e_hex|msg_hex|sig_hex (RSASSA-PSS, {hash_name}, MGF1-{hash_name}, salt length = hash length)",
        rows,
    )


# --------------------------------------------------------------------------
# ECDSA P-256/P-384 signature verification (Wycheproof, brief M8-T3)
# --------------------------------------------------------------------------

def gen_ecdsa_fixture(wycheproof_name, out_name, curve_name, hash_name):
    doc = fetch_wycheproof(wycheproof_name, WYCHEPROOF_FILES[wycheproof_name])
    rows = []
    for group in doc["testGroups"]:
        assert group["publicKey"]["curve"] == curve_name, f"unexpected curve {group['publicKey']['curve']!r}"
        assert group["sha"] == hash_name, f"unexpected hash {group['sha']!r}"
        pubkey = group["publicKey"]["uncompressed"]
        for t in group["tests"]:
            flags = ",".join(t.get("flags", [])) or "-"
            rows.append((str(t["tcId"]), t["result"], flags, pubkey, t["msg"], t["sig"]))
    return write_fixture(
        out_name,
        f"tcid|result|flags|pubkey_uncompressed_hex|msg_hex|der_sig_hex (ECDSA, {curve_name}, {hash_name})",
        rows,
    )


# --------------------------------------------------------------------------
# Big-integer differential cross-check against Python's own arbitrary-
# precision integers (brief M8-T3): random 2048- and 4096-bit cases for
# multiplication, Montgomery-reduction-constant setup, and exponentiation.
# Not Wycheproof data -- generated directly, like the SHA-2 cross-check below.
# --------------------------------------------------------------------------

BIGINT_CROSS_CHECK_SEED = 0xB16_1712
BIGINT_CASES_PER_SIZE = 20


def hex_even(value):
    """`f"{value:x}"`, zero-padded to an even number of digits: Rust's
    `from_be_bytes` (2 hex digits per byte, unlike Python's arbitrary-width
    `int`) needs that, and Python strips all leading zero nibbles by default,
    which yields an odd-length string almost half the time."""
    h = f"{value:x}"
    return h if len(h) % 2 == 0 else "0" + h


def gen_bigint_cross_check_fixture():
    rng = random.Random(BIGINT_CROSS_CHECK_SEED)
    rows = []
    for bits in (2048, 4096):
        for _ in range(BIGINT_CASES_PER_SIZE):
            # Odd (Montgomery needs it coprime to the limb radix, like every
            # real RSA modulus/curve prime) and exactly `bits` bits (top bit set).
            n = rng.getrandbits(bits) | (1 << (bits - 1)) | 1
            a = rng.randrange(n)
            b = rng.randrange(n)
            mul_expected = (a * b) % n
            # `bits` is a multiple of 64, so this crate's own limb count for
            # an n of this bit length (ceil(bits/64)) times 64 is just `bits`.
            r_mod_n = pow(2, bits, n)
            r2_mod_n = pow(2, 2 * bits, n)
            # A modest exponent (not another full-width value): exercises the
            # square-and-multiply loop across many random bit patterns
            # without every one of these 40 cases paying a full 2048/4096-bit
            # exponentiation's cost.
            exponent = rng.getrandbits(rng.randrange(1, 257))
            pow_expected = pow(a, exponent, n)
            rows.append(
                (
                    str(bits),
                    hex_even(n),
                    hex_even(a),
                    hex_even(b),
                    hex_even(mul_expected),
                    hex_even(r_mod_n),
                    hex_even(r2_mod_n),
                    hex_even(exponent),
                    hex_even(pow_expected),
                )
            )
    return write_fixture(
        "bigint_python_cross.txt",
        "bits|n_hex|a_hex|b_hex|mul_mod_expected_hex|r_mod_n_expected_hex|r2_mod_n_expected_hex|exponent_hex|pow_mod_expected_hex",
        rows,
    )


# --------------------------------------------------------------------------
# SHA-256/384/512 cross-check against Python's hashlib
# --------------------------------------------------------------------------

def gen_sha2_cross_check_fixture():
    rng = random.Random(CROSS_CHECK_SEED)
    rows = []
    for length in range(0, CROSS_CHECK_MAX_LEN + 1):
        message = bytes(rng.randrange(256) for _ in range(length))
        rows.append(
            (
                str(length),
                message.hex(),
                hashlib.sha256(message).hexdigest(),
                hashlib.sha384(message).hexdigest(),
                hashlib.sha512(message).hexdigest(),
            )
        )
    return write_fixture(
        "sha2_cross.txt",
        "len|message_hex|sha256_hex|sha384_hex|sha512_hex -- hashlib cross-check, every length 0..=1100",
        rows,
    )


def main():
    counts = {}
    counts["sha2_cross"] = gen_sha2_cross_check_fixture()
    for hash_name, wycheproof_name in (
        ("sha256", "hmac_sha256_test.json"),
        ("sha384", "hmac_sha384_test.json"),
        ("sha512", "hmac_sha512_test.json"),
    ):
        counts[f"hmac_{hash_name}"] = gen_hmac_fixture(hash_name, wycheproof_name)
    for hash_name, wycheproof_name in (
        ("sha256", "hkdf_sha256_test.json"),
        ("sha384", "hkdf_sha384_test.json"),
        ("sha512", "hkdf_sha512_test.json"),
    ):
        counts[f"hkdf_{hash_name}"] = gen_hkdf_fixture(hash_name, wycheproof_name)
    counts["chacha20_poly1305"] = gen_chacha20_poly1305_fixture()
    counts["aes128_gcm"] = gen_aes_gcm_fixture(128)
    counts["aes256_gcm"] = gen_aes_gcm_fixture(256)
    counts["x25519"] = gen_x25519_fixture()

    for key_size in (2048, 3072, 4096):
        counts[f"rsa_pkcs1v15_{key_size}"] = gen_rsa_pkcs1v15_fixture(key_size)
    counts["rsa_pss_2048_sha256"] = gen_rsa_pss_fixture(
        "rsa_pss_2048_sha256_mgf1_32_test.json", "rsa_pss_2048_sha256_wycheproof.txt", "SHA-256"
    )
    counts["rsa_pss_4096_sha512"] = gen_rsa_pss_fixture(
        "rsa_pss_4096_sha512_mgf1_64_test.json", "rsa_pss_4096_sha512_wycheproof.txt", "SHA-512"
    )
    counts["ecdsa_p256_sha256"] = gen_ecdsa_fixture(
        "ecdsa_secp256r1_sha256_test.json", "ecdsa_p256_sha256_wycheproof.txt", "secp256r1", "SHA-256"
    )
    counts["ecdsa_p384_sha384"] = gen_ecdsa_fixture(
        "ecdsa_secp384r1_sha384_test.json", "ecdsa_p384_sha384_wycheproof.txt", "secp384r1", "SHA-384"
    )

    counts["bigint_python_cross"] = gen_bigint_cross_check_fixture()

    host_generated = {"sha2_cross", "bigint_python_cross"}
    total_wycheproof = sum(v for k, v in counts.items() if k not in host_generated)
    total_host = sum(counts[k] for k in host_generated)
    print(f"gen-crypto-vectors: {total_wycheproof} Wycheproof cases, {total_host} host-generated cross-check rows")


if __name__ == "__main__":
    main()
