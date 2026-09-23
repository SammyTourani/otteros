//! AEAD_CHACHA20_POLY1305 (RFC 8439 section 2.8): authenticated encryption
//! combining [`crate::chacha20`] and [`crate::poly1305`].

use crate::chacha20;
use crate::ct;
use crate::poly1305::Poly1305;

/// Up to 15 zero bytes, RFC 8439's `pad16`: pads AAD/ciphertext up to a multiple
/// of 16 bytes before they go into the Poly1305 computation.
const ZERO_PAD: [u8; 15] = [0; 15];

/// [`open_in_place`] failed: the tag did not match. The only failure mode this
/// AEAD has -- a bad tag might mean a tampered ciphertext, a tampered tag, or
/// tampered AAD, and RFC 8439 gives no way (nor reason) to tell those apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AeadError {
    /// The computed tag did not match the one supplied to `open_in_place`.
    InvalidTag,
}

fn pad16_len(len: usize) -> usize {
    let rem = len % 16;
    if rem == 0 { 0 } else { 16 - rem }
}

/// RFC 8439 section 2.6: the one-time Poly1305 key is the first 32 bytes of the
/// ChaCha20 block function's output for this key/nonce with the block counter at
/// zero (message encryption itself starts at counter 1, section 2.8.1).
fn derive_poly1305_key(key: &[u8; 32], nonce: &[u8; 12]) -> [u8; 32] {
    let block0 = chacha20::block(key, 0, nonce);
    block0[..32].try_into().unwrap()
}

/// RFC 8439 section 2.8.1's `mac_data`: `aad || pad16(aad) || ciphertext ||
/// pad16(ciphertext) || len(aad) || len(ciphertext)` (the last two as 64-bit
/// little-endian integers), MACed with the derived one-time key.
fn compute_tag(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    // The one-time Poly1305 key is as secret as `key` itself; wipe it once
    // `Poly1305::new` has copied what it needs out of it.
    let otk = ct::Zeroizing::new(derive_poly1305_key(key, nonce));
    let mut mac = Poly1305::new(&otk);
    mac.update(aad);
    mac.update(&ZERO_PAD[..pad16_len(aad.len())]);
    mac.update(ciphertext);
    mac.update(&ZERO_PAD[..pad16_len(ciphertext.len())]);
    mac.update(&(aad.len() as u64).to_le_bytes());
    mac.update(&(ciphertext.len() as u64).to_le_bytes());
    mac.finalize()
}

/// Encrypts `buf` in place (plaintext -> ciphertext) under `key`/`nonce`,
/// authenticating `aad` alongside it, and returns the 16-byte tag. `nonce` must
/// never repeat for a given `key` (RFC 8439 section 2.8: "different for each
/// invocation with the same key").
pub fn seal_in_place(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> [u8; 16] {
    // RFC 8439 section 2.8.1: encryption uses the ChaCha20 keystream starting at
    // block counter 1 (block 0 was consumed deriving the Poly1305 key above).
    chacha20::xor_keystream(key, nonce, 1, buf);
    compute_tag(key, nonce, aad, buf)
}

/// Verifies `tag` and, only if it matches, decrypts `buf` in place (ciphertext ->
/// plaintext). Verifies before decrypting, so a forged ciphertext is never
/// XORed with the keystream at all; either way `buf` is zeroed on failure so a
/// caller that forgets to check the `Result` cannot accidentally use rejected
/// data.
pub fn open_in_place(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    buf: &mut [u8],
    tag: &[u8; 16],
) -> Result<(), AeadError> {
    let expected = compute_tag(key, nonce, aad, buf);
    if !ct::ct_eq(&expected, tag) {
        ct::zeroize(buf);
        return Err(AeadError::InvalidTag);
    }
    chacha20::xor_keystream(key, nonce, 1, buf);
    Ok(())
}

/// A keyed AEAD cipher: 96-bit nonces, 128-bit tags, in-place `seal`/`open` --
/// the shape [`crate::gcm`]'s `Aes128Gcm`/`Aes256Gcm` share with
/// [`ChaCha20Poly1305`] below, so TLS 1.3 (a later brief) can pick whichever
/// cipher suite negotiation selects without matching on which AEAD it got.
pub trait Aead {
    /// Encrypts `buf` in place under `nonce`, authenticating `aad` alongside it,
    /// and returns the 16-byte tag. `nonce` must never repeat for this key.
    fn seal_in_place(&self, nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> [u8; 16];

    /// Verifies `tag` and, only on success, decrypts `buf` in place. Leaves
    /// `buf` zeroed on failure.
    fn open_in_place(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        buf: &mut [u8],
        tag: &[u8; 16],
    ) -> Result<(), AeadError>;
}

/// AEAD_CHACHA20_POLY1305 as an [`Aead`]: wraps a copy of the 256-bit key so
/// callers that are generic over [`Aead`] do not need chacha20-poly1305's own
/// free-function, pass-the-key-every-call shape.
pub struct ChaCha20Poly1305 {
    key: [u8; 32],
}

impl ChaCha20Poly1305 {
    /// Builds a cipher instance bound to `key`.
    pub fn new(key: &[u8; 32]) -> Self {
        ChaCha20Poly1305 { key: *key }
    }
}

impl Aead for ChaCha20Poly1305 {
    fn seal_in_place(&self, nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> [u8; 16] {
        seal_in_place(&self.key, nonce, aad, buf)
    }

    fn open_in_place(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        buf: &mut [u8],
        tag: &[u8; 16],
    ) -> Result<(), AeadError> {
        open_in_place(&self.key, nonce, aad, buf, tag)
    }
}

impl Drop for ChaCha20Poly1305 {
    fn drop(&mut self) {
        ct::zeroize(&mut self.key);
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

    fn key32(s: &str) -> [u8; 32] {
        hex_decode(s).try_into().unwrap()
    }

    fn nonce12(s: &str) -> [u8; 12] {
        hex_decode(s).try_into().unwrap()
    }

    #[test]
    fn rfc_8439_2_8_2_example() {
        let key = key32("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
        let nonce = nonce12("070000004041424344454647");
        let aad = hex_decode("50515253c0c1c2c3c4c5c6c7");
        let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";

        let mut buf = plaintext.to_vec();
        let tag = seal_in_place(&key, &nonce, &aad, &mut buf);
        assert_eq!(
            hex(&buf),
            "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc3ff4def08e4b7a9de576d26586cec64b6116"
        );
        assert_eq!(hex(&tag), "1ae10b594f09e26a7e902ecbd0600691");

        let mut roundtrip = buf.clone();
        open_in_place(&key, &nonce, &aad, &mut roundtrip, &tag).expect("valid tag must open");
        assert_eq!(roundtrip, plaintext);
    }

    #[test]
    fn seal_then_open_roundtrips_for_various_lengths_and_aad() {
        let key = [0x11u8; 32];
        let nonce = [0x22u8; 12];
        for msg_len in [0usize, 1, 15, 16, 17, 63, 64, 65, 300] {
            for aad_len in [0usize, 1, 16, 33] {
                let plaintext: Vec<u8> = (0..msg_len as u32).map(|i| (i % 256) as u8).collect();
                let aad: Vec<u8> = (0..aad_len as u32).map(|i| ((i * 7) % 256) as u8).collect();
                let mut buf = plaintext.clone();
                let tag = seal_in_place(&key, &nonce, &aad, &mut buf);
                open_in_place(&key, &nonce, &aad, &mut buf, &tag)
                    .unwrap_or_else(|e| panic!("msg_len={msg_len} aad_len={aad_len}: {e:?}"));
                assert_eq!(buf, plaintext, "msg_len={msg_len} aad_len={aad_len}");
            }
        }
    }

    #[test]
    fn tampered_ciphertext_is_rejected_and_zeroes_the_buffer() {
        let key = [0x33u8; 32];
        let nonce = [0x44u8; 12];
        let aad = b"header";
        let plaintext = b"attack at dawn, repeat, attack at dawn";
        let mut buf = plaintext.to_vec();
        let tag = seal_in_place(&key, &nonce, aad, &mut buf);

        buf[0] ^= 1;
        let err = open_in_place(&key, &nonce, aad, &mut buf, &tag).unwrap_err();
        assert_eq!(err, AeadError::InvalidTag);
        assert!(buf.iter().all(|&b| b == 0), "buffer must be zeroed after a rejected tag");
    }

    #[test]
    fn tampered_tag_is_rejected() {
        let key = [0x55u8; 32];
        let nonce = [0x66u8; 12];
        let aad = b"header";
        let plaintext = b"attack at dawn";
        let mut buf = plaintext.to_vec();
        let mut tag = seal_in_place(&key, &nonce, aad, &mut buf);
        tag[15] ^= 1;
        assert_eq!(open_in_place(&key, &nonce, aad, &mut buf, &tag), Err(AeadError::InvalidTag));
    }

    #[test]
    fn tampered_aad_is_rejected() {
        let key = [0x77u8; 32];
        let nonce = [0x88u8; 12];
        let plaintext = b"attack at dawn";
        let mut buf = plaintext.to_vec();
        let tag = seal_in_place(&key, &nonce, b"header-a", &mut buf);
        assert_eq!(open_in_place(&key, &nonce, b"header-b", &mut buf, &tag), Err(AeadError::InvalidTag));
    }

    #[test]
    fn aead_trait_matches_free_functions() {
        let key = key32("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
        let nonce = nonce12("070000004041424344454647");
        let aad = hex_decode("50515253c0c1c2c3c4c5c6c7");
        let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";

        let cipher = ChaCha20Poly1305::new(&key);
        let mut buf = plaintext.to_vec();
        let tag = cipher.seal_in_place(&nonce, &aad, &mut buf);
        assert_eq!(
            hex(&buf),
            "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc3ff4def08e4b7a9de576d26586cec64b6116"
        );
        cipher.open_in_place(&nonce, &aad, &mut buf, &tag).expect("valid tag must open");
        assert_eq!(buf, plaintext);
    }

    #[test]
    fn different_nonce_does_not_decrypt() {
        let key = [0x99u8; 32];
        let plaintext = b"attack at dawn";
        let mut buf = plaintext.to_vec();
        let tag = seal_in_place(&key, &[1u8; 12], b"", &mut buf);
        assert_eq!(open_in_place(&key, &[2u8; 12], b"", &mut buf, &tag), Err(AeadError::InvalidTag));
    }
}
