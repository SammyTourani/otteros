# Brief M9-T0d — otter-tools: the agent's tools, their safety policy, the action log

## Goal
Agent mode lets Claude (and, later, the local Otter model) operate OtterOS through seven tools.
Everything that decides what a tool call may do lives in one host-tested crate, so the kernel
and desktop only supply the `Os` trait (files, commands, apps, network, screenshots, the Allow/Deny
dialog): input validation against each tool's JSON schema, path normalisation, the protected
paths (the API key's directory, devices), where writes may go, which calls need the human's
approval and what the dialog says, output formatting and limits, redaction of the key, and the
action log. Crate `crates/otter-tools`, passing `crates/otter-tools/tests/oracle.rs`
(orchestrator, unchanged; its header is the exact API and policy). DECISIONS D23 and D30.

## Applies
D2, D27: `#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`, depends only on `otter-json` (path
dependency; parse inputs with it, write the schemas as JSON text). Add `otter-tools` to
`crates/Cargo.toml` members (single-line edit). Files under ~600 lines (e.g. `lib.rs`, `spec.rs`
(the seven ToolSpecs), `path.rs`, `policy.rs`, `session.rs`, `format.rs`). Never run git
checkout/restore/reset/stash/clean. Tests only via `scripts/verify-crate.sh` or
`scripts/memguard.py`. Do not edit the oracle.

## Design notes
- Tool schemas are JSON Schema objects with `"additionalProperties": false`, `required`, and an
  `enum` for open_app's `app`; the model sees them verbatim. Descriptions tell the model what the
  tool does, its limits (64 KiB reads, 16 KiB command output, 30 s timeout) and that the human
  approves state-changing calls.
- Validate against the schema in the crate (a small checker for the subset used: object,
  string, enum, required, additionalProperties false); do not trust the model.
- The policy order in the oracle header matters: invalid input and refusals never reach the
  dialog or the `Os`.
- Redaction: replace every occurrence of the secret in text (tool results, approval summary and
  detail, log summaries); an empty secret disables redaction.
- The session never panics on any input; `call` always returns a `ToolResult` and logs once.

## Acceptance
- `scripts/verify-crate.sh otter-tools` → exit 0; the oracle shows 12 passed.
- The oracle file is byte-for-byte unchanged (the orchestrator compares checksums).

## Report
<=15 lines: files, test counts, deviations, any oracle expectation you believe is unsafe or
inconsistent (quote it). Do not commit.
