# Brief M4-T4c — desktop polish: the real apps inside otter-wm, to the Kelp design

## Goal
The desktop specimen must look like the finished product in a demo video: the actual Terminal,
Editor, Otter and Files apps (crates otter-apps and otter-apps-ai) composed inside otter-wm windows,
exactly to docs/DESIGN.md. The orchestrator judges the PNGs at native resolution against
docs/DESIGN.md; "looks roughly right" is not accepted.

## Applies
D24 (visual quality bar), D27. docs/DESIGN.md is the specification for every colour, size and
radius below.

## Problems in the current specimen (artifacts/keep/wm-desktop.png) to fix
1. The dock spans the full width with tiny left-aligned icons. Per DESIGN it is centred and as
   wide as its content, floats 10 px above the bottom edge, 14 px radius, `chrome` at 92 %, 40 px
   app icons with 8 px gaps, running-app indicator dots, the Otter button on the left at the same
   40 px size in `accent` with a drawn otter glyph, and the clock on the right inside the dock.
2. The terminal window is light; it must use `surface.dark`, `text.inverse`, JetBrains Mono 14 px,
   and the prompt in `accent`.
3. Title texts are lower-case placeholders. Use "Terminal", "Editor — notes.txt", "Otter", "Files"
   in Inter semibold 14 px, centred; unfocused title text in `text.muted`.
4. The chat bubbles use white text on amber, and the text sits at the top of the bubble. Text
   colour must be chosen by contrast, with ≥ 4.5:1 WCAG contrast for every text/background pair the
   theme can produce (add a test that enumerates them). Use 8/12 px padding, vertically centred
   text, 12 px radius and a maximum width of 70 % of the window.
5. The window contents are placeholders. Render the real apps into the window surfaces: Terminal
   running a short `otsh` session; Editor with a Rust file, line numbers and the status bar; Otter
   with a streamed answer, a thinking line and a pending `write_file` tool card with Allow/Deny;
   Files listing a directory.
6. The cursor must be a crisp arrow, 20 px tall, with a dark fill and a white 1.5 px outline,
   anti-aliased, hotspot at the tip, always on top.
7. Add a system modal: the agent-mode Allow/Deny dialog from DESIGN.md "Components" (centred,
   `surface`, the action in monospace, Allow in `ok`, Deny in `danger`, Enter/Esc hints) over a
   dimmed backdrop, drawn above all windows.

## Tests and specimens (host; `cargo test -p otter-wm`)
- Composition: 500 seeded random operations (open, close, move, resize, raise, minimise,
  maximise, focus change, app content damage). The incremental, damage-tracked result must be
  pixel-identical to a full recomposition after every 50 operations.
- The contrast test from item 4.
- Specimens written by a test into crates/otter-wm/artifacts/: `desktop-1280x800.png` and
  `desktop-1920x1080.png` (four overlapping app windows, focused Otter window, the dock, the
  cursor), and `dialog-1280x800.png` (the modal from item 7). otter-apps and otter-apps-ai may be
  dev-dependencies for rendering the real apps.

## Acceptance
`scripts/verify-crate.sh otter-wm` all clear. `scripts/verify-crate.sh otter-apps` and
`scripts/verify-crate.sh otter-apps-ai` still pass if you touched them. The three PNGs exist at
the stated sizes.

## Report
<=10 lines: the verify-crate lines, the specimen paths, anything in the list you could not do.
Do not commit.
