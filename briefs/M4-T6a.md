# Brief M4-T6a — Terminal and Editor apps as host-testable libraries

## Goal
The two workhorse desktop apps, complete except for their OS backends: each is a library
implementing an `App` trait over otter-ui, talking to the OS only through small service traits, so
they are tested and rendered on the Mac now and get real backends when the display server lands.

## Applies
D2, D10, D24, D27, docs/DESIGN.md. Path dependencies: otter-ui, otter-gfx, otter-term, otter-textbuf.

## Design
- New crate `crates/otter-apps/` with `trait App { fn title(&self) -> String; fn handle(&mut self,
  event: Event, now_ms: u64) -> Vec<AppRequest>; fn render(&mut self, surface: &mut Surface); fn
  min_size(&self) -> (u32, u32); }` and `AppRequest` (Close, SetTitle, Spawn, WriteFile, …).
- Terminal app: owns an otter-term Terminal (newline mode on, because OtterOS has no pty layer) and
  a `trait ByteChannel { fn read(&mut self, buf) -> usize; fn write(&mut self, bytes); }` to the
  shell; key events → bytes via otter-term; renders the TerminalView with a blinking cursor (caller
  time), scrollback via wheel and Shift+PageUp/PageDown, selection by drag and copy with Ctrl+Shift+C,
  window title from OSC 0/2.
- Editor app: owns an otter-textbuf buffer, `trait FileStore { read(path) -> Result<Vec<u8>>;
  write(path, bytes) -> Result<()> }`; shows a title with the file name and a modified dot, line
  numbers, a status bar (line:col, UTF-8, LF/CRLF), keyboard shortcuts Ctrl+S save, Ctrl+Z/Y undo/
  redo, Ctrl+F find bar, Ctrl+A select all, Ctrl+X/C/V clipboard (caller-owned clipboard via a
  trait), scrolling that keeps the cursor visible, mouse click to place the cursor and drag to select.

## Tests and specimens (host)
Unit tests driving each app with events against fake backends: typing a command into the terminal
sends the right bytes and output appears on screen; scrollback; the editor opens a file, edits,
saves exactly the expected bytes, undo/redo, find, clipboard, and the modified dot. Specimens in
artifacts/app-specimens/: terminal-app.png (the recorded OtterOS shell session from crates/otter-term
fed through a fake channel), editor-app.png (a Rust file with a selection and the find bar open).

## Acceptance
`scripts/verify-crate.sh otter-apps` all clear with >= 30 tests; both specimens exist.
