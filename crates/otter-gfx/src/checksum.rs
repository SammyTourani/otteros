//! CRC-32 (PNG chunks) and Adler-32 (zlib streams) checksums.

/// Streaming CRC-32 (ISO 3309 / PNG Annex D: poly `0xEDB88320`, init/final `0xFFFFFFFF`).
pub struct Crc32 {
    state: u32,
}

fn crc_table_entry(mut n: u32) -> u32 {
    for _ in 0..8 {
        n = if n & 1 != 0 { 0xEDB8_8320 ^ (n >> 1) } else { n >> 1 };
    }
    n
}

impl Crc32 {
    /// A fresh CRC accumulator.
    pub fn new() -> Self {
        Crc32 { state: 0xFFFF_FFFF }
    }

    /// Feeds more bytes into the running checksum.
    pub fn update(&mut self, data: &[u8]) {
        let mut c = self.state;
        for &byte in data {
            let idx = (c ^ byte as u32) & 0xff;
            c = crc_table_entry(idx) ^ (c >> 8);
        }
        self.state = c;
    }

    /// The final CRC-32 value.
    pub fn finish(&self) -> u32 {
        self.state ^ 0xFFFF_FFFF
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot CRC-32 over a byte slice.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.finish()
}

/// One-shot Adler-32 (RFC 1950) over a byte slice.
pub fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    // Accumulate in chunks so `b`'s intermediate sum can't overflow u32 before a
    // reduction: worst case per byte adds up to ~65520 to `b`, so ~5552 bytes is the
    // safe chunk size before b could overflow (65521 * 65520 < u32::MAX).
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= MOD_ADLER;
        b %= MOD_ADLER;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_check_value() {
        // The standard CRC-32 check value for the ASCII string "123456789".
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn crc32_of_empty_is_zero() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn crc32_streaming_matches_one_shot() {
        let data = b"the quick brown fox jumps over the lazy dog";
        let mut c = Crc32::new();
        c.update(&data[..10]);
        c.update(&data[10..]);
        assert_eq!(c.finish(), crc32(data));
    }

    #[test]
    fn adler32_check_value() {
        // Adler-32 of "Wikipedia" is a commonly-cited test vector.
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn adler32_of_empty_is_one() {
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn adler32_large_input_does_not_overflow() {
        let data = alloc::vec![0x5Au8; 20_000];
        // Just needs to not panic/overflow; cross-check the chunked path against an
        // unchunked reference implementation.
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for &byte in &data {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        assert_eq!(adler32(&data), (b << 16) | a);
    }
}
