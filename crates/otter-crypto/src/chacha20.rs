//! ChaCha20 (RFC 8439 section 2.3/2.4): a 256-bit-key, 96-bit-nonce, 32-bit-block-
//! counter stream cipher.

/// "expand 32-byte k" as four little-endian 32-bit words (RFC 8439 section 2.3).
const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

/// The ChaCha20 quarter round (RFC 8439 section 2.1): mixes four of the 16
/// state words in place.
#[inline]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);

    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

/// The ChaCha20 block function (RFC 8439 section 2.3): produces 64 bytes of
/// keystream from a key, a 32-bit little-endian block counter and a 96-bit
/// nonce.
pub fn block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[0..4].copy_from_slice(&CONSTANTS);
    for i in 0..8 {
        state[4 + i] = u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
    }
    state[12] = counter;
    for i in 0..3 {
        state[13 + i] = u32::from_le_bytes(nonce[i * 4..i * 4 + 4].try_into().unwrap());
    }

    let initial = state;
    for _ in 0..10 {
        // A "column round" then a "diagonal round" (RFC 8439 section 2.3.1);
        // ten iterations of both make the 20 ChaCha rounds.
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }

    let mut out = [0u8; 64];
    for i in 0..16 {
        let word = state[i].wrapping_add(initial[i]);
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// Encrypts (or, identically, decrypts) `buf` in place by XORing it with the
/// ChaCha20 keystream starting at `initial_counter` (RFC 8439 section 2.4).
pub fn xor_keystream(key: &[u8; 32], nonce: &[u8; 12], initial_counter: u32, buf: &mut [u8]) {
    let mut counter = initial_counter;
    for chunk in buf.chunks_mut(64) {
        let keystream = block(key, counter, nonce);
        for (byte, ks) in chunk.iter_mut().zip(keystream.iter()) {
            *byte ^= ks;
        }
        // RFC 8439 caps a single invocation at 2^32-1 blocks (section 2.8, note
        // 1); wrapping here matches the spec's counter arithmetic instead of
        // silently producing a nonsensical keystream via a debug-mode panic.
        counter = counter.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn hex(bytes: &[u8]) -> alloc::string::String {
        use alloc::string::String;
        use core::fmt::Write;
        let mut s = String::new();
        for b in bytes {
            write!(s, "{b:02x}").unwrap();
        }
        s
    }

    fn key_from_hex(s: &str) -> [u8; 32] {
        hex_decode(s).try_into().unwrap()
    }

    fn nonce_from_hex(s: &str) -> [u8; 12] {
        hex_decode(s).try_into().unwrap()
    }

    #[test]
    fn rfc_8439_2_3_2_block_function() {
        let key = key_from_hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let nonce = nonce_from_hex("000000090000004a00000000");
        let out = block(&key, 1, &nonce);
        assert_eq!(
            hex(&out),
            "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4ed2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e"
        );
    }

    #[test]
    fn rfc_8439_2_4_2_encryption() {
        let key = key_from_hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let nonce = nonce_from_hex("000000000000004a00000000");
        let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let mut buf = plaintext.to_vec();
        xor_keystream(&key, &nonce, 1, &mut buf);
        assert_eq!(
            hex(&buf),
            "6e2e359a2568f98041ba0728dd0d6981e97e7aec1d4360c20a27afccfd9fae0bf91b65c5524733ab8f593dabcd62b3571639d624e65152ab8f530c359f0861d807ca0dbf500d6a6156a38e088a22b65e52bc514d16ccf806818ce91ab77937365af90bbf74a35be6b40b8eedf2785e42874d"
        );
    }

    #[test]
    fn block_function_counter_and_nonce_change_the_output() {
        let key = [0x42u8; 32];
        let nonce = [0x24u8; 12];
        let b0 = block(&key, 0, &nonce);
        let b1 = block(&key, 1, &nonce);
        assert_ne!(b0, b1);
        let other_nonce = [0x25u8; 12];
        let b0_other_nonce = block(&key, 0, &other_nonce);
        assert_ne!(b0, b0_other_nonce);
    }

    #[test]
    fn xor_keystream_is_its_own_inverse() {
        let key = [7u8; 32];
        let nonce = [9u8; 12];
        let plaintext: Vec<u8> = (0..300u32).map(|i| (i % 256) as u8).collect();
        let mut buf = plaintext.clone();
        xor_keystream(&key, &nonce, 1, &mut buf);
        assert_ne!(buf, plaintext);
        xor_keystream(&key, &nonce, 1, &mut buf);
        assert_eq!(buf, plaintext);
    }
}
