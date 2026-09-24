# Brief M4-T5b — `otter-textbuf`: text editor core (pure crate)

## Goal
The engine of the desktop Editor app: a text buffer with cursor movement, selection, editing,
undo/redo, search, and line/column mapping, independent of rendering, tested on the Mac.

## Applies
D2, D27 (pure no_std + alloc, zero external dependencies).

## Design
- Storage: a piece table or gap buffer over UTF-8 with a line index kept up to date incrementally;
  positions are byte offsets on char boundaries; (line, column) conversion in chars.
- Cursor and selection: anchor + head; movement by char, word (Unicode-aware enough for ASCII
  words and punctuation), line start/end, document start/end, up/down keeping a preferred column,
  page up/down by a given line count; shift-extends selection.
- Editing: insert text (replacing the selection), backspace/delete (grapheme-naive: by char), new
  line with auto-indent copying the previous line's leading whitespace, tab inserts four spaces,
  indent/outdent selected lines, cut/copy/paste via a caller-owned clipboard string.
- Undo/redo: grouped edits (consecutive typing merges until a cursor jump or 1 s pause supplied by
  the caller's clock), redo cleared on a new edit.
- Search: find next/previous (case-sensitive and insensitive), replace one/all.
- Load/save helpers: from bytes (invalid UTF-8 rejected with the byte offset), CRLF and LF both
  read, the original line ending style preserved on save; `is_modified` flag.

## Tests (host, >= 50)
Every operation, including edge cases at document start/end, empty lines and multi-byte
characters; undo/redo grouping; a randomized differential test: 10,000 seeded random edit
operations applied to the buffer and to a plain `String` model must always agree on content and
cursor; large-file behaviour (a 5 MB file loads and edits within generous time bounds).

## Acceptance
`scripts/verify-crate.sh otter-textbuf` shows 0 failed, 0 ignored, clippy ok, no_std ok, 0 external deps.
