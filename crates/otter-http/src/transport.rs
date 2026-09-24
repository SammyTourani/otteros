use alloc::vec::Vec;
use crate::HttpError;

/// A trait for reading and writing bytes (used by test helpers)
pub trait Transport {
    /// Read bytes into the buffer, returning the number of bytes read
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, HttpError>;

    /// Write bytes, returning the number of bytes written
    fn write(&mut self, buf: &[u8]) -> Result<usize, HttpError>;

    /// Flush any pending writes
    fn flush(&mut self) -> Result<(), HttpError> {
        Ok(())
    }
}

#[cfg(test)]
impl Transport for std::net::TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        std::io::Read::read(self, buf).map_err(|_| HttpError::Other)
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize, HttpError> {
        std::io::Write::write(self, buf).map_err(|_| HttpError::Other)
    }

    fn flush(&mut self) -> Result<(), HttpError> {
        std::io::Write::flush(self).map_err(|_| HttpError::Other)
    }
}

/// A simple in-memory mock transport for tests
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct MockTransport {
    /// Data to be read
    read_buffer: Vec<u8>,
    /// Current read position
    read_pos: usize,
    /// Data that has been written
    written_data: Vec<u8>,
}

impl MockTransport {
    /// Create a new mock transport with read data
    #[allow(dead_code)]
    pub fn new(read_data: &[u8]) -> Self {
        MockTransport {
            read_buffer: read_data.to_vec(),
            read_pos: 0,
            written_data: Vec::new(),
        }
    }

    /// Get the written data
    #[allow(dead_code)]
    pub fn written(&self) -> &[u8] {
        &self.written_data
    }
}

impl Transport for MockTransport {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        let remaining = self.read_buffer.len() - self.read_pos;
        let to_read = remaining.min(buf.len());

        if to_read > 0 {
            buf[..to_read].copy_from_slice(&self.read_buffer[self.read_pos..self.read_pos + to_read]);
            self.read_pos += to_read;
        }

        Ok(to_read)
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize, HttpError> {
        self.written_data.extend_from_slice(buf);
        Ok(buf.len())
    }
}
