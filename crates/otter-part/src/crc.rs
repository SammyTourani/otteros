//! CRC-32 computation (IEEE 802.3 polynomial, as used by GPT).
//! Reflected polynomial 0xEDB88320, initial and final XOR 0xFFFFFFFF.

/// Computes the CRC-32 of a byte slice using the IEEE 802.3 polynomial.
pub const fn crc32(data: &[u8]) -> u32 {
    let table = crc32_table();
    let mut crc = !0u32;
    let mut i = 0;
    while i < data.len() {
        let byte = data[i];
        crc = table[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
        i += 1;
    }
    !crc
}

/// Builds a 256-entry CRC-32 lookup table at compile time.
const fn crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32_ieee() {
        // Standard test vector
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }
}
