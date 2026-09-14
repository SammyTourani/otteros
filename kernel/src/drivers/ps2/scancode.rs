//! Scancode Set 1 decoding (brief M1-T6): the set the i8042 hands us once
//! `i8042::init` turns translation on, regardless of what scancode set
//! the physical (or QEMU-emulated) keyboard natively speaks. `Decoder` is
//! a small state machine -- one `0xE0` extended-prefix bit of memory,
//! plus the currently-held modifiers -- fed one raw byte at a time.

/// A key identity, independent of what character (if any) it currently
/// produces -- `KeyEvent::to_char` applies modifiers on top of this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    Minus, Equal, LeftBracket, RightBracket, Semicolon, Apostrophe, Grave, Backslash, Comma, Period, Slash,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    Left, Right, Up, Down, Home, End, PageUp, PageDown, Insert, Delete,
    Enter, Backspace, Tab, Escape, Space,
    LeftShift, RightShift, LeftCtrl, RightCtrl, LeftAlt, RightAlt,
    CapsLock, NumLock, ScrollLock,
    /// A make code (not a break: the high bit and any `0xE0` prefix are
    /// already stripped) this decoder doesn't assign any of the above to.
    Unknown(u8),
}

/// Whether a key's unshifted/shifted characters are a cased letter pair
/// (caps lock also flips it) or a fixed symbol pair (only shift flips
/// it) -- see `Key::chars` and `KeyEvent::to_char`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharKind {
    Letter,
    Symbol,
}

impl Key {
    /// The `(unshifted, shifted, kind)` this key types on a US QWERTY
    /// layout, or `None` for a key that never produces a character
    /// (arrows, function keys, modifiers, locks, `Unknown`).
    fn chars(self) -> Option<(char, char, CharKind)> {
        use CharKind::{Letter, Symbol};
        Some(match self {
            Key::A => ('a', 'A', Letter), Key::B => ('b', 'B', Letter), Key::C => ('c', 'C', Letter),
            Key::D => ('d', 'D', Letter), Key::E => ('e', 'E', Letter), Key::F => ('f', 'F', Letter),
            Key::G => ('g', 'G', Letter), Key::H => ('h', 'H', Letter), Key::I => ('i', 'I', Letter),
            Key::J => ('j', 'J', Letter), Key::K => ('k', 'K', Letter), Key::L => ('l', 'L', Letter),
            Key::M => ('m', 'M', Letter), Key::N => ('n', 'N', Letter), Key::O => ('o', 'O', Letter),
            Key::P => ('p', 'P', Letter), Key::Q => ('q', 'Q', Letter), Key::R => ('r', 'R', Letter),
            Key::S => ('s', 'S', Letter), Key::T => ('t', 'T', Letter), Key::U => ('u', 'U', Letter),
            Key::V => ('v', 'V', Letter), Key::W => ('w', 'W', Letter), Key::X => ('x', 'X', Letter),
            Key::Y => ('y', 'Y', Letter), Key::Z => ('z', 'Z', Letter),
            Key::Digit1 => ('1', '!', Symbol), Key::Digit2 => ('2', '@', Symbol), Key::Digit3 => ('3', '#', Symbol),
            Key::Digit4 => ('4', '$', Symbol), Key::Digit5 => ('5', '%', Symbol), Key::Digit6 => ('6', '^', Symbol),
            Key::Digit7 => ('7', '&', Symbol), Key::Digit8 => ('8', '*', Symbol), Key::Digit9 => ('9', '(', Symbol),
            Key::Digit0 => ('0', ')', Symbol),
            Key::Minus => ('-', '_', Symbol), Key::Equal => ('=', '+', Symbol),
            Key::LeftBracket => ('[', '{', Symbol), Key::RightBracket => (']', '}', Symbol),
            Key::Semicolon => (';', ':', Symbol), Key::Apostrophe => ('\'', '"', Symbol),
            Key::Grave => ('`', '~', Symbol), Key::Backslash => ('\\', '|', Symbol),
            Key::Comma => (',', '<', Symbol), Key::Period => ('.', '>', Symbol), Key::Slash => ('/', '?', Symbol),
            Key::Space => (' ', ' ', Symbol),
            Key::Enter => ('\n', '\n', Symbol), Key::Tab => ('\t', '\t', Symbol), Key::Backspace => ('\u{8}', '\u{8}', Symbol),
            _ => return None,
        })
    }
}

/// Modifier keys' current state, tracked from make (press) / break
/// (release) codes as they're decoded. `caps_lock` toggles on the make
/// code only (a physical caps-lock press toggles it once; its own break
/// code does nothing to it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub caps_lock: bool,
}

/// One decoded make or break event, with the modifier state as of right
/// after this event was applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    /// `true` for a make code (key pressed/repeated), `false` for a break
    /// code (key released).
    pub pressed: bool,
    pub mods: Modifiers,
}

impl KeyEvent {
    /// The US-QWERTY character this event types, honouring shift and
    /// caps lock -- `None` for a break event, or for a key that never
    /// produces a character at all (arrows, function keys, modifiers,
    /// locks). Caps lock only affects cased letters (it XORs with
    /// shift); shift alone picks the shifted symbol for everything else.
    pub fn to_char(&self) -> Option<char> {
        if !self.pressed {
            return None;
        }
        let (unshifted, shifted, kind) = self.key.chars()?;
        let use_shifted = match kind {
            CharKind::Letter => self.mods.shift ^ self.mods.caps_lock,
            CharKind::Symbol => self.mods.shift,
        };
        Some(if use_shifted { shifted } else { unshifted })
    }
}

/// Decodes a non-extended (no `0xE0` prefix) Set 1 make code (high bit
/// already stripped).
fn decode_key(code: u8) -> Key {
    match code {
        0x01 => Key::Escape,
        0x02 => Key::Digit1,
        0x03 => Key::Digit2,
        0x04 => Key::Digit3,
        0x05 => Key::Digit4,
        0x06 => Key::Digit5,
        0x07 => Key::Digit6,
        0x08 => Key::Digit7,
        0x09 => Key::Digit8,
        0x0A => Key::Digit9,
        0x0B => Key::Digit0,
        0x0C => Key::Minus,
        0x0D => Key::Equal,
        0x0E => Key::Backspace,
        0x0F => Key::Tab,
        0x10 => Key::Q,
        0x11 => Key::W,
        0x12 => Key::E,
        0x13 => Key::R,
        0x14 => Key::T,
        0x15 => Key::Y,
        0x16 => Key::U,
        0x17 => Key::I,
        0x18 => Key::O,
        0x19 => Key::P,
        0x1A => Key::LeftBracket,
        0x1B => Key::RightBracket,
        0x1C => Key::Enter,
        0x1D => Key::LeftCtrl,
        0x1E => Key::A,
        0x1F => Key::S,
        0x20 => Key::D,
        0x21 => Key::F,
        0x22 => Key::G,
        0x23 => Key::H,
        0x24 => Key::J,
        0x25 => Key::K,
        0x26 => Key::L,
        0x27 => Key::Semicolon,
        0x28 => Key::Apostrophe,
        0x29 => Key::Grave,
        0x2A => Key::LeftShift,
        0x2B => Key::Backslash,
        0x2C => Key::Z,
        0x2D => Key::X,
        0x2E => Key::C,
        0x2F => Key::V,
        0x30 => Key::B,
        0x31 => Key::N,
        0x32 => Key::M,
        0x33 => Key::Comma,
        0x34 => Key::Period,
        0x35 => Key::Slash,
        0x36 => Key::RightShift,
        0x38 => Key::LeftAlt,
        0x39 => Key::Space,
        0x3A => Key::CapsLock,
        0x3B => Key::F1,
        0x3C => Key::F2,
        0x3D => Key::F3,
        0x3E => Key::F4,
        0x3F => Key::F5,
        0x40 => Key::F6,
        0x41 => Key::F7,
        0x42 => Key::F8,
        0x43 => Key::F9,
        0x44 => Key::F10,
        0x45 => Key::NumLock,
        0x46 => Key::ScrollLock,
        0x57 => Key::F11,
        0x58 => Key::F12,
        other => Key::Unknown(other),
    }
}

/// Decodes an `0xE0`-prefixed (extended) Set 1 make code (high bit
/// already stripped) -- just the dedicated arrow/navigation cluster and
/// the right-hand Ctrl/Alt this kernel actually cares about; anything
/// else extended (keypad Enter/`/`, ACPI power keys, ...) decodes as
/// `Unknown`.
fn decode_extended_key(code: u8) -> Key {
    match code {
        0x1D => Key::RightCtrl,
        0x38 => Key::RightAlt,
        0x47 => Key::Home,
        0x48 => Key::Up,
        0x49 => Key::PageUp,
        0x4B => Key::Left,
        0x4D => Key::Right,
        0x4F => Key::End,
        0x50 => Key::Down,
        0x51 => Key::PageDown,
        0x52 => Key::Insert,
        0x53 => Key::Delete,
        other => Key::Unknown(other),
    }
}

/// The scancode Set 1 state machine: `0xE0`-prefix tracking plus the
/// currently-held modifiers, fed one raw byte (as read from the i8042
/// data port) at a time.
pub struct Decoder {
    modifiers: Modifiers,
    /// Set by a bare `0xE0` byte, consumed by the very next byte fed.
    pending_extended: bool,
}

impl Decoder {
    /// A fresh decoder: no modifiers held, no pending prefix. `const fn`
    /// so it can back a `static` directly (e.g. behind an `IrqMutex`).
    pub const fn new() -> Self {
        Self { modifiers: Modifiers { shift: false, ctrl: false, alt: false, caps_lock: false }, pending_extended: false }
    }

    fn apply_modifier(&mut self, key: Key, pressed: bool) {
        match key {
            Key::LeftShift | Key::RightShift => self.modifiers.shift = pressed,
            Key::LeftCtrl | Key::RightCtrl => self.modifiers.ctrl = pressed,
            Key::LeftAlt | Key::RightAlt => self.modifiers.alt = pressed,
            Key::CapsLock if pressed => self.modifiers.caps_lock = !self.modifiers.caps_lock,
            _ => {}
        }
    }

    /// Feeds one raw byte from the data port. Returns a decoded
    /// `KeyEvent` once a full make/break code (possibly `0xE0`-prefixed)
    /// has been consumed, or `None` if this byte only extended the
    /// pending sequence (the `0xE0` prefix itself), or is one this
    /// decoder deliberately never turns into a key event at all (see
    /// below).
    pub fn feed(&mut self, byte: u8) -> Option<KeyEvent> {
        if byte == 0xE0 {
            self.pending_extended = true;
            return None;
        }
        if byte == 0xE1 || byte == 0xFA || byte == 0xFE {
            // 0xE1: the Pause/Break key's non-standard 6-byte sequence --
            // not decoded (nothing in `Key` needs it); resynchronizes on
            // the very next byte instead of waiting forever for a
            // prefix-completion that this decoder never produces.
            // 0xFA/0xFE: a device ACK/Resend byte, e.g. from
            // `i8042::set_leds`'s Set-LEDs command once IRQ1 is already
            // live (see that function's own docs) -- never a real
            // scancode, so it's swallowed here rather than misdecoded as
            // some unrelated key's make/break code.
            self.pending_extended = false;
            return None;
        }

        let extended = core::mem::take(&mut self.pending_extended);
        let pressed = byte & 0x80 == 0;
        let code = byte & 0x7F;
        let key = if extended { decode_extended_key(code) } else { decode_key(code) };

        self.apply_modifier(key, pressed);

        Some(KeyEvent { key, pressed, mods: self.modifiers })
    }
}

// See `ring::SpscRing`'s identical `Default` impl for why this forwards
// to `new()` rather than `new()` itself satisfying clippy's
// `new_without_default`: `new()` is `const fn` (for `static DECODER`,
// keyboard.rs) and `Default::default` can't be.
impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}
