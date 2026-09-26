//! Acceptance oracle for brief M4-T7a (otter-wire: the display protocol v1), written by the
//! orchestrator. The crate must pass this file unchanged. The display server and every desktop
//! client share this crate; the byte layouts pinned here ARE the protocol (DECISIONS D10: clients
//! draw into shared-memory buffers and talk to the server over message channels, M4-T3).
//!
//! Wire format: one message per channel message (channels keep boundaries). Little-endian.
//! Header: kind u16, then the fields of that kind in the order listed, no padding. `str` = u16
//! byte length + UTF-8 bytes (at most 256 bytes). Buffers travel as channel handles next to the
//! message bytes, never inside them. A decoder rejects: fewer bytes than the kind needs
//! (Truncated), bytes left over (Trailing), an unknown kind (UnknownKind), invalid UTF-8
//! (BadUtf8), a string over 256 bytes (TooLong), an enum value out of range (BadValue).
//!
//! Client -> server (`ClientMsg`):
//!    1 Hello { version: u16 }
//!    2 CreateWindow { req: u32, width: u16, height: u16, stride: u32, flags: u32, title: str }
//!      (+1 handle: the shm pixel buffer, stride * height bytes, BGRA8888 premultiplied)
//!    3 Damage { window: u32, x: u16, y: u16, w: u16, h: u16 }
//!    4 SetTitle { window: u32, title: str }
//!    5 Close { window: u32 }
//!    6 AttachBuffer { window: u32, width: u16, height: u16, stride: u32 } (+1 handle, after Configure)
//!    7 SetCursor { shape: Cursor }  (u8: 0 Arrow, 1 Text, 2 Hand, 3 ResizeH, 4 ResizeV, 5 Busy)
//! Server -> client (`ServerMsg`):
//!  101 Welcome { version: u16, screen_w: u16, screen_h: u16 }
//!  102 WindowCreated { req: u32, window: u32 }
//!  103 Configure { window: u32, width: u16, height: u16 }
//!  104 Focus { window: u32, focused: bool }            (bool: u8 0/1, other values BadValue)
//!  105 Key { window: u32, code: u16, pressed: bool, mods: u8, ch: u32 }   (ch: a char or 0; not a
//!      Unicode scalar value -> BadValue)
//!  106 Pointer { window: u32, x: i16, y: i16, buttons: u8 }
//!  107 Button { window: u32, button: u8, pressed: bool, x: i16, y: i16 }
//!  108 Wheel { window: u32, delta: i16 }
//!  109 CloseRequested { window: u32 }
//!  110 Error { req: u32, code: u16, message: str }
//!
//! API: `ClientMsg::encode(&self) -> Result<Vec<u8>, WireError>` (TooLong for long strings),
//! `ClientMsg::decode(&[u8]) -> Result<ClientMsg, WireError>`, the same for `ServerMsg`;
//! `ClientMsg::handles(&self) -> usize` (how many handles travel with it); constants
//! `PROTOCOL_VERSION = 1`, `MAX_STR = 256`, flag bits `FLAG_RESIZABLE = 1`, `FLAG_UNDECORATED = 2`,
//! `FLAG_DIALOG = 4`. All types derive Debug, Clone, PartialEq, Eq.

use otter_wire::*;

#[test]
fn golden_bytes() {
    assert_eq!(ClientMsg::Hello { version: 1 }.encode().unwrap(), [1, 0, 1, 0]);
    let create = ClientMsg::CreateWindow { req: 7, width: 640, height: 480, stride: 2560, flags: FLAG_RESIZABLE, title: "Otter".into() };
    assert_eq!(
        create.encode().unwrap(),
        [2, 0, 7, 0, 0, 0, 0x80, 0x02, 0xE0, 0x01, 0x00, 0x0A, 0, 0, 1, 0, 0, 0, 5, 0, b'O', b't', b't', b'e', b'r']
    );
    assert_eq!(create.handles(), 1);
    assert_eq!(ClientMsg::Damage { window: 3, x: 1, y: 2, w: 300, h: 40 }.encode().unwrap(), [3, 0, 3, 0, 0, 0, 1, 0, 2, 0, 0x2C, 0x01, 40, 0]);
    assert_eq!(ClientMsg::SetCursor { shape: Cursor::Hand }.encode().unwrap(), [7, 0, 2]);
    assert_eq!(ServerMsg::Welcome { version: 1, screen_w: 1280, screen_h: 800 }.encode().unwrap(), [101, 0, 1, 0, 0x00, 0x05, 0x20, 0x03]);
    assert_eq!(
        ServerMsg::Key { window: 2, code: 0x1E, pressed: true, mods: 0x02, ch: 'A' as u32 }.encode().unwrap(),
        [105, 0, 2, 0, 0, 0, 0x1E, 0, 1, 0x02, 0x41, 0, 0, 0]
    );
    assert_eq!(ServerMsg::Pointer { window: 1, x: -5, y: 300, buttons: 1 }.encode().unwrap(), [106, 0, 1, 0, 0, 0, 0xFB, 0xFF, 0x2C, 0x01, 1]);
    assert_eq!(
        ServerMsg::Error { req: 9, code: 4, message: "no".into() }.encode().unwrap(),
        [110, 0, 9, 0, 0, 0, 4, 0, 2, 0, b'n', b'o']
    );
    for m in [ClientMsg::Hello { version: 1 }, ClientMsg::Close { window: 1 }, ClientMsg::SetTitle { window: 1, title: "x".into() }] {
        assert_eq!(m.handles(), 0);
    }
    assert_eq!(ClientMsg::AttachBuffer { window: 1, width: 2, height: 3, stride: 8 }.handles(), 1);
}

fn xorshift(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

fn text(x: &mut u64) -> String {
    let pool = ["", "Otter", "Terminal", "résumé.txt", "🦦 Chat", "a b c", "日本語"];
    let mut s = String::from(pool[(xorshift(x) % pool.len() as u64) as usize]);
    for _ in 0..xorshift(x) % 4 {
        s.push_str(pool[(xorshift(x) % pool.len() as u64) as usize]);
    }
    s
}

fn random_client(x: &mut u64) -> ClientMsg {
    let r = xorshift(x);
    match r % 7 {
        0 => ClientMsg::Hello { version: r as u16 },
        1 => ClientMsg::CreateWindow { req: r as u32, width: (r >> 8) as u16, height: (r >> 24) as u16, stride: (r >> 16) as u32, flags: (r >> 40) as u32 & 7, title: text(x) },
        2 => ClientMsg::Damage { window: r as u32, x: (r >> 3) as u16, y: (r >> 13) as u16, w: (r >> 23) as u16, h: (r >> 33) as u16 },
        3 => ClientMsg::SetTitle { window: (r >> 7) as u32, title: text(x) },
        4 => ClientMsg::Close { window: (r >> 9) as u32 },
        5 => ClientMsg::AttachBuffer { window: r as u32, width: (r >> 32) as u16, height: (r >> 48) as u16, stride: (r >> 11) as u32 },
        _ => ClientMsg::SetCursor { shape: [Cursor::Arrow, Cursor::Text, Cursor::Hand, Cursor::ResizeH, Cursor::ResizeV, Cursor::Busy][(r % 6) as usize] },
    }
}

fn random_server(x: &mut u64) -> ServerMsg {
    let r = xorshift(x);
    let ch = ['\0', 'a', 'é', '🦦', '\u{FFFD}'][(r % 5) as usize] as u32;
    match r % 10 {
        0 => ServerMsg::Welcome { version: r as u16, screen_w: (r >> 16) as u16, screen_h: (r >> 32) as u16 },
        1 => ServerMsg::WindowCreated { req: r as u32, window: (r >> 32) as u32 },
        2 => ServerMsg::Configure { window: r as u32, width: (r >> 32) as u16, height: (r >> 48) as u16 },
        3 => ServerMsg::Focus { window: r as u32, focused: r & 1 == 1 },
        4 => ServerMsg::Key { window: r as u32, code: (r >> 32) as u16, pressed: r & 2 == 2, mods: (r >> 48) as u8, ch },
        5 => ServerMsg::Pointer { window: r as u32, x: (r >> 32) as i16, y: (r >> 48) as i16, buttons: (r >> 8) as u8 },
        6 => ServerMsg::Button { window: r as u32, button: (r >> 40) as u8, pressed: r & 4 == 4, x: (r >> 16) as i16, y: (r >> 48) as i16 },
        7 => ServerMsg::Wheel { window: r as u32, delta: (r >> 32) as i16 },
        8 => ServerMsg::CloseRequested { window: r as u32 },
        _ => ServerMsg::Error { req: r as u32, code: (r >> 32) as u16, message: text(x) },
    }
}

#[test]
fn every_message_round_trips() {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..5000 {
        let c = random_client(&mut x);
        match c.encode() {
            Ok(bytes) => assert_eq!(ClientMsg::decode(&bytes), Ok(c.clone()), "{c:?}"),
            Err(e) => assert_eq!(e, WireError::TooLong, "{c:?}"),
        }
        let s = random_server(&mut x);
        match s.encode() {
            Ok(bytes) => assert_eq!(ServerMsg::decode(&bytes), Ok(s.clone()), "{s:?}"),
            Err(e) => assert_eq!(e, WireError::TooLong, "{s:?}"),
        }
    }
}

#[test]
fn strings_are_bounded() {
    let ok = ClientMsg::SetTitle { window: 1, title: "t".repeat(256) };
    assert!(ok.encode().is_ok(), "256 bytes is the limit, inclusive");
    let long = ClientMsg::SetTitle { window: 1, title: "t".repeat(257) };
    assert_eq!(long.encode(), Err(WireError::TooLong));
    let mut forged = vec![4, 0, 1, 0, 0, 0, 0x01, 0x01]; // SetTitle claiming 257 bytes
    forged.extend(std::iter::repeat_n(b't', 257));
    assert_eq!(ClientMsg::decode(&forged), Err(WireError::TooLong));
    let emoji = "🦦".repeat(64); // 256 bytes
    assert!(ClientMsg::SetTitle { window: 1, title: emoji }.encode().is_ok());
}

#[test]
fn malformed_input_is_rejected_precisely() {
    let good = ClientMsg::Damage { window: 3, x: 1, y: 2, w: 3, h: 4 }.encode().unwrap();
    assert_eq!(ClientMsg::decode(&good[..good.len() - 1]), Err(WireError::Truncated));
    assert_eq!(ClientMsg::decode(&[]), Err(WireError::Truncated));
    assert_eq!(ClientMsg::decode(&[3]), Err(WireError::Truncated));
    let mut trailing = good.clone();
    trailing.push(0);
    assert_eq!(ClientMsg::decode(&trailing), Err(WireError::Trailing));
    assert_eq!(ClientMsg::decode(&[99, 0]), Err(WireError::UnknownKind));
    assert_eq!(ClientMsg::decode(&[101, 0, 1, 0, 0, 5, 0x20, 3]), Err(WireError::UnknownKind), "a server kind is not a client kind");
    assert_eq!(ServerMsg::decode(&[1, 0, 1, 0]), Err(WireError::UnknownKind), "and vice versa");
    assert_eq!(ClientMsg::decode(&[4, 0, 1, 0, 0, 0, 2, 0, 0xC3, 0x28]), Err(WireError::BadUtf8));
    assert_eq!(ClientMsg::decode(&[7, 0, 6]), Err(WireError::BadValue), "cursor shape 6 does not exist");
    assert_eq!(ServerMsg::decode(&[104, 0, 1, 0, 0, 0, 2]), Err(WireError::BadValue), "bool must be 0 or 1");
    let surrogate = [105, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0x00, 0xD8, 0, 0];
    assert_eq!(ServerMsg::decode(&surrogate), Err(WireError::BadValue), "0xD800 is not a char");
    let too_big = [105, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0x00, 0x00, 0x11, 0];
    assert_eq!(ServerMsg::decode(&too_big), Err(WireError::BadValue), "0x110000 is not a char");
}

#[test]
fn random_bytes_never_panic() {
    let mut x = 42u64;
    for _ in 0..100_000 {
        let len = (xorshift(&mut x) % 40) as usize;
        let mut bytes: Vec<u8> = (0..len).map(|_| xorshift(&mut x) as u8).collect();
        if len >= 2 && xorshift(&mut x).is_multiple_of(2) {
            let kinds = [1u16, 2, 3, 4, 5, 6, 7, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110];
            let k = kinds[(xorshift(&mut x) % kinds.len() as u64) as usize];
            bytes[..2].copy_from_slice(&k.to_le_bytes());
        }
        if let Ok(m) = ClientMsg::decode(&bytes) {
            assert_eq!(m.encode().unwrap(), bytes, "a decoded message re-encodes to the same bytes");
        }
        if let Ok(m) = ServerMsg::decode(&bytes) {
            assert_eq!(m.encode().unwrap(), bytes);
        }
    }
}
