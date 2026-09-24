# Third-party notices

OtterOS's own kernel logic is original (see DECISIONS.md D2's crate
allowlist -- no OS-logic crates). This file tracks the non-code, tooling,
and boot-protocol pieces that come from elsewhere.

## Limine bootloader

OtterOS boots via [Limine](https://github.com/limine-bootloader/limine)
(binary release branch, currently `v11.x-binary`). Its prebuilt binaries and
host deploy tool are fetched into `third_party/limine` at build time
(`gmake deps`, or automatically as a dependency of `gmake iso`/`test`/etc.)
and are not vendored in this repository (`third_party/` is gitignored).
Limine is BSD-2-Clause licensed, copyright Mintsuki and contributors.

## `limine` Rust crate

Request/response struct definitions for the Limine boot protocol
(framebuffer, HHDM, memory map, RSDP, command line, ...) come from the
`limine` crate (crates.io, dual MIT OR Apache-2.0), used strictly as
"boot protocol structs only" per DECISIONS.md D2 -- it contains no OS logic.

## Embedded font (`kernel/src/font8x8.rs`)

The 8x8 bitmap font drawn on the framebuffer banner (space, A-Z, 0-9, and
common punctuation) is original artwork produced for OtterOS, generated
from hand-authored ASCII-art glyph definitions -- not extracted from any
third-party font file or ROM dump. It carries no licence obligations beyond
this project's own.

## Fonts: Inter and JetBrains Mono (brief M4-T2, `otter-gfx`)

The desktop's UI font (Inter) and monospace font (JetBrains Mono) are data,
not code, per DECISIONS.md D11 -- the same rationale as the kernel's embedded
8x8 font, just licensed separately because these are third-party artwork:

- **Inter**, (c) 2016 The Inter Project Authors (<https://github.com/rsms/inter>),
  SIL Open Font License 1.1. `scripts/fetch-fonts.sh` downloads the official
  `v4.1` release archive and extracts the static (non-variable), TrueType-outline
  (not CFF) Regular and SemiBold weights from `extras/ttf/`.
- **JetBrains Mono**, (c) 2020 The JetBrains Mono Project Authors
  (<https://github.com/JetBrains/JetBrainsMono>), SIL Open Font License 1.1.
  `scripts/fetch-fonts.sh` downloads the official `v2.304` release archive and
  extracts the static, TrueType-outline Regular and Bold weights from
  `fonts/ttf/`.

Both release archives and every extracted `.ttf` file are verified against a
SHA-256 pinned in `scripts/fetch-fonts.sh` before use. Like Limine, the fonts
are fetched into `third_party/fonts/` at tooling time (run the script) rather
than vendored in this repository (`third_party/` is gitignored); each
extracted licence text is copied alongside its font as `LICENSE-OFL.txt`.
`otter-gfx` (`crates/otter-gfx`) is the from-scratch, dependency-free crate
(DECISIONS.md D27) that parses these files -- its own TrueType/PNG/zlib code
is original, only the font *data* comes from elsewhere.

## Project Wycheproof test vectors (brief M8-T1, `otter-crypto`)

`otter-crypto` (`crates/otter-crypto`) is a from-scratch, dependency-free
implementation of SHA-256/384/512, HMAC, HKDF and ChaCha20-Poly1305
(DECISIONS.md D2, D22, D27) -- proven correct against RFC vectors hardcoded in
its own unit tests, plus Project Wycheproof's independent test vectors, which
are test *data*, not code, same rationale as the fonts above:

- **Project Wycheproof** (c) Google LLC and contributors
  (<https://github.com/C2SP/wycheproof>), Apache License 2.0.
  `scripts/gen-crypto-vectors.py` downloads seven JSON files from that
  repository's `testvectors_v1/` directory (`chacha20_poly1305_test.json`,
  `hmac_sha{256,384,512}_test.json`, `hkdf_sha{256,384,512}_test.json`), each
  verified against a SHA-256 pinned in the script before use, and converts them
  into compact line-based fixtures committed under
  `crates/otter-crypto/tests/vectors/` (not the original JSON, so the test
  code needs no JSON parser). The downloaded JSON itself is cached under
  `build/crypto-vector-downloads/` (gitignored, like `third_party/fonts/`)
  rather than vendored.
- The same script also emits `sha2_cross.txt`, a cross-check of this crate's
  SHA-256/384/512 against Python's own `hashlib` (standard library, not a
  "crypto crate" -- host-side test tooling, exactly like `gen-gfx-fixtures.py`
  using `zlib` to build PNG fixtures) for a seeded-random message at every
  length 0-1100 bytes.

Brief M8-T2 (AES-128/256-GCM, X25519) extends `scripts/gen-crypto-vectors.py`
with two more Project Wycheproof files from the same repository, verified and
cached the same way: `aes_gcm_test.json` (split into
`aes128_gcm_wycheproof.txt`/`aes256_gcm_wycheproof.txt`, its 96-bit-IV/
128-bit-tag groups only, same rationale as ChaCha20-Poly1305's fixed-size-API
filtering above) and `x25519_test.json` (`x25519_wycheproof.txt`, converted
whole -- it has no parameter-size variation to filter).

Brief M8-T3 (big integers, RSA PKCS#1 v1.5/PSS and ECDSA P-256/P-384
signature verification) extends the script with seven more Project
Wycheproof files, same repository, same SHA-256-pin-then-cache treatment,
each converted whole (no parameter-size groups to filter, unlike the AEAD
files above): `rsa_signature_{2048,3072,4096}_sha256_test.json` (->
`rsa_pkcs1v15_{2048,3072,4096}_sha256_wycheproof.txt`),
`rsa_pss_2048_sha256_mgf1_32_test.json` / `rsa_pss_4096_sha512_mgf1_64_test.json`
(-> `rsa_pss_2048_sha256_wycheproof.txt` / `rsa_pss_4096_sha512_wycheproof.txt`,
the two files whose salt length equals their hash's output length, this
crate's only supported PSS configuration), and
`ecdsa_secp256r1_sha256_test.json` / `ecdsa_secp384r1_sha384_test.json` (->
`ecdsa_p256_sha256_wycheproof.txt` / `ecdsa_p384_sha384_wycheproof.txt`). The
same brief also adds `bigint_python_cross.txt`, generated the same way as
`sha2_cross.txt` (host-side arithmetic, here Python's own arbitrary-precision
`int` type rather than `hashlib`): random 2048- and 4096-bit moduli, with
expected multiplication, Montgomery-setup-constant and modular-exponentiation
results.

## Root store and x509-limbo test suite (brief M8-T4, `otter-x509`)

`otter-x509` (`crates/otter-x509`) is a from-scratch, dependency-free (beyond
`otter-crypto` by path) implementation of strict DER parsing, X.509
certificate parsing, RFC 5280 certification path validation and RFC 6125
host-name matching (DECISIONS.md D2, D22, D27). Two pieces of third-party
*data* (not code) feed it:

- **The embedded trust anchor bundle**, (c) the curl project and Mozilla
  contributors, [MPL-2.0](https://curl.se/docs/copyright.html).
  `scripts/fetch-roots.py` downloads `cacert.pem`
  (<https://curl.se/ca/cacert.pem>, a Mozilla-derived CA bundle the curl
  project republishes on a regular schedule), verified against a SHA-256
  pinned in the script, and generates `crates/otter-x509/src/roots/
  generated.rs` (committed: this crate has no build script and no network
  access when built into the kernel or userspace, unlike `third_party/`,
  which is fetched at build time and gitignored). The downloaded PEM itself
  is cached under `build/roots-download/` (gitignored).
- **x509-limbo's test suite**, (c) the C2SP project and Google LLC,
  [Apache License 2.0](https://github.com/C2SP/x509-limbo/blob/main/LICENSE).
  `scripts/fetch-x509-limbo.py` downloads `limbo.json`
  (<https://github.com/C2SP/x509-limbo>), verified against a SHA-256 pinned
  in the script, filters it to the `SERVER`-validation `webpki::`/
  `rfc5280::` cases relevant to a TLS client, and converts them to a
  compact, pipe-delimited fixture (`crates/otter-x509/tests/fixtures/
  x509_limbo/cases.txt`, committed; not the original JSON, so
  `tests/x509_limbo.rs` needs no JSON parser -- the same reasoning
  `scripts/gen-crypto-vectors.py`'s Wycheproof fixtures above use). The
  downloaded JSON itself is cached under `build/x509-limbo-download/`
  (gitignored).

Two more fixture sets under `crates/otter-x509/tests/fixtures/` are *not*
third-party data needing a notice: `pki/` is a test PKI (root, intermediates,
leaves) generated entirely by `scripts/make-test-pki.sh` using the Homebrew
OpenSSL CLI, and `chains/` is a point-in-time capture, by
`scripts/capture-chains.py`, of the (public, unauthenticated) certificate
chains six real sites serve to any TLS client during the handshake --
recorded for otter-x509's own tests, not redistributed software.
