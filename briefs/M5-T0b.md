# Brief M5-T0b — `otter-html`: HTML tokenizer and document tree (pure crate)

## Goal
Turn real web pages into a document tree that the OtterOS page viewer can lay out: a WHATWG-style
tokenizer proven against the html5lib test suite, a forgiving tree builder, and helpers for text,
links and titles.

## Applies
D2, D27 (pure no_std + alloc, zero external dependencies; path dependency on otter-http for URL
resolution is fine).

## Scope and boundaries (other agents work in kernel/, user/ and crates/otter-claude)
New crate `crates/otter-html/`, `scripts/fetch-html5lib-tests.py`, fixtures under
`crates/otter-html/tests/`. Do not touch anything else. Do not run gmake or QEMU.

## Design
1. Tokenizer following the WHATWG tokenization states that matter for real pages: data, tag open,
   end tag open, tag name, attribute name/value (double-, single- and unquoted), self-closing,
   markup declaration, comments, DOCTYPE (name, public/system ids, force-quirks), character
   references (numeric decimal/hex with the replacement rules, and the full named reference table
   generated from the WHATWG entities.json into Rust source by the fetch script), RCDATA (title,
   textarea), RAWTEXT (style, xmp, iframe, noembed, noframes), script data (content kept as raw
   text), PLAINTEXT. Input is UTF-8; invalid sequences become U+FFFD.
2. Tree builder (simplified, forgiving; not the full adoption agency algorithm): html/head/body
   always present, head elements routed to head, void elements, implied end tags for p, li, dd,
   dt, option, optgroup, tr, td, th, thead, tbody, tfoot, rp, rt; mismatched end tags close up to
   the nearest matching open element or are ignored; text merged into adjacent text nodes.
3. Document API: arena of nodes (Document, Element {tag, attrs}, Text, Comment) with parent and
   children links; `title()`, `text_content(node)`, `elements_by_tag(tag)`, `element_by_id`,
   `links()` returning (text, href resolved against a base URL via otter-http), `<base href>`
   honoured, block/inline classification per tag for the layout engine.

## Tests (host)
- `scripts/fetch-html5lib-tests.py` downloads html5lib-tests (github.com/html5lib/html5lib-tests,
  pinned commit or SHA-256) into third_party/ (an SSD symlink) and entities.json from the WHATWG.
- Tokenizer: run every case in tokenizer/test1..test4.test, entities.test, numericEntities.test,
  domjs? (whatever exists), comparing tokens (ignore parse-error counts); report the pass rate per
  file; required ≥ 95% on test1–test4 and 100% on the entity tests.
- Tree builder: run tree-construction/tests1.dat and report the pass rate (informational, the
  builder is simplified), plus targeted tests for every rule in §2.
- Real page: a fixture captured from https://example.com (title "Example Domain", one link
  resolved against the base), and synthetic pages with nested lists, tables, unclosed p/li, scripts
  and styles, and malformed markup (never panics; fuzz by truncating at every offset).

## Acceptance
- `cd crates && cargo test -p otter-html` -> 0 with the pass rates above; clippy with --tests clean; builds for x86_64-unknown-none

## Report
<=10 lines with the observed pass rates. Do not commit.
