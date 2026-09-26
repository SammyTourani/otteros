# Brief M5-T2a — otter-html: parse simple pages, render blocks for the viewer, wrap text for `fetch`

## Goal
PLAN M5 wants `fetch <url>` in the shell and a GUI page viewer that shows headings, paragraphs and
links of simple HTML. otter-html has an html5lib-conformant tokenizer and a `Document` type but no
tree builder. Add a forgiving tree builder, a rendering model (blocks of styled spans with
resolved links) and a plain-text formatter with numbered link references. Acceptance:
`crates/otter-html/tests/render_oracle.rs` (orchestrator, unchanged; its header is the API and
every rule) and every existing otter-html test (html5lib conformance must not regress).

## Applies
D2, D27 (no_std + alloc; otter-http is already a dependency, use its `Url` for resolution).
Files under ~600 lines (e.g. `src/tree.rs`, `src/render.rs`, `src/text.rs`). Never run git
checkout/restore/reset/stash/clean. Tests only via `scripts/verify-crate.sh otter-html` or
`scripts/memguard.py`. Do not edit the oracle. Host-only: no gmake/QEMU.

## Design notes
- Document mode for the tokenizer: after emitting a start tag for script/style/xmp/iframe/
  noembed/noframes/title/textarea/plaintext, continue in the matching text state with that tag as
  the last start tag (the html5lib tests keep using `new`/`with_state`, unchanged).
- The tree builder keeps a stack of open elements and applies the oracle's closing rules; it is
  not the full HTML5 insertion-mode algorithm, and must never panic or loop on any input.
- `render` walks the tree once, carrying inline style (bold/italic/code/link) down and emitting a
  block whenever a block-level element starts or ends; whitespace collapsing follows CSS
  `white-space: normal` across inline boundaries.
- `to_text`: greedy word wrap by characters; the reference list is built while formatting.

## Acceptance
- `scripts/verify-crate.sh otter-html` → exit 0; render_oracle shows 10 passed; the html5lib
  oracle still passes.
- The oracle file is byte-for-byte unchanged.

## Report
<=15 lines: files, test counts (render oracle and html5lib), deviations. Do not commit.
