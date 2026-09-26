/// Reader for decoding wire format messages.
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, pos: 0 }
    }

    pub fn u8(&mut self) -> Result<u8, crate::WireError> {
        if self.pos >= self.bytes.len() {
            return Err(crate::WireError::Truncated);
        }
        let v = self.bytes[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn u16(&mut self) -> Result<u16, crate::WireError> {
        if self.pos + 2 > self.bytes.len() {
            return Err(crate::WireError::Truncated);
        }
        let v = u16::from_le_bytes([self.bytes[self.pos], self.bytes[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    pub fn u32(&mut self) -> Result<u32, crate::WireError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(crate::WireError::Truncated);
        }
        let v = u32::from_le_bytes([
            self.bytes[self.pos],
            self.bytes[self.pos + 1],
            self.bytes[self.pos + 2],
            self.bytes[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }

    pub fn i16(&mut self) -> Result<i16, crate::WireError> {
        if self.pos + 2 > self.bytes.len() {
            return Err(crate::WireError::Truncated);
        }
        let v = i16::from_le_bytes([self.bytes[self.pos], self.bytes[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    pub fn bool(&mut self) -> Result<bool, crate::WireError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(crate::WireError::BadValue),
        }
    }

    pub fn char(&mut self) -> Result<u32, crate::WireError> {
        let v = self.u32()?;
        // Validate that it's a valid char or 0
        if v == 0 {
            return Ok(0);
        }
        // Check if it's a valid Unicode scalar value
        if v > 0x10FFFF || (0xD800..=0xDFFF).contains(&v) {
            return Err(crate::WireError::BadValue);
        }
        Ok(v)
    }

    pub fn string(&mut self) -> Result<alloc::string::String, crate::WireError> {
        let len = self.u16()? as usize;
        if len > crate::MAX_STR {
            return Err(crate::WireError::TooLong);
        }
        if self.pos + len > self.bytes.len() {
            return Err(crate::WireError::Truncated);
        }
        let bytes = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        alloc::string::String::from_utf8(bytes.to_vec()).map_err(|_| crate::WireError::BadUtf8)
    }

    pub fn finish(&self) -> Result<(), crate::WireError> {
        if self.pos < self.bytes.len() {
            Err(crate::WireError::Trailing)
        } else {
            Ok(())
        }
    }
}

/// Writer for encoding wire format messages.
pub struct Writer {
    pub bytes: alloc::vec::Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer {
            bytes: alloc::vec::Vec::new(),
        }
    }

    pub fn u8(&mut self, v: u8) {
        self.bytes.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i16(&mut self, v: i16) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bool(&mut self, v: bool) {
        self.bytes.push(if v { 1 } else { 0 });
    }

    pub fn char(&mut self, v: u32) {
        self.u32(v);
    }

    pub fn string(&mut self, s: &str) -> Result<(), crate::WireError> {
        let bytes = s.as_bytes();
        if bytes.len() > crate::MAX_STR {
            return Err(crate::WireError::TooLong);
        }
        self.u16(bytes.len() as u16);
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    pub fn finish(self) -> alloc::vec::Vec<u8> {
        self.bytes
    }
}
