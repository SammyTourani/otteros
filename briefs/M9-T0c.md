# Brief M9-T0c — `otter-grammar`: grammar-constrained decoding for Otter's tool calls

## Goal
When the local model (Otter) calls a tool, every token it samples is masked so the output is
always a valid tool call: well-formed JSON with a known tool name and an input that satisfies that
tool's JSON schema. Invalid tool calls become impossible rather than merely unlikely. This is the
M9 "local backend" requirement; the sampler applies the mask (disallowed logits → −inf).

## Applies
D2, D23 (tool calls still need Allow/Deny; the grammar only guarantees their shape), D27 (pure
crate, no_std + alloc, zero external dependencies; path dependencies on otter-json and, for tests
only, otter-llm and otter-claude are allowed).

## Design (crates/otter-grammar)
1. Schema subset, compiled from JSON Schema text (parsed with otter-json): `object` with
   `properties` (keys must appear in the listed order; optional keys may be skipped), `required`,
   `additionalProperties: false` (implied); `string` with optional `enum` (plain printable ASCII
   values) and `maxLength` (Unicode scalar values after unescaping); `const` strings; `integer`
   (`-?(0|[1-9][0-9]*)`, at most 18 digits); `boolean`; `null`; `array` with `items` and
   `maxItems`; `anyOf` whose alternatives are objects with a distinguishing first `const`
   property. Anything else → a compile error naming the unsupported keyword.
2. `Grammar::tool_call(tools: &[(&str name, &str input_schema_json)], prefix: &[u8], suffix: &[u8])`
   builds: literal `prefix`, then `{"name": <const name>, "input": <that tool's schema>}` as an
   anyOf over the tools, then literal `suffix` (e.g. `<tool_call>` … `</tool_call>`), then end.
3. Byte-level JSON: at most one space (0x20) at each structural position (after `{ [ , :`), no
   other whitespace; strings are valid UTF-8 (a UTF-8 character may be split across tokens: track
   continuation bytes), no raw control characters, escapes `\" \\ \/ \b \f \n \r \t \uXXXX` with
   surrogate code points rejected; exactly one top-level value.
4. `Matcher` holds the automaton state (a small stack of frames; cheap to clone). API:
   `Matcher::new(&Grammar)`, `accept_bytes(&[u8]) -> Result<(), Rejected>`,
   `accept_token(&Vocab, id)`, `allowed_tokens(&Vocab, &mut TokenMask)`, `is_complete()` (the end
   token is allowed exactly when complete), `reset()`.
5. `Vocab::new(token_bytes: Vec<Vec<u8>>, end_token: u32, special: &[u32])` builds a byte trie
   once; special tokens other than the end token are never allowed. `allowed_tokens` walks the
   trie depth-first carrying the matcher state, pruning a subtree at the first rejected byte, so a
   step costs far less than trying all tokens.

## Tests (host; `cargo test -p otter-grammar`; ≥ 35 tests)
Tool set for the tests (write these schemas once in a test module; the same shapes will be used by
the OS): list_dir{path}, read_file{path}, write_file{path, content}, run_command{command},
open_app{app: enum [terminal, editor, files, otter], path?}, fetch_url{url (maxLength 2048)},
screenshot{}, plus a synthetic schema that exercises integer, boolean, null, arrays with
maxItems and nested objects.
- Unit: accepts hand-written valid calls (escapes, Unicode, optional key present and absent,
  empty strings, split UTF-8); rejects at the exact offending byte: wrong key order, unknown key,
  missing required key, unknown tool name, value not in enum, maxLength + 1, trailing comma, raw
  newline in a string, `\uD800`, invalid UTF-8, a leading zero, a second top-level value, text
  after the suffix.
- Mask equivalence: for 300 random reachable states, the trie mask equals a brute-force mask made
  by trying every token independently, with (a) the real SmolLM2 vocabulary (load the .otm from
  `$OTTEROS_MODEL_DIR`, default /Volumes/SammyDisk/models/otteros, with otter-llm's loader; fail
  loudly if missing unless `OTTEROS_SKIP_MODEL_TESTS=1`) and (b) a synthetic vocabulary of all 256
  single bytes plus 2,000 random multi-byte tokens.
- Soundness: 2,000 seeded random walks, each sampling uniformly among allowed tokens (biased toward
  closing strings after 40 bytes so walks end): every completed output parses with otter-json, has
  a known tool name, and its `input` passes otter-claude's `validate_tool_input` for that tool's
  schema. At least 95 % of walks complete within 512 tokens; none ever reaches a state where no
  token is allowed (no dead ends).
- Completeness: 1,000 random valid tool calls generated from the schemas (random strings with
  escapes, Unicode and spaces), serialized with the allowed spacing and tokenized with otter-llm's
  real tokenizer, are accepted token by token, end complete, and the end token is allowed only at
  the end.
- Speed (release, reported, asserted loosely): mean `allowed_tokens` time over a full tool call with
  the SmolLM2 vocabulary; assert < 20 ms, report the actual number.

## Acceptance
`scripts/verify-crate.sh otter-grammar` → ≥ 35 tests passed, 0 failed, clippy ok, no_std ok,
external deps 0 (internal path dependencies are allowed), and the release speed number reported.

## Report
<=10 lines: the verify-crate line, walks completed / dead ends, completeness result, mask speed,
deviations. Do not commit.
