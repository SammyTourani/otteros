//! AES-128/256-GCM (NIST SP 800-38D), 96-bit nonces only -- the case TLS 1.3
//! actually uses, and the only one this crate's fixed-size `&[u8; 12]` nonce
//! type can even express (SP 800-38D's more general variable-length-IV
//! derivation, needed only for other nonce sizes, is simply never reachable).
//!
//! [`Aes128Gcm`]/[`Aes256Gcm`] each hold a key schedule ([`crate::aes`]'s
//! constant-time software one, and -- when [`crate::hw::aes_ni_available`] says
//! so -- an AES-NI one from [`crate::hw::aes`]) plus the GHASH subkey
//! `H = E_K(0^128)`, computed once at construction. `seal_in_place`/
//! `open_in_place` mirror [`crate::aead`]'s ChaCha20-Poly1305 functions in
//! shape (nonce, then AAD, then the in-place buffer, then the 16-byte tag),
//! differing only in taking the key at construction instead of per call, since
//! (unlike ChaCha20-Poly1305) AES key expansion is too expensive to redo on
//! every single `seal`/`open`; both also implement [`crate::aead::Aead`] for
//! code that is generic over the AEAD.
//!
//! Encryption (SP 800-38D section 7.1): `J0 = nonce || 0^31 || 1`;
//! ciphertext `C = GCTR_K(inc32(J0), P)`; tag
//! `T = E_K(J0) XOR GHASH_H(AAD || pad || C || pad || [bitlen(AAD)]_64 ||
//! [bitlen(C)]_64)` -- note GHASH's length block counts *bits*, big-endian,
//! unlike RFC 8439 Poly1305's *byte*, little-endian lengths in [`crate::aead`].
//! Decryption (section 7.2) computes the same tag from the received
//! ciphertext and compares in constant time before decrypting anything.

use crate::aead::{Aead, AeadError};
use crate::aes;
use crate::ct;
use crate::ghash::Ghash;

// On x86_64, an AES-NI round-key schedule really exists; off it, `HwAes128`/
// `HwAes256` are uninhabited (zero-variant) types, so `Option<HwAesNNN>` can
// only ever be `None` and every match arm that would construct or consume
// `Some` is simply dead code the compiler proves unreachable -- no `cfg` is
// needed anywhere below this point.
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
type HwAes128 = crate::hw::aes::Aes128RoundKeys;
#[cfg(not(all(target_arch = "x86_64", target_feature = "sse2")))]
enum HwAes128 {}
#[cfg(not(all(target_arch = "x86_64", target_feature = "sse2")))]
impl HwAes128 {
    /// Unreachable: `self` is uninhabited off x86_64, so no value of this
    /// type can ever exist to call this on.
    fn encrypt_block(&self, _block: &mut [u8; 16]) {
        match *self {}
    }
}

#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
type HwAes256 = crate::hw::aes::Aes256RoundKeys;
#[cfg(not(all(target_arch = "x86_64", target_feature = "sse2")))]
enum HwAes256 {}
#[cfg(not(all(target_arch = "x86_64", target_feature = "sse2")))]
impl HwAes256 {
    /// Unreachable: see `HwAes128`'s.
    fn encrypt_block(&self, _block: &mut [u8; 16]) {
        match *self {}
    }
}

/// `J0` for a 96-bit nonce (SP 800-38D section 7.1): the nonce followed by a
/// 32-bit big-endian counter starting at 1.
fn j0_from_nonce(nonce: &[u8; 12]) -> [u8; 16] {
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(nonce);
    j0[15] = 1;
    j0
}

/// `inc32` (SP 800-38D section 6.2): increments only the low 32 bits of the
/// block, wrapping, leaving the top 96 bits (here, the nonce) untouched.
fn inc32(block: [u8; 16]) -> [u8; 16] {
    let mut out = block;
    let counter = u32::from_be_bytes(out[12..16].try_into().unwrap());
    out[12..16].copy_from_slice(&counter.wrapping_add(1).to_be_bytes());
    out
}

/// `GCTR_K` (SP 800-38D section 6.5): CTR-mode encryption (its own inverse),
/// starting at `icb` and incrementing with `inc32` for every subsequent block.
/// An empty `buf` is left alone, matching `GCTR_K(_, epsilon) = epsilon`.
fn gctr(mut encrypt_block: impl FnMut(&mut [u8; 16]), icb: [u8; 16], buf: &mut [u8]) {
    let mut cb = icb;
    for chunk in buf.chunks_mut(16) {
        let mut keystream = cb;
        encrypt_block(&mut keystream);
        for (b, k) in chunk.iter_mut().zip(keystream.iter()) {
            *b ^= k;
        }
        cb = inc32(cb);
    }
}

macro_rules! define_gcm {
    ($name:ident, $sw_round_keys:ty, $hw_round_keys:ty, $key_len:literal, $new_sw:path, $encrypt_sw:path, $new_hw:path) => {
        #[doc = concat!(
            "AES-", stringify!($key_len), "-bit-key GCM: a key schedule (software, ",
            "and an AES-NI one if available) plus the GHASH subkey, computed once."
        )]
        pub struct $name {
            sw: $sw_round_keys,
            hw: Option<$hw_round_keys>,
            h_block: [u8; 16],
            use_hw_ghash: bool,
        }

        impl $name {
            fn build(key: &[u8; $key_len], hw: Option<$hw_round_keys>, use_hw_ghash: bool) -> Self {
                let sw = $new_sw(key);
                let mut h_block = [0u8; 16];
                Self::encrypt_block_with(&sw, &hw, &mut h_block);
                $name { sw, hw, h_block, use_hw_ghash }
            }

            fn encrypt_block_with(sw: &$sw_round_keys, hw: &Option<$hw_round_keys>, block: &mut [u8; 16]) {
                if let Some(hw) = hw {
                    hw.encrypt_block(block);
                } else {
                    $encrypt_sw(sw, block);
                }
            }

            fn encrypt_block(&self, block: &mut [u8; 16]) {
                Self::encrypt_block_with(&self.sw, &self.hw, block)
            }

            /// Builds a cipher bound to `key`, using AES-NI/PCLMULQDQ when this
            /// CPU has them (`crate::hw::aes_ni_available`/`pclmulqdq_available`).
            pub fn new(key: &[u8; $key_len]) -> Self {
                #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
                let hw = $new_hw(key);
                #[cfg(not(all(target_arch = "x86_64", target_feature = "sse2")))]
                let hw: Option<$hw_round_keys> = None;
                Self::build(key, hw, crate::hw::pclmulqdq_available())
            }

            /// Builds a cipher that always uses the constant-time software AES
            /// and GHASH paths, regardless of what this CPU supports -- for the
            /// hardware/software differential test, and for callers that want a
            /// result with no dependency on CPU features.
            pub fn new_software(key: &[u8; $key_len]) -> Self {
                Self::build(key, None, false)
            }

            /// Builds a cipher that always uses AES-NI and PCLMULQDQ, or returns
            /// `None` if this CPU lacks either -- for the differential test.
            #[cfg_attr(not(all(target_arch = "x86_64", target_feature = "sse2")), allow(unused_variables))]
            pub fn new_hardware(key: &[u8; $key_len]) -> Option<Self> {
                #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
                {
                    let hw = $new_hw(key)?;
                    if !crate::hw::pclmulqdq_available() {
                        return None;
                    }
                    Some(Self::build(key, Some(hw), true))
                }
                #[cfg(not(all(target_arch = "x86_64", target_feature = "sse2")))]
                None
            }

            fn tag(&self, j0: [u8; 16], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
                let mut g = Ghash::new(self.h_block, self.use_hw_ghash);
                g.absorb_padded(aad);
                g.absorb_padded(ciphertext);
                let mut len_block = [0u8; 16];
                len_block[0..8].copy_from_slice(&((aad.len() as u64) * 8).to_be_bytes());
                len_block[8..16].copy_from_slice(&((ciphertext.len() as u64) * 8).to_be_bytes());
                g.absorb(&len_block);
                let s = g.finalize();

                let mut ek_j0 = j0;
                self.encrypt_block(&mut ek_j0);
                let mut t = [0u8; 16];
                for i in 0..16 {
                    t[i] = ek_j0[i] ^ s[i];
                }
                t
            }

            /// Encrypts `buf` in place under `nonce`, authenticating `aad`
            /// alongside it, and returns the 16-byte tag. `nonce` must never
            /// repeat for this key.
            pub fn seal_in_place(&self, nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> [u8; 16] {
                let j0 = j0_from_nonce(nonce);
                gctr(|b| self.encrypt_block(b), inc32(j0), buf);
                self.tag(j0, aad, buf)
            }

            /// Verifies `tag` and, only if it matches, decrypts `buf` in place.
            /// Leaves `buf` zeroed on failure.
            pub fn open_in_place(
                &self,
                nonce: &[u8; 12],
                aad: &[u8],
                buf: &mut [u8],
                tag: &[u8; 16],
            ) -> Result<(), AeadError> {
                let j0 = j0_from_nonce(nonce);
                let expected = self.tag(j0, aad, buf);
                if !ct::ct_eq(&expected, tag) {
                    ct::zeroize(buf);
                    return Err(AeadError::InvalidTag);
                }
                gctr(|b| self.encrypt_block(b), inc32(j0), buf);
                Ok(())
            }
        }

        impl Aead for $name {
            fn seal_in_place(&self, nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> [u8; 16] {
                $name::seal_in_place(self, nonce, aad, buf)
            }

            fn open_in_place(
                &self,
                nonce: &[u8; 12],
                aad: &[u8],
                buf: &mut [u8],
                tag: &[u8; 16],
            ) -> Result<(), AeadError> {
                $name::open_in_place(self, nonce, aad, buf, tag)
            }
        }
    };
}

define_gcm!(
    Aes128Gcm,
    aes::RoundKeys128,
    HwAes128,
    16,
    aes::RoundKeys128::new,
    aes::encrypt_block_128,
    HwAes128::new
);
define_gcm!(
    Aes256Gcm,
    aes::RoundKeys256,
    HwAes256,
    32,
    aes::RoundKeys256::new,
    aes::encrypt_block_256,
    HwAes256::new
);

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn hex(bytes: &[u8]) -> alloc::string::String {
        use alloc::string::String;
        use core::fmt::Write;
        let mut s = String::new();
        for b in bytes {
            write!(s, "{b:02x}").unwrap();
        }
        s
    }

    /// The Galois/Counter Mode of Operation (McGrew & Viega) / NIST's own GCM
    /// specification "Test Case 1": an all-zero 128-bit key, empty plaintext and
    /// AAD, and a 96-bit all-zero IV -- the most widely reproduced GCM vector
    /// there is. Independently reconfirmed against a local libcrypto oracle
    /// during development (not committed; see the brief's report).
    #[test]
    fn nist_test_case_1_aes128() {
        let gcm = Aes128Gcm::new_software(&[0u8; 16]);
        let mut buf: [u8; 0] = [];
        let tag = gcm.seal_in_place(&[0u8; 12], &[], &mut buf);
        assert_eq!(hex(&tag), "58e2fccefa7e3061367f1d57a4e7455a");
        gcm.open_in_place(&[0u8; 12], &[], &mut buf, &tag).unwrap();
    }

    /// NIST GCM "Test Case 2": as above but one all-zero plaintext block.
    #[test]
    fn nist_test_case_2_aes128() {
        let gcm = Aes128Gcm::new_software(&[0u8; 16]);
        let mut buf = [0u8; 16]; // one all-zero plaintext block.
        let tag = gcm.seal_in_place(&[0u8; 12], &[], &mut buf);
        assert_eq!(hex(&buf), "0388dace60b6a392f328c2b971b2fe78");
        assert_eq!(hex(&tag), "ab6e47d42cec13bdf53a67b21257bddf");
        gcm.open_in_place(&[0u8; 12], &[], &mut buf, &tag).unwrap();
        assert_eq!(buf, [0u8; 16], "must decrypt back to the all-zero plaintext");
    }

    #[test]
    fn seal_then_open_roundtrips_for_various_lengths_and_aad_aes128() {
        let gcm = Aes128Gcm::new_software(&[0x11u8; 16]);
        let nonce = [0x22u8; 12];
        for msg_len in [0usize, 1, 15, 16, 17, 63, 64, 65, 300] {
            for aad_len in [0usize, 1, 16, 33] {
                let plaintext: Vec<u8> = (0..msg_len as u32).map(|i| (i % 256) as u8).collect();
                let aad: Vec<u8> = (0..aad_len as u32).map(|i| ((i * 7) % 256) as u8).collect();
                let mut buf = plaintext.clone();
                let tag = gcm.seal_in_place(&nonce, &aad, &mut buf);
                gcm.open_in_place(&nonce, &aad, &mut buf, &tag)
                    .unwrap_or_else(|e| panic!("msg_len={msg_len} aad_len={aad_len}: {e:?}"));
                assert_eq!(buf, plaintext, "msg_len={msg_len} aad_len={aad_len}");
            }
        }
    }

    #[test]
    fn seal_then_open_roundtrips_aes256() {
        let gcm = Aes256Gcm::new_software(&[0x33u8; 32]);
        let nonce = [0x44u8; 12];
        let plaintext = b"attack at dawn, repeat, attack at dawn";
        let mut buf = plaintext.to_vec();
        let tag = gcm.seal_in_place(&nonce, b"header", &mut buf);
        gcm.open_in_place(&nonce, b"header", &mut buf, &tag).unwrap();
        assert_eq!(buf, plaintext);
    }

    #[test]
    fn tampered_ciphertext_is_rejected_and_zeroes_the_buffer() {
        let gcm = Aes128Gcm::new_software(&[0x55u8; 16]);
        let nonce = [0x66u8; 12];
        let plaintext = b"attack at dawn";
        let mut buf = plaintext.to_vec();
        let tag = gcm.seal_in_place(&nonce, b"", &mut buf);
        buf[0] ^= 1;
        let err = gcm.open_in_place(&nonce, b"", &mut buf, &tag).unwrap_err();
        assert_eq!(err, AeadError::InvalidTag);
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn tampered_tag_and_aad_are_rejected() {
        let gcm = Aes128Gcm::new_software(&[0x77u8; 16]);
        let nonce = [0x88u8; 12];
        let plaintext = b"attack at dawn";
        let mut buf = plaintext.to_vec();
        let mut tag = gcm.seal_in_place(&nonce, b"header-a", &mut buf);
        tag[15] ^= 1;
        assert_eq!(gcm.open_in_place(&nonce, b"header-a", &mut buf, &tag), Err(AeadError::InvalidTag));

        let mut buf2 = plaintext.to_vec();
        let tag2 = gcm.seal_in_place(&nonce, b"header-a", &mut buf2);
        assert_eq!(gcm.open_in_place(&nonce, b"header-b", &mut buf2, &tag2), Err(AeadError::InvalidTag));
    }

    #[test]
    fn aead_trait_matches_inherent_methods() {
        let gcm = Aes128Gcm::new_software(&[0x99u8; 16]);
        let nonce = [0xaau8; 12];
        let plaintext = b"generic over Aead";
        let mut via_trait = plaintext.to_vec();
        let tag = Aead::seal_in_place(&gcm, &nonce, b"aad", &mut via_trait);
        let mut via_inherent = plaintext.to_vec();
        let tag2 = gcm.seal_in_place(&nonce, b"aad", &mut via_inherent);
        assert_eq!(via_trait, via_inherent);
        assert_eq!(tag, tag2);
    }

    #[test]
    fn hardware_and_software_agree_when_hardware_is_available() {
        let key = [0x5cu8; 16];
        let Some(hw) = Aes128Gcm::new_hardware(&key) else {
            eprintln!("skipping: AES-NI/PCLMULQDQ not available on this host");
            return;
        };
        let sw = Aes128Gcm::new_software(&key);
        let nonce = [0x5du8; 12];
        let plaintext: Vec<u8> = (0..250u32).map(|i| (i % 256) as u8).collect();
        let mut hw_buf = plaintext.clone();
        let mut sw_buf = plaintext.clone();
        let hw_tag = hw.seal_in_place(&nonce, b"aad", &mut hw_buf);
        let sw_tag = sw.seal_in_place(&nonce, b"aad", &mut sw_buf);
        assert_eq!(hw_buf, sw_buf);
        assert_eq!(hw_tag, sw_tag);
    }
}
