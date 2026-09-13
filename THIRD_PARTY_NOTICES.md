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
