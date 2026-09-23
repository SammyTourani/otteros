//! GHASH (NIST SP 800-38D section 6.3/6.4): the polynomial universal hash GCM
//! authenticates with, over GF(2^128) with reduction polynomial
//! `f(x) = x^128 + x^7 + x^2 + x + 1`.
//!
//! GCM's bit convention is easy to get backwards: a 128-bit block `x0 x1 ...
//! x127` (`x0` the leftmost/most-significant bit of its first byte) represents
//! the field element `x0 + x1*a + x2*a^2 + ... + x127*a^127` -- the *leftmost*
//! bit is the coefficient of `a^0`, the lowest power, opposite the usual
//! "leftmost = most significant" reading. [`gf128_mul`] below is [`crate::hw`]'s
//! constant-time software fallback (used directly when PCLMULQDQ is absent, and
//! as the independent ground truth [`crate::hw::ghash`] differential-tests its
//! PCLMULQDQ path against); it works entirely in this native GCM convention,
//! following SP 800-38D's own bit-at-a-time reference algorithm (section 6.3):
//! walk `x`'s bits left to right, conditionally XOR the running `v` into the
//! result, then shift `v` right by one bit, XORing in the reduction constant
//! `R = 11100001 || 0^120` whenever the bit shifted out of `v` was a 1. Every
//! step here is mask-based (no secret-dependent branch), per DECISIONS.md D22.

/// `R = 11100001 || 0^120` (SP 800-38D section 6.3): the top byte is `0xE1`, the
/// remaining 15 bytes are zero. Written as a shift so no 32-hex-digit literal
/// has to be transcribed by hand.
const R: u128 = 0xE1u128 << 120;

/// Constant-time GF(2^128) multiplication in GCM's bit convention (`x`/`y` as
/// produced by `u128::from_be_bytes` on a 16-byte block). No secret-indexed
/// table, no secret-dependent branch: every step is a fixed shift plus a
/// mask-selected XOR (`ct`-style, see `crate::ct`'s module doc), run for a fixed
/// 128 iterations regardless of the inputs.
pub fn gf128_mul(x: u128, y: u128) -> u128 {
    let mut z: u128 = 0;
    let mut v = y;
    let mut i = 0u32;
    while i < 128 {
        let xi = (x >> (127 - i)) & 1;
        let select = 0u128.wrapping_sub(xi); // all-ones if this bit of x is 1, else 0.
        z ^= v & select;

        let carry_out = 0u128.wrapping_sub(v & 1); // all-ones if v's LSB is 1, else 0.
        v >>= 1;
        v ^= R & carry_out;

        i += 1;
    }
    z
}

/// Multiplies `x` by the (secret) subkey `h`, using PCLMULQDQ when available.
fn mul(x: u128, h: u128, use_hw: bool) -> u128 {
    if use_hw {
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        if let Some(v) = crate::hw::ghash::mul(x, h) {
            return v;
        }
    }
    gf128_mul(x, h)
}

/// Streaming GHASH: absorbs whole 16-byte blocks (callers are responsible for
/// zero-padding a final partial block, exactly like [`crate::poly1305::Poly1305`]'s
/// caller-driven `pad16` in [`crate::aead`] -- GCM's block structure, unlike
/// Poly1305's, never needs GHASH itself to buffer a partial block across calls).
pub struct Ghash {
    h: u128,
    y: u128,
    use_hw: bool,
}

impl Ghash {
    /// `h_block` is `E_K(0^128)`, GCM's per-key hash subkey.
    pub fn new(h_block: [u8; 16], use_hw: bool) -> Self {
        Ghash { h: u128::from_be_bytes(h_block), y: 0, use_hw }
    }

    /// Absorbs one 16-byte block: `Y_i = (Y_{i-1} XOR block) . H`.
    pub fn absorb(&mut self, block: &[u8; 16]) {
        self.y ^= u128::from_be_bytes(*block);
        self.y = mul(self.y, self.h, self.use_hw);
    }

    /// Absorbs `data`, zero-padding a final short block up to 16 bytes (SP
    /// 800-38D's implicit zero-padding of AAD/ciphertext before the length block).
    pub fn absorb_padded(&mut self, data: &[u8]) {
        let (chunks, rest) = data.as_chunks::<16>();
        for chunk in chunks {
            self.absorb(chunk);
        }
        if !rest.is_empty() {
            let mut block = [0u8; 16];
            block[..rest.len()].copy_from_slice(rest);
            self.absorb(&block);
        }
    }

    /// Consumes the state, returning the final GHASH output block.
    pub fn finalize(self) -> [u8; 16] {
        self.y.to_be_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An independent reference implementation used only to differential-test
    /// [`gf128_mul`]: full schoolbook convolution (every pair of set bits XORs
    /// `x^(i+j)` into a 255-coefficient accumulator) followed by explicit
    /// top-down polynomial long division by `f(x) = x^128+x^7+x^2+x+1`. This
    /// shares no code, and not even the same algorithmic shape, with
    /// `gf128_mul`'s shift-register method, so an agreement between the two is
    /// real evidence rather than two copies of the same bug.
    fn gf128_mul_naive_reference(x: u128, y: u128) -> u128 {
        let xb = |i: usize| (x >> (127 - i)) & 1 == 1;
        let yb = |i: usize| (y >> (127 - i)) & 1 == 1;

        let mut product = [false; 255]; // coefficients of x^0 .. x^254.
        for i in 0..128 {
            if !xb(i) {
                continue;
            }
            for j in 0..128 {
                if yb(j) {
                    product[i + j] ^= true;
                }
            }
        }

        // x^128 = x^7 + x^2 + x + 1 (mod f): fold every bit at degree >= 128 down,
        // highest first, so a fold's own output (always strictly lower-degree,
        // since 7 < 128) is visited later in this same descending sweep.
        for k in (128..255).rev() {
            if product[k] {
                product[k] = false;
                let base = k - 128;
                product[base + 7] ^= true;
                product[base + 2] ^= true;
                product[base + 1] ^= true;
                product[base] ^= true;
            }
        }

        let mut result: u128 = 0;
        for (k, &bit) in product.iter().enumerate().take(128) {
            if bit {
                result |= 1 << (127 - k);
            }
        }
        result
    }

    #[test]
    fn naive_reference_agrees_with_itself_on_the_identity() {
        // GCM convention: the block `00...01` (a single 1 in the *last* bit)
        // represents the field element `a^127`; multiplying by the convention's
        // "one" element (`80 00 ... 00`, a single 1 in the *first* bit, i.e. the
        // coefficient of `a^0`) must be the identity.
        let one = 1u128 << 127;
        let x = 0x0123456789abcdef0123456789abcdefu128;
        assert_eq!(gf128_mul_naive_reference(x, one), x);
        assert_eq!(gf128_mul(x, one), x);
    }

    #[test]
    fn constant_time_multiply_matches_naive_reference() {
        let mut state = 0x9e3779b97f4a7c15f39cc0605cedc835u128; // arbitrary seed (public).
        for _ in 0..5000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = state;
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let y = state;
            assert_eq!(gf128_mul(x, y), gf128_mul_naive_reference(x, y), "mismatch for x={x:#x} y={y:#x}");
        }
    }

    #[test]
    fn zero_is_absorbing() {
        assert_eq!(gf128_mul(0, 0x42), 0);
        assert_eq!(gf128_mul(0x42, 0), 0);
    }

    #[test]
    fn streaming_ghash_matches_hand_computed_two_block_example() {
        // Y0 = 0; Y1 = (Y0 XOR block0).H; Y2 = (Y1 XOR block1).H -- checked
        // directly against two independent calls to `gf128_mul`.
        let h = [0x66u8; 16];
        let block0 = [0x11u8; 16];
        let block1 = [0x22u8; 16];
        let mut g = Ghash::new(h, false);
        g.absorb(&block0);
        g.absorb(&block1);
        let got = g.finalize();

        let hv = u128::from_be_bytes(h);
        let y1 = gf128_mul(u128::from_be_bytes(block0), hv);
        let y2 = gf128_mul(y1 ^ u128::from_be_bytes(block1), hv);
        assert_eq!(got, y2.to_be_bytes());
    }

    #[test]
    fn absorb_padded_zero_pads_a_short_final_block() {
        let h = [0x77u8; 16];
        let mut a = Ghash::new(h, false);
        a.absorb_padded(b"exactly16bytes!!");
        a.absorb_padded(b"short");
        let a_out = a.finalize();

        let mut b = Ghash::new(h, false);
        b.absorb(b"exactly16bytes!!");
        let mut padded_short = [0u8; 16];
        padded_short[..5].copy_from_slice(b"short");
        b.absorb(&padded_short);
        assert_eq!(a_out, b.finalize());
    }
}
