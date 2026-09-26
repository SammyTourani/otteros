# Brief M4-T7a — otter-wire: the display protocol v1

## Goal
The desktop is a userspace display server plus client apps (D10) talking over message channels
with shared-memory pixel buffers (M4-T3). Pin the protocol as a pure crate both sides link:
`crates/otter-wire` with `ClientMsg`/`ServerMsg`, exact little-endian layouts, strict decoding.
Acceptance: `crates/otter-wire/tests/oracle.rs` (orchestrator, unchanged; its header is the
protocol specification).

## Applies
D2, D10, D27 (no_std + alloc, zero dependencies, `#![forbid(unsafe_code)]`). Add `otter-wire` to
`crates/Cargo.toml` members (single-line edit). Files under ~600 lines (e.g. `lib.rs`,
`client.rs`, `server.rs`, `codec.rs` with a small reader/writer). Never run git
checkout/restore/reset/stash/clean. Tests only via `scripts/verify-crate.sh otter-wire` or
`scripts/memguard.py`. Do not edit the oracle.

## Design notes
- A `Reader` over `&[u8]` with `u8/u16/u32/i16/bool/char/str` methods returning
  `Result<_, WireError>`, and `finish()` that returns Trailing when bytes remain; a `Writer` over
  `Vec<u8>`. Each message kind is one match arm in `encode` and one in `decode`.
- `Cursor` is `#[repr(u8)]` with the listed values; `WireError` = Truncated, Trailing,
  UnknownKind, BadUtf8, TooLong, BadValue.
- Document each message's layout in a doc comment next to its variant (copy the oracle header's
  table): the display server and clients are written from these docs.

## Acceptance
- `scripts/verify-crate.sh otter-wire` → exit 0; the oracle shows 5 passed.
- The oracle file is byte-for-byte unchanged.

## Report
<=15 lines: files, test counts, deviations. Do not commit.
