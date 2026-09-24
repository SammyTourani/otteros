# Brief M4-T4a — `otter-ui`: widget toolkit (pure crate, rendered with otter-gfx)

## Goal
The toolkit every OtterOS app uses: a small retained widget tree with layout, input handling and
the Kelp theme from docs/DESIGN.md, rendered into an `otter-gfx` Surface. Pure and host-tested;
the display-server connection is added later.

## Applies
D2, D24, D27, docs/DESIGN.md (follow its tokens exactly). Path dependencies: otter-gfx (drawing,
text), otter-textbuf (the text field and multi-line editor view), otter-term (terminal view).

## Design
- Widgets: Label, Button (primary/secondary/danger), TextField (single line, cursor, selection,
  placeholder), TextArea (multi-line view over otter-textbuf with scrolling and line numbers
  option), ScrollView, List (selectable rows, keyboard navigation), Checkbox, ProgressBar,
  TerminalView (draws an otter-term screen grid in JetBrains Mono with the 16-colour palette),
  Image (from an otter-gfx Surface), Spacer, and containers Column/Row with padding, spacing,
  alignment and flexible children.
- Layout: two-pass measure/arrange with min/preferred sizes; text measured with otter-gfx.
- Events: pointer move/down/up, wheel, key (with modifiers and text), focus traversal with Tab,
  hover and pressed states; widgets return actions (Clicked(id), TextChanged(id), Submitted(id)).
- Theme: colours, radii, spacing, fonts exactly from docs/DESIGN.md; focus ring in accent.
- Rendering: `Ui::render(&mut Surface)` with damage tracking per widget.

## Tests and specimens (host)
Unit tests for layout (sizes, alignment, flex), hit testing, focus traversal, TextField editing
through events, List keyboard navigation, button hover/press state transitions. Specimen PNGs
written to artifacts/ui-specimens/: (1) a form with labels, fields (one focused with a cursor and
a selection), checkboxes and three button kinds; (2) a list of files with one selected row;
(3) a chat window mock: alternating message bubbles (user in accent, Otter on surface.dark), a
text field and Send button; (4) a TerminalView showing the recorded shell session fixture from
crates/otter-term; (5) a TextArea editing Rust code with line numbers. The orchestrator will judge
them against docs/DESIGN.md.

## Acceptance
`scripts/verify-crate.sh otter-ui` all clear with >= 40 tests; the five specimens exist.
