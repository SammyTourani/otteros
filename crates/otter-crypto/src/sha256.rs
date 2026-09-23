//! SHA-256 (FIPS 180-4 section 6.2): streaming and one-shot.

use crate::hash::{Digest, Hash};

const BLOCK_SIZE: usize = 64;

// FIPS 180-4 section 5.3.3: the first 32 bits of the fractional parts of the
// square roots of the first 8 primes.
const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

// FIPS 180-4 section 4.2.2: the first 32 bits of the fractional parts of the
// cube roots of the first 64 primes.
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// One 64-byte block through the SHA-256 compression function (FIPS 180-4
/// section 6.2.2), updating `state` in place.
fn compress(state: &mut [u32; 8], block: &[u8; BLOCK_SIZE]) {
    // Message schedule: the block's 16 big-endian words, expanded to 64.
    let mut w = [0u32; 64];
    for (i, word) in w.iter_mut().take(16).enumerate() {
        *word = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;

    for t in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[t])
            .wrapping_add(w[t]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
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

/// Streaming SHA-256. `Sha256::digest(data)` for one-shot use.
#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; BLOCK_SIZE],
    buffer_len: usize,
    /// Total bytes absorbed so far (message length, not bit length -- FIPS
    /// 180-4's 64-bit length field is filled in at `finalize`).
    total_len: u64,
}

impl Sha256 {
    /// Starts a new SHA-256 computation.
    pub fn new() -> Self {
        Sha256 { state: H0, buffer: [0; BLOCK_SIZE], buffer_len: 0, total_len: 0 }
    }

    /// Feeds `data` into the running hash without touching `total_len` -- shared
    /// by [`Sha256::update`] and the padding bytes `finalize` absorbs.
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

    /// Feeds more input. May be called any number of times with any split of the
    /// message into chunks -- the result never depends on the chunking.
    pub fn update(&mut self, data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        self.absorb(data);
    }

    /// Consumes the hasher, applies FIPS 180-4 padding and returns the 32-byte
    /// digest.
    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        self.absorb(&[0x80]);
        while self.buffer_len != 56 {
            self.absorb(&[0]);
        }
        self.absorb(&bit_len.to_be_bytes());

        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// One-shot: `Sha256::new()` + `update(data)` + `finalize()`.
    pub fn digest(data: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(data);
        h.finalize()
    }
}

impl Default for Sha256 {
    fn default() -> Self {
        Sha256::new()
    }
}

impl Hash for Sha256 {
    const OUTPUT_SIZE: usize = 32;
    const BLOCK_SIZE: usize = BLOCK_SIZE;

    fn new() -> Self {
        Sha256::new()
    }

    fn update(&mut self, data: &[u8]) {
        Sha256::update(self, data)
    }

    fn finalize(self) -> Digest {
        Digest::from_slice(&Sha256::finalize(self))
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

    #[test]
    fn fips_180_4_empty() {
        assert_eq!(
            hex(&Sha256::digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn fips_180_4_abc() {
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn fips_180_4_two_block_message() {
        // FIPS 180-4 section B.2 (448-bit message).
        let msg = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(
            hex(&Sha256::digest(msg)),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn fips_180_4_one_million_a() {
        // FIPS 180-4 section B.3.
        let mut h = Sha256::new();
        let chunk = [b'a'; 1000];
        for _ in 0..1000 {
            h.update(&chunk);
        }
        assert_eq!(hex(&h.finalize()), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
    }

    #[test]
    fn streaming_matches_one_shot_at_every_split() {
        let msg: alloc::vec::Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
        let expected = Sha256::digest(&msg);
        for split in 0..=msg.len() {
            let mut h = Sha256::new();
            h.update(&msg[..split]);
            h.update(&msg[split..]);
            assert_eq!(h.finalize(), expected, "mismatch splitting at {split}");
        }
    }
}
