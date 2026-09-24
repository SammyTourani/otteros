# Brief M4-T5a — `otter-term`: terminal emulator core (pure crate)

## Goal
The engine of the desktop Terminal app: consume the byte stream a shell writes, maintain the
screen grid, cursor, attributes and scrollback, and produce the bytes to send for key presses.
Rendering is someone else's job; this crate is pure state, tested on the Mac.

## Applies
D2, D27 (pure no_std + alloc, zero external dependencies).

## Design
- Screen: cols × rows grid of cells {char (Unicode scalar), fg, bg, bold, underline, inverse,
  dirty}, cursor (row, col, visible), saved cursor, scroll region, scrollback ring (default 5,000
  lines), resize that reflows nothing but clamps the cursor and keeps content.
- Parser: UTF-8 decoding across chunk boundaries (invalid → U+FFFD); C0 controls (BS, HT with
  8-column stops, LF, CR, BEL ignored, NUL ignored); ESC sequences: ESC 7/8 save/restore, ESC c
  reset, ESC D/M/E index/reverse index/next line; CSI with parameters and private `?`: CUU/CUD/
  CUF/CUB (A/B/C/D), CNL/CPL (E/F), CHA (G), CUP/HVP (H/f), ED (J 0/1/2/3), EL (K 0/1/2), IL/DL (L/M),
  ICH/DCH (@/P), ECH (X), SU/SD (S/T), DECSTBM (r), SGR (m: 0, 1, 4, 7, 22, 24, 27, 30–37, 39, 40–47,
  49, 90–97, 100–107, 38;5;n and 48;5;n mapped to the 16-colour palette, 38;2;r;g;b kept as RGB),
  DECTCEM (?25 h/l), DSR 6n (reply with CPR), alternate screen (?1049 h/l). Unknown sequences are
  consumed and ignored; OSC strings (ESC ] … BEL or ST) consumed and ignored except OSC 0/2 title.
- Keys to bytes: printable characters as UTF-8, Enter `\r`, Backspace 0x7F, Tab, arrows as
  ESC[A–D, Home/End ESC[H/ESC[F, Delete ESC[3~, PageUp/PageDown ESC[5~/ESC[6~, Ctrl+letter.
- Damage API: which rows changed since the last `take_damage()`.

## Tests (host, >= 50)
Every sequence above; wrap at the right margin (deferred wrap semantics like xterm); scrolling
within a region; scrollback growth and cap; alternate screen restore; UTF-8 split at every byte;
a recorded session fixture (a script that replays the OtterOS shell's actual output bytes from a
`gmake shell-test` serial log) rendered to a text snapshot that matches expected lines; malformed
input fuzz never panics.

## Acceptance
`scripts/verify-crate.sh otter-term` shows 0 failed, 0 ignored, clippy ok, no_std ok, 0 external deps.
