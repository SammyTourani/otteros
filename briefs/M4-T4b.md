# Brief M4-T4b — `otter-wm`: window manager and compositor logic (pure crate)

## Goal
Everything the display server decides, without any kernel code: the window stack, focus, moving and
resizing, window decorations, the taskbar, the cursor, damage tracking and composition into a frame,
using otter-gfx. The kernel-facing display server (M4) becomes a thin loop around this.

## Applies
D2, D10, D24, D27, docs/DESIGN.md. Path dependency: otter-gfx.

## Design
- Model: windows {id, title, rect, client surface (premultiplied ARGB), state normal/minimised/
  maximised, focus, z}; a desktop with wallpaper (DESIGN.md gradient + glow, dithered) and a
  floating taskbar (centred app icons drawn procedurally, an Otter button in accent on the left,
  a clock text on the right supplied by the caller).
- Decorations per DESIGN.md: 34 px title bar, title centred in Inter semibold, close and minimise
  dots, 10 px rounded corners, soft shadows (focused vs unfocused values from DESIGN.md).
- Input routing: pointer events hit-test decorations (drag title to move, drag edges/corners to
  resize with minimum size, click dots), otherwise route to the window under the pointer with
  window-local coordinates; click raises and focuses; keyboard to the focused window; Alt+Tab
  cycles focus; the taskbar restores minimised windows.
- Composition: damage rectangles unioned per frame (window moves, content updates, cursor moves,
  clock), only damaged regions recomposed; a software cursor sprite (arrow, drawn as a vector path
  with a white outline); output is a frame Surface plus the list of dirty rects to blit.
- Performance: compose a 1920x1080 frame with 5 windows and report full-frame and small-damage
  times on the host.

## Tests and specimens (host)
Unit tests: stacking and focus rules, hit testing incl. corners and resize handles, drag/resize
sequences, minimise/restore via the taskbar, Alt+Tab order, damage correctness (a randomized test
comparing incremental composition against a full recomposition pixel for pixel over 500 random
operations). Specimens in artifacts/wm-specimens/: (1) desktop with three overlapping windows
(terminal-like, editor-like, chat-like placeholder contents), focused window in front with the
stronger shadow, taskbar and cursor visible; (2) one window maximised; (3) mid-drag state.

## Acceptance
`scripts/verify-crate.sh otter-wm` all clear with >= 40 tests; the three specimens exist.
