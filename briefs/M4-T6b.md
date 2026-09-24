# Brief M4-T6b — Otter assistant app and Files app as host-testable libraries

## Goal
The app the demo revolves around, and a file browser, built like M4-T6a: libraries over otter-ui with
the OS reached through traits, tested and rendered on the Mac.

## Applies
D2, D10, D23 (every state-changing tool call needs an on-screen Allow/Deny), D24, D27,
docs/DESIGN.md. Path dependencies: otter-ui, otter-gfx, otter-json. Put the apps in a new crate
`crates/otter-apps-ai/` (another agent owns crates/otter-apps concurrently; copy its `App` trait
shape from briefs/M4-T6a.md and unify later).

## Design
- Otter app: a conversation view (user bubbles in accent with dark text, assistant text on
  surface.dark, markdown-lite: paragraphs, bullet lists, inline and fenced code in JetBrains Mono),
  a composer (text field, Send, Enter to send, Shift+Enter newline), a backend picker (Local Otter /
  Claude), a streaming indicator with tokens/s, a Stop button, and tool-call cards: each shows the
  tool name and its arguments in monospace; read-only tools show "ran automatically"; state-changing
  tools show Allow (ok green) and Deny (danger red) with Enter/Esc shortcuts, and nothing runs until
  the human answers. An action log panel lists every tool call with its outcome. The backend is a
  trait `Assistant { fn start(&mut self, conversation) ; fn poll(&mut self) -> Vec<AssistantEvent> }`
  with events TextDelta, ThinkingDelta (shown as a subtle one-line "thinking…" summary),
  ToolRequest{id, name, args, mutating}, Done{stop_reason}, Error.
- Files app: a directory view over `trait FileSystem { list(path) -> Vec<Entry{name, is_dir, size,
  mtime}>; … }` with a path bar, sortable name/size/date columns, keyboard and mouse navigation,
  open-in-editor request, new folder, delete (with confirmation), rename.

## Tests and specimens (host)
Event-driven tests with fake backends: streaming text appears incrementally; Stop cancels; a
mutating tool request blocks until Allow, Deny sends a denial result, read-only runs automatically;
the action log records outcomes; Files navigation and requests. Specimens in artifacts/app-specimens/:
otter-app.png (a conversation where Claude asks to write /data/notes/haiku.txt, the Allow/Deny card
visible, earlier messages including a code block), otter-app-local.png (local backend streaming with
tokens/s), files-app.png.

## Acceptance
`scripts/verify-crate.sh otter-apps-ai` all clear with >= 30 tests; the three specimens exist.
