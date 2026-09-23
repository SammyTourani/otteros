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
