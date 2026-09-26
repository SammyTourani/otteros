use alloc::string::String;
use alloc::vec::Vec;
use crate::{Cursor, WireError};
use crate::codec::{Reader, Writer};

/// Client -> server messages.
///
/// Layout (little-endian, no padding):
/// 1 Hello { version: u16 }
/// 2 CreateWindow { req: u32, width: u16, height: u16, stride: u32, flags: u32, title: str } (+1 handle)
/// 3 Damage { window: u32, x: u16, y: u16, w: u16, h: u16 }
/// 4 SetTitle { window: u32, title: str }
/// 5 Close { window: u32 }
/// 6 AttachBuffer { window: u32, width: u16, height: u16, stride: u32 } (+1 handle)
/// 7 SetCursor { shape: Cursor } (u8)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMsg {
    Hello {
        version: u16,
    },
    CreateWindow {
        req: u32,
        width: u16,
        height: u16,
        stride: u32,
        flags: u32,
        title: String,
    },
    Damage {
        window: u32,
        x: u16,
        y: u16,
        w: u16,
        h: u16,
    },
    SetTitle {
        window: u32,
        title: String,
    },
    Close {
        window: u32,
    },
    AttachBuffer {
        window: u32,
        width: u16,
        height: u16,
        stride: u32,
    },
    SetCursor {
        shape: Cursor,
    },
}

impl ClientMsg {
    /// Encode this message to bytes.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        match self {
            ClientMsg::Hello { version } => {
                let mut w = Writer::new();
                w.u16(1);
                w.u16(*version);
                Ok(w.finish())
            }
            ClientMsg::CreateWindow {
                req,
                width,
                height,
                stride,
                flags,
                title,
            } => {
                let mut w = Writer::new();
                w.u16(2);
                w.u32(*req);
                w.u16(*width);
                w.u16(*height);
                w.u32(*stride);
                w.u32(*flags);
                w.string(title)?;
                Ok(w.finish())
            }
            ClientMsg::Damage {
                window,
                x,
                y,
                w,
                h,
            } => {
                let mut wr = Writer::new();
                wr.u16(3);
                wr.u32(*window);
                wr.u16(*x);
                wr.u16(*y);
                wr.u16(*w);
                wr.u16(*h);
                Ok(wr.finish())
            }
            ClientMsg::SetTitle { window, title } => {
                let mut w = Writer::new();
                w.u16(4);
                w.u32(*window);
                w.string(title)?;
                Ok(w.finish())
            }
            ClientMsg::Close { window } => {
                let mut w = Writer::new();
                w.u16(5);
                w.u32(*window);
                Ok(w.finish())
            }
            ClientMsg::AttachBuffer {
                window,
                width,
                height,
                stride,
            } => {
                let mut w = Writer::new();
                w.u16(6);
                w.u32(*window);
                w.u16(*width);
                w.u16(*height);
                w.u32(*stride);
                Ok(w.finish())
            }
            ClientMsg::SetCursor { shape } => {
                let mut w = Writer::new();
                w.u16(7);
                w.u8(*shape as u8);
                Ok(w.finish())
            }
        }
    }

    /// Decode a message from bytes.
    pub fn decode(bytes: &[u8]) -> Result<ClientMsg, WireError> {
        let mut r = Reader::new(bytes);
        let kind = r.u16()?;
        let msg = match kind {
            1 => {
                let version = r.u16()?;
                ClientMsg::Hello { version }
            }
            2 => {
                let req = r.u32()?;
                let width = r.u16()?;
                let height = r.u16()?;
                let stride = r.u32()?;
                let flags = r.u32()?;
                let title = r.string()?;
                ClientMsg::CreateWindow {
                    req,
                    width,
                    height,
                    stride,
                    flags,
                    title,
                }
            }
            3 => {
                let window = r.u32()?;
                let x = r.u16()?;
                let y = r.u16()?;
                let w = r.u16()?;
                let h = r.u16()?;
                ClientMsg::Damage { window, x, y, w, h }
            }
            4 => {
                let window = r.u32()?;
                let title = r.string()?;
                ClientMsg::SetTitle { window, title }
            }
            5 => {
                let window = r.u32()?;
                ClientMsg::Close { window }
            }
            6 => {
                let window = r.u32()?;
                let width = r.u16()?;
                let height = r.u16()?;
                let stride = r.u32()?;
                ClientMsg::AttachBuffer {
                    window,
                    width,
                    height,
                    stride,
                }
            }
            7 => {
                let shape_byte = r.u8()?;
                let shape = match shape_byte {
                    0 => Cursor::Arrow,
                    1 => Cursor::Text,
                    2 => Cursor::Hand,
                    3 => Cursor::ResizeH,
                    4 => Cursor::ResizeV,
                    5 => Cursor::Busy,
                    _ => return Err(WireError::BadValue),
                };
                ClientMsg::SetCursor { shape }
            }
            _ => return Err(WireError::UnknownKind),
        };
        r.finish()?;
        Ok(msg)
    }

    /// Return the number of handles that travel with this message.
    pub fn handles(&self) -> usize {
        match self {
            ClientMsg::CreateWindow { .. } => 1,
            ClientMsg::AttachBuffer { .. } => 1,
            _ => 0,
        }
    }
}
