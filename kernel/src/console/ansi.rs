//! Minimal ANSI/VT escape-sequence subset (brief M1-T7): `ESC[0m` reset,
//! `ESC[1m` bold (bright foreground), `ESC[30-37m`/`ESC[90-97m`
//! foreground, `ESC[40-47m` background, `ESC[2J` clear, `ESC[H` home.
//!
//! `Parser` is a small state machine fed one byte at a time
//! (`Console::write_bytes` feeds it every byte of every `write_str`
//! call), so a sequence split across two calls -- e.g. `kprint!("\x1b")`
//! immediately followed by `kprint!("[31m")` -- still parses correctly:
//! the mid-sequence state (`State::Escape`/`State::Csi` plus whatever
//! parameters have been read so far) lives in `self`, not on any call's
//! stack. Any byte that doesn't fit this subset's grammar (an
//! unrecognised `ESC` follow-up, an unsupported SGR code, a CSI sequence
//! with an unsupported final byte) is consumed and dropped rather than
//! ever falling through to be printed as garbage -- "unknown sequences
//! are ignored safely" (brief M1-T7 step 2).

use super::cell::Rgb;

pub const DEFAULT_FG: Rgb = (0xe0, 0xe0, 0xe0);
pub const DEFAULT_BG: Rgb = (0x10, 0x14, 0x1c);
/// What bold (code 1) promotes the *default* foreground to when no
/// explicit 30-37 colour is active -- pure white against the light-grey
/// default, so `ESC[1m` alone (e.g. `main.rs`'s title line) is visibly
/// distinct without requiring a colour code to pair it with.
const BRIGHT_DEFAULT_FG: Rgb = (0xff, 0xff, 0xff);

/// The 8-colour ANSI palette (codes 30-37 / 40-47), a fairly standard
/// "CGA-ish" set: dim enough to read comfortably on the dark default
/// background, saturated enough to tell apart at a glance.
const PALETTE: [Rgb; 8] = [
    (0x00, 0x00, 0x00), // 0 black
    (0xaa, 0x00, 0x00), // 1 red
    (0x00, 0xaa, 0x00), // 2 green
    (0xaa, 0xaa, 0x00), // 3 yellow
    (0x00, 0x00, 0xaa), // 4 blue
    (0xaa, 0x00, 0xaa), // 5 magenta
    (0x00, 0xaa, 0xaa), // 6 cyan
    (0xaa, 0xaa, 0xaa), // 7 white
];

/// The "bright" variants codes 90-97 select directly, and that bold
/// (code 1) promotes a 30-37 foreground to.
const PALETTE_BRIGHT: [Rgb; 8] = [
    (0x55, 0x55, 0x55),
    (0xff, 0x55, 0x55),
    (0x55, 0xff, 0x55),
    (0xff, 0xff, 0x55),
    (0x55, 0x55, 0xff),
    (0xff, 0x55, 0xff),
    (0x55, 0xff, 0xff),
    (0xff, 0xff, 0xff),
];

/// Current text attributes an SGR (`ESC[...m`) sequence can change;
/// persists across characters (and across separate `write_str` calls)
/// until the next one changes it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Attrs {
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
}

impl Attrs {
    pub const fn default_attrs() -> Self {
        Self { fg: DEFAULT_FG, bg: DEFAULT_BG, bold: false }
    }
}

/// What `Console::feed_byte` should do with one input byte, once any
/// escape-sequence framing around it has been resolved.
pub enum Action {
    /// Not part of an escape sequence: hand it to the console as-is.
    /// `Console` (not this parser) decides what a control byte like
    /// `\n`/`\t`/backspace does; everything else in this subset is only
    /// ever consumed, never emitted.
    Emit(u8),
    /// An `ESC[...m` sequence completed. `Parser::attrs` already reflects
    /// the change; the console doesn't need the value repeated here, only
    /// the fact that this byte's job was applying it, not printing it.
    SetAttrs,
    /// `ESC[2J` completed.
    ClearScreen,
    /// `ESC[H` completed.
    Home,
    /// `ESC[K` completed (erase to end of line).
    EraseToEndOfLine,
    /// `ESC[<n>C` completed (cursor right n columns, default 1).
    CursorRight(u16),
    /// `ESC[<n>D` completed (cursor left n columns, default 1).
    CursorLeft(u16),
    /// `ESC[<n>G` completed (cursor to column n, default 1).
    CursorToColumn(u16),
    /// A byte was consumed as part of an in-progress (or now-complete but
    /// unsupported) escape sequence; nothing for the console to do.
    None,
}

enum State {
    Normal,
    /// Just saw `ESC` (0x1b); the very next byte decides whether this is
    /// a CSI sequence (`[`) or something outside this subset.
    Escape,
    /// `ESC[` seen; collecting `;`-separated decimal parameters until a
    /// final byte in 0x40..=0x7e (ECMA-48's CSI terminator range).
    Csi,
}

/// Parameters a single CSI sequence can carry before extras are parsed
/// (so the final byte is still found correctly) but otherwise dropped.
/// This kernel's own log lines never chain anywhere near this many SGR
/// codes in one `ESC[...m`.
const MAX_PARAMS: usize = 6;

pub struct Parser {
    state: State,
    params: [u16; MAX_PARAMS],
    count: usize,
    attrs: Attrs,
}

impl Parser {
    pub const fn new() -> Self {
        Self { state: State::Normal, params: [0; MAX_PARAMS], count: 0, attrs: Attrs::default_attrs() }
    }

    /// The attributes the most recently completed `ESC[...m` (if any)
    /// left in effect; `Console` draws every subsequent glyph with these
    /// until the next one changes them.
    pub fn attrs(&self) -> Attrs {
        self.attrs
    }

    /// Overrides the current attributes directly, bypassing SGR parsing
    /// entirely (`console::panic_print`: the panic message is always
    /// bright red-on-default, regardless of whatever attributes were
    /// active when the panic happened).
    pub fn set_attrs(&mut self, attrs: Attrs) {
        self.attrs = attrs;
    }

    pub fn feed(&mut self, byte: u8) -> Action {
        match self.state {
            State::Normal => self.feed_normal(byte),
            State::Escape => self.feed_escape(byte),
            State::Csi => self.feed_csi(byte),
        }
    }

    fn feed_normal(&mut self, byte: u8) -> Action {
        if byte == 0x1b {
            self.state = State::Escape;
            Action::None
        } else {
            Action::Emit(byte)
        }
    }

    fn feed_escape(&mut self, byte: u8) -> Action {
        if byte == b'[' {
            self.params = [0; MAX_PARAMS];
            self.count = 0;
            self.state = State::Csi;
        } else {
            // Nothing else in this subset is recognised; swallow the one
            // byte following `ESC` and get out of the way rather than
            // printing it (or, worse, getting stuck waiting for a `[`
            // that will never come).
            self.state = State::Normal;
        }
        Action::None
    }

    fn feed_csi(&mut self, byte: u8) -> Action {
        match byte {
            b'0'..=b'9' => {
                if self.count < MAX_PARAMS {
                    let digit = u16::from(byte - b'0');
                    self.params[self.count] = self.params[self.count].saturating_mul(10).saturating_add(digit);
                }
                Action::None
            }
            b';' => {
                if self.count + 1 < MAX_PARAMS {
                    self.count += 1;
                }
                Action::None
            }
            0x40..=0x7e => {
                let n_params = self.count + 1;
                self.state = State::Normal;
                self.dispatch(byte, n_params)
            }
            _ => {
                // Not a valid parameter/intermediate byte for this
                // subset (brief: "unknown sequences are ignored
                // safely") -- bail out cleanly instead of staying stuck
                // in `Csi` waiting for a final byte that fits a grammar
                // we don't speak.
                self.state = State::Normal;
                Action::None
            }
        }
    }

    fn dispatch(&mut self, final_byte: u8, n_params: usize) -> Action {
        match final_byte {
            b'm' => {
                if n_params == 0 {
                    // Bare `ESC[m` is shorthand for `ESC[0m`.
                    self.attrs = Attrs::default_attrs();
                } else {
                    for i in 0..n_params {
                        self.apply_sgr(self.params[i]);
                    }
                }
                Action::SetAttrs
            }
            b'J' if n_params >= 1 && self.params[0] == 2 => Action::ClearScreen,
            b'H' => Action::Home,
            b'K' => Action::EraseToEndOfLine,
            b'C' => {
                let n = if n_params >= 1 && self.params[0] > 0 { self.params[0] } else { 1 };
                Action::CursorRight(n)
            }
            b'D' => {
                let n = if n_params >= 1 && self.params[0] > 0 { self.params[0] } else { 1 };
                Action::CursorLeft(n)
            }
            b'G' => {
                let n = if n_params >= 1 && self.params[0] > 0 { self.params[0] } else { 1 };
                Action::CursorToColumn(n)
            }
            _ => Action::None,
        }
    }

    fn apply_sgr(&mut self, code: u16) {
        match code {
            0 => self.attrs = Attrs::default_attrs(),
            1 => {
                self.attrs.bold = true;
                // No explicit colour active yet: promote the default
                // foreground itself, so bold-alone still visibly does
                // something (brief M1-T7: "ESC[1m bold (bright fg)").
                if self.attrs.fg == DEFAULT_FG {
                    self.attrs.fg = BRIGHT_DEFAULT_FG;
                }
            }
            30..=37 => {
                let i = (code - 30) as usize;
                self.attrs.fg = if self.attrs.bold { PALETTE_BRIGHT[i] } else { PALETTE[i] };
            }
            90..=97 => self.attrs.fg = PALETTE_BRIGHT[(code - 90) as usize],
            40..=47 => self.attrs.bg = PALETTE[(code - 40) as usize],
            _ => {} // Unsupported SGR code: ignored, not an error.
        }
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}
