use alloc::string::String;
use alloc::vec::Vec;
use crate::WireError;
use crate::codec::{Reader, Writer};

/// Server -> client messages.
///
/// Layout (little-endian, no padding):
/// 101 Welcome { version: u16, screen_w: u16, screen_h: u16 }
/// 102 WindowCreated { req: u32, window: u32 }
/// 103 Configure { window: u32, width: u16, height: u16 }
/// 104 Focus { window: u32, focused: bool }
/// 105 Key { window: u32, code: u16, pressed: bool, mods: u8, ch: u32 }
/// 106 Pointer { window: u32, x: i16, y: i16, buttons: u8 }
/// 107 Button { window: u32, button: u8, pressed: bool, x: i16, y: i16 }
/// 108 Wheel { window: u32, delta: i16 }
/// 109 CloseRequested { window: u32 }
/// 110 Error { req: u32, code: u16, message: str }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMsg {
    Welcome {
        version: u16,
        screen_w: u16,
        screen_h: u16,
    },
    WindowCreated {
        req: u32,
        window: u32,
    },
    Configure {
        window: u32,
        width: u16,
        height: u16,
    },
    Focus {
        window: u32,
        focused: bool,
    },
    Key {
        window: u32,
        code: u16,
        pressed: bool,
        mods: u8,
        ch: u32,
    },
    Pointer {
        window: u32,
        x: i16,
        y: i16,
        buttons: u8,
    },
    Button {
        window: u32,
        button: u8,
        pressed: bool,
        x: i16,
        y: i16,
    },
    Wheel {
        window: u32,
        delta: i16,
    },
    CloseRequested {
        window: u32,
    },
    Error {
        req: u32,
        code: u16,
        message: String,
    },
}

impl ServerMsg {
    /// Encode this message to bytes.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        match self {
            ServerMsg::Welcome {
                version,
                screen_w,
                screen_h,
            } => {
                let mut w = Writer::new();
                w.u16(101);
                w.u16(*version);
                w.u16(*screen_w);
                w.u16(*screen_h);
                Ok(w.finish())
            }
            ServerMsg::WindowCreated { req, window } => {
                let mut w = Writer::new();
                w.u16(102);
                w.u32(*req);
                w.u32(*window);
                Ok(w.finish())
            }
            ServerMsg::Configure {
                window,
                width,
                height,
            } => {
                let mut w = Writer::new();
                w.u16(103);
                w.u32(*window);
                w.u16(*width);
                w.u16(*height);
                Ok(w.finish())
            }
            ServerMsg::Focus { window, focused } => {
                let mut w = Writer::new();
                w.u16(104);
                w.u32(*window);
                w.bool(*focused);
                Ok(w.finish())
            }
            ServerMsg::Key {
                window,
                code,
                pressed,
                mods,
                ch,
            } => {
                let mut w = Writer::new();
                w.u16(105);
                w.u32(*window);
                w.u16(*code);
                w.bool(*pressed);
                w.u8(*mods);
                w.char(*ch);
                Ok(w.finish())
            }
            ServerMsg::Pointer {
                window,
                x,
                y,
                buttons,
            } => {
                let mut w = Writer::new();
                w.u16(106);
                w.u32(*window);
                w.i16(*x);
                w.i16(*y);
                w.u8(*buttons);
                Ok(w.finish())
            }
            ServerMsg::Button {
                window,
                button,
                pressed,
                x,
                y,
            } => {
                let mut w = Writer::new();
                w.u16(107);
                w.u32(*window);
                w.u8(*button);
                w.bool(*pressed);
                w.i16(*x);
                w.i16(*y);
                Ok(w.finish())
            }
            ServerMsg::Wheel { window, delta } => {
                let mut w = Writer::new();
                w.u16(108);
                w.u32(*window);
                w.i16(*delta);
                Ok(w.finish())
            }
            ServerMsg::CloseRequested { window } => {
                let mut w = Writer::new();
                w.u16(109);
                w.u32(*window);
                Ok(w.finish())
            }
            ServerMsg::Error { req, code, message } => {
                let mut w = Writer::new();
                w.u16(110);
                w.u32(*req);
                w.u16(*code);
                w.string(message)?;
                Ok(w.finish())
            }
        }
    }

    /// Decode a message from bytes.
    pub fn decode(bytes: &[u8]) -> Result<ServerMsg, WireError> {
        let mut r = Reader::new(bytes);
        let kind = r.u16()?;
        let msg = match kind {
            101 => {
                let version = r.u16()?;
                let screen_w = r.u16()?;
                let screen_h = r.u16()?;
                ServerMsg::Welcome {
                    version,
                    screen_w,
                    screen_h,
                }
            }
            102 => {
                let req = r.u32()?;
                let window = r.u32()?;
                ServerMsg::WindowCreated { req, window }
            }
            103 => {
                let window = r.u32()?;
                let width = r.u16()?;
                let height = r.u16()?;
                ServerMsg::Configure {
                    window,
                    width,
                    height,
                }
            }
            104 => {
                let window = r.u32()?;
                let focused = r.bool()?;
                ServerMsg::Focus { window, focused }
            }
            105 => {
                let window = r.u32()?;
                let code = r.u16()?;
                let pressed = r.bool()?;
                let mods = r.u8()?;
                let ch = r.char()?;
                ServerMsg::Key {
                    window,
                    code,
                    pressed,
                    mods,
                    ch,
                }
            }
            106 => {
                let window = r.u32()?;
                let x = r.i16()?;
                let y = r.i16()?;
                let buttons = r.u8()?;
                ServerMsg::Pointer {
                    window,
                    x,
                    y,
                    buttons,
                }
            }
            107 => {
                let window = r.u32()?;
                let button = r.u8()?;
                let pressed = r.bool()?;
                let x = r.i16()?;
                let y = r.i16()?;
                ServerMsg::Button {
                    window,
                    button,
                    pressed,
                    x,
                    y,
                }
            }
            108 => {
                let window = r.u32()?;
                let delta = r.i16()?;
                ServerMsg::Wheel { window, delta }
            }
            109 => {
                let window = r.u32()?;
                ServerMsg::CloseRequested { window }
            }
            110 => {
                let req = r.u32()?;
                let code = r.u16()?;
                let message = r.string()?;
                ServerMsg::Error { req, code, message }
            }
            _ => return Err(WireError::UnknownKind),
        };
        r.finish()?;
        Ok(msg)
    }

    /// Return the number of handles that travel with this message.
    pub fn handles(&self) -> usize {
        0 // No ServerMsg carries handles
    }
}
