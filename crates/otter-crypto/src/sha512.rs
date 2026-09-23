//! SHA-512 and SHA-384 (FIPS 180-4 section 6.4/6.5): both are the same 64-bit,
//! 128-byte-block compression function with a different initial hash value and,
//! for SHA-384, a truncated output -- so [`Sha512Core`] implements the algorithm
//! once and [`Sha512`]/[`Sha384`] just pick an IV and an output length.

use crate::hash::{Digest, Hash};

const BLOCK_SIZE: usize = 128;

// FIPS 180-4 section 5.3.4: SHA-512 initial hash value (first 64 bits of the
// fractional parts of the square roots of the first 8 primes).
const SHA512_IV: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

// FIPS 180-4 section 5.3.4: SHA-384 initial hash value (the second 64 bits of
// the fractional parts of the square roots of the 9th through 16th primes).
const SHA384_IV: [u64; 8] = [
    0xcbbb9d5dc1059ed8,
    0x629a292a367cd507,
    0x9159015a3070dd17,
    0x152fecd8f70e5939,
    0x67332667ffc00b31,
    0x8eb44a8768581511,
    0xdb0c2e0d64f98fa7,
    0x47b5481dbefa4fa4,
];

// FIPS 180-4 section 4.2.3: the first 64 bits of the fractional parts of the
// cube roots of the first 80 primes.
const K: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

/// One 128-byte block through the SHA-512 compression function (FIPS 180-4
/// section 6.4.2), updating `state` in place.
fn compress(state: &mut [u64; 8], block: &[u8; BLOCK_SIZE]) {
    let mut w = [0u64; 80];
    for (i, word) in w.iter_mut().take(16).enumerate() {
        *word = u64::from_be_bytes(block[i * 8..i * 8 + 8].try_into().unwrap());
    }
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;

    for t in 0..80 {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[t])
            .wrapping_add(w[t]);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(maj);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

/// The shared SHA-512/SHA-384 streaming state (FIPS 180-4 uses one algorithm for
/// both, differing only in IV and output truncation).
#[derive(Clone)]
struct Sha512Core {
    state: [u64; 8],
    buffer: [u8; BLOCK_SIZE],
    buffer_len: usize,
    /// Total bytes absorbed so far; FIPS 180-4's 128-bit *bit* length field is
    /// filled in at `finalize` (`total_len * 8`, which cannot overflow `u128` for
    /// any message this crate could plausibly be asked to hash).
    total_len: u128,
}

impl Sha512Core {
    fn with_iv(iv: [u64; 8]) -> Self {
        Sha512Core { state: iv, buffer: [0; BLOCK_SIZE], buffer_len: 0, total_len: 0 }
    }

    fn absorb(&mut self, mut data: &[u8]) {
        if self.buffer_len > 0 {
            let need = BLOCK_SIZE - self.buffer_len;
            let take = need.min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];
            if self.buffer_len < BLOCK_SIZE {
                return;
            }
            let block = self.buffer;
            compress(&mut self.state, &block);
            self.buffer_len = 0;
        }
        while data.len() >= BLOCK_SIZE {
            let block: [u8; BLOCK_SIZE] = data[..BLOCK_SIZE].try_into().unwrap();
            compress(&mut self.state, &block);
            data = &data[BLOCK_SIZE..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u128);
        self.absorb(data);
    }

    /// Applies FIPS 180-4 padding and returns all 64 bytes of the final state;
    /// SHA-384 truncates this to its first 48 bytes.
    fn finalize(mut self) -> [u8; 64] {
        let bit_len = self.total_len.wrapping_mul(8);
        self.absorb(&[0x80]);
        while self.buffer_len != 112 {
            self.absorb(&[0]);
        }
        self.absorb(&bit_len.to_be_bytes());

        let mut out = [0u8; 64];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

/// Streaming SHA-512. `Sha512::digest(data)` for one-shot use.
#[derive(Clone)]
pub struct Sha512(Sha512Core);

impl Sha512 {
    /// Starts a new SHA-512 computation.
    pub fn new() -> Self {
        Sha512(Sha512Core::with_iv(SHA512_IV))
    }

    /// Feeds more input. May be called any number of times.
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data)
    }

    /// Consumes the hasher and returns the 64-byte digest.
    pub fn finalize(self) -> [u8; 64] {
        self.0.finalize()
    }

    /// One-shot: `Sha512::new()` + `update(data)` + `finalize()`.
    pub fn digest(data: &[u8]) -> [u8; 64] {
        let mut h = Sha512::new();
        h.update(data);
        h.finalize()
    }
}

impl Default for Sha512 {
    fn default() -> Self {
        Sha512::new()
    }
}

impl Hash for Sha512 {
    const OUTPUT_SIZE: usize = 64;
    const BLOCK_SIZE: usize = BLOCK_SIZE;

    fn new() -> Self {
        Sha512::new()
    }

    fn update(&mut self, data: &[u8]) {
        Sha512::update(self, data)
    }

    fn finalize(self) -> Digest {
        Digest::from_slice(&Sha512::finalize(self))
    }
}

/// Streaming SHA-384: SHA-512's compression function with a different IV,
/// truncated to 48 bytes of output. `Sha384::digest(data)` for one-shot use.
#[derive(Clone)]
pub struct Sha384(Sha512Core);

impl Sha384 {
    /// Starts a new SHA-384 computation.
    pub fn new() -> Self {
        Sha384(Sha512Core::with_iv(SHA384_IV))
    }

    /// Feeds more input. May be called any number of times.
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data)
    }

    /// Consumes the hasher and returns the 48-byte digest.
    pub fn finalize(self) -> [u8; 48] {
        let full = self.0.finalize();
        let mut out = [0u8; 48];
        out.copy_from_slice(&full[..48]);
        out
    }

    /// One-shot: `Sha384::new()` + `update(data)` + `finalize()`.
    pub fn digest(data: &[u8]) -> [u8; 48] {
        let mut h = Sha384::new();
        h.update(data);
        h.finalize()
    }
}

impl Default for Sha384 {
    fn default() -> Self {
        Sha384::new()
    }
}

impl Hash for Sha384 {
    const OUTPUT_SIZE: usize = 48;
    const BLOCK_SIZE: usize = BLOCK_SIZE;

    fn new() -> Self {
        Sha384::new()
    }

    fn update(&mut self, data: &[u8]) {
        Sha384::update(self, data)
    }

    fn finalize(self) -> Digest {
        Digest::from_slice(&Sha384::finalize(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> alloc::string::String {
        use alloc::string::String;
        use core::fmt::Write;
        let mut s = String::new();
        for b in bytes {
            write!(s, "{b:02x}").unwrap();
        }
        s
    }

    const MSG_448: &[u8] = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    // FIPS 180-4's two-block SHA-512-family example message (896 bits).
    const MSG_896: &[u8] =
        b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu";

    #[test]
    fn sha512_fips_180_4_vectors() {
        assert_eq!(
            hex(&Sha512::digest(b"")),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        );
        assert_eq!(
            hex(&Sha512::digest(b"abc")),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            hex(&Sha512::digest(MSG_896)),
            "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"
        );
    }

    #[test]
    fn sha512_one_million_a() {
        let mut h = Sha512::new();
        let chunk = [b'a'; 1000];
        for _ in 0..1000 {
            h.update(&chunk);
        }
        assert_eq!(
            hex(&h.finalize()),
            "e718483d0ce769644e2e42c7bc15b4638e1f98b13b2044285632a803afa973ebde0ff244877ea60a4cb0432ce577c31beb009c5c2c49aa2e4eadb217ad8cc09b"
        );
    }

    #[test]
    fn sha384_fips_180_4_vectors() {
        assert_eq!(
            hex(&Sha384::digest(b"")),
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"
        );
        assert_eq!(
            hex(&Sha384::digest(b"abc")),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
        assert_eq!(
            hex(&Sha384::digest(MSG_448)),
            "3391fdddfc8dc7393707a65b1b4709397cf8b1d162af05abfe8f450de5f36bc6b0455a8520bc4e6f5fe95b1fe3c8452b"
        );
        assert_eq!(
            hex(&Sha384::digest(MSG_896)),
            "09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039"
        );
    }

    #[test]
    fn sha384_one_million_a() {
        let mut h = Sha384::new();
        let chunk = [b'a'; 1000];
        for _ in 0..1000 {
            h.update(&chunk);
        }
        assert_eq!(
            hex(&h.finalize()),
            "9d0e1809716474cb086e834e310a4a1ced149e9c00f248527972cec5704c2a5b07b8b3dc38ecc4ebae97ddd87f3d8985"
        );
    }

    #[test]
    fn sha512_streaming_matches_one_shot_at_every_split() {
        let msg: alloc::vec::Vec<u8> = (0..400u32).map(|i| (i % 251) as u8).collect();
        let expected = Sha512::digest(&msg);
        for split in 0..=msg.len() {
            let mut h = Sha512::new();
            h.update(&msg[..split]);
            h.update(&msg[split..]);
            assert_eq!(h.finalize(), expected, "mismatch splitting at {split}");
        }
    }

    #[test]
    fn sha384_streaming_matches_one_shot_at_every_split() {
        let msg: alloc::vec::Vec<u8> = (0..400u32).map(|i| (i % 251) as u8).collect();
        let expected = Sha384::digest(&msg);
        for split in 0..=msg.len() {
            let mut h = Sha384::new();
            h.update(&msg[..split]);
            h.update(&msg[split..]);
            assert_eq!(h.finalize(), expected, "mismatch splitting at {split}");
        }
    }
}
