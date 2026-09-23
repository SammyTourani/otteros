//! Constant-time software AES-128/256, encryption only (FIPS 197). GCM only
//! ever needs the forward cipher (its own CTR-mode keystream and its GHASH
//! subkey `E_K(0)`), never the inverse cipher.
//!
//! `SubBytes` is the step every non-constant-time AES implementation gets wrong
//! by indexing a 256-entry table with a secret byte (a textbook cache-timing
//! side channel). Here the S-box is instead *computed*: FIPS 197 defines it as
//! "take the multiplicative inverse in GF(2^8) (0 maps to itself), then apply a
//! fixed affine transformation" -- both steps are ordinary arithmetic on the
//! byte value, not a lookup, so nothing here is ever indexed by secret data.
//! The GF(2^8) inverse of `x` is `x^254` (Fermat: `x^255 = 1` for `x != 0`, and
//! `0^254 = 0` handles the `x = 0` case for free, no branch needed): since
//! `254 = 2^1+2^2+...+2^7`, `x^254` is the product of the seven values `x^(2^k)`
//! for `k = 1..=7`, each just the previous one squared -- seven squarings and
//! six multiplications, always, regardless of `x`. `ShiftRows`/`MixColumns`/
//! `AddRoundKey` were never table-based to begin with; `MixColumns`'s GF(2^8)
//! multiplications by the fixed constants 2 and 3 use the standard `xtime`
//! shift-and-conditionally-XOR-0x1B construction (mask-based, not a branch).
//!
//! State is kept as a plain `[u8; 16]` in the same byte order as a 16-byte
//! block; FIPS 197's `state[r][c]` is `state[r + 4*c]` here (column-major, the
//! order the standard's own examples use).

/// Constant-time GF(2^8) multiplication (AES's field, reduction polynomial
/// `x^8+x^4+x^3+x+1 = 0x11B`) via shift-and-mask peasant multiplication: no
/// secret-indexed table, no secret-dependent branch.
fn gf256_mul(a: u8, b: u8) -> u8 {
    let mut result: u8 = 0;
    let mut a = a;
    let mut b = b;
    for _ in 0..8 {
        let select = 0u8.wrapping_sub(b & 1); // all-ones if this bit of b is set.
        result ^= a & select;
        let carry = 0u8.wrapping_sub(a >> 7); // all-ones if a's top bit is set.
        a = (a << 1) ^ (0x1B & carry);
        b >>= 1;
    }
    result
}

/// The GF(2^8) multiplicative inverse of `x` (`0` maps to `0`), via `x^254 =
/// x^2 * x^4 * x^8 * x^16 * x^32 * x^64 * x^128` -- see the module doc.
fn gf256_inv(x: u8) -> u8 {
    let mut powers = [0u8; 7]; // powers[k] = x^(2^(k+1)), k = 0..6.
    powers[0] = gf256_mul(x, x);
    for k in 1..7 {
        powers[k] = gf256_mul(powers[k - 1], powers[k - 1]);
    }
    let mut result = powers[0];
    for &p in &powers[1..] {
        result = gf256_mul(result, p);
    }
    result
}

/// FIPS 197's S-box affine transformation applied to an already-inverted byte:
/// `s_i = b_i XOR b_(i+4 mod 8) XOR b_(i+5 mod 8) XOR b_(i+6 mod 8) XOR b_(i+7
/// mod 8) XOR c_i` (`c = 0x63`), equivalently `b XOR rotl(b,1) XOR rotl(b,2)
/// XOR rotl(b,3) XOR rotl(b,4) XOR 0x63`.
fn affine(b: u8) -> u8 {
    b ^ b.rotate_left(1) ^ b.rotate_left(2) ^ b.rotate_left(3) ^ b.rotate_left(4) ^ 0x63
}

/// The AES S-box, computed rather than looked up (see the module doc).
fn sbox(x: u8) -> u8 {
    affine(gf256_inv(x))
}

fn sub_bytes(state: &mut [u8; 16]) {
    for b in state.iter_mut() {
        *b = sbox(*b);
    }
}

/// Row `r` (`r = 0..4`) of the state cyclically shifts left by `r` columns:
/// `state'[r][c] = state[r][(c+r) mod 4]`.
fn shift_rows(state: &mut [u8; 16]) {
    let orig = *state;
    for c in 0..4 {
        for r in 0..4 {
            state[r + 4 * c] = orig[r + 4 * ((c + r) % 4)];
        }
    }
}

/// GF(2^8) multiplication by `x` (`{02}`): shift left one bit, XOR in the
/// reduction constant `0x1B` if a bit fell off the top.
fn xtime(x: u8) -> u8 {
    let carry = 0u8.wrapping_sub(x >> 7);
    (x << 1) ^ (0x1B & carry)
}

/// GF(2^8) multiplication by `{03} = x + 1`.
fn gmul3(x: u8) -> u8 {
    xtime(x) ^ x
}

/// FIPS 197's `MixColumns`: each column, read as a degree-<4 polynomial over
/// GF(2^8), is multiplied by the fixed polynomial `{03}x^3+{01}x^2+{01}x+{02}`
/// modulo `x^4+1`.
fn mix_columns(state: &mut [u8; 16]) {
    for c in 0..4 {
        let a0 = state[4 * c];
        let a1 = state[4 * c + 1];
        let a2 = state[4 * c + 2];
        let a3 = state[4 * c + 3];
        state[4 * c] = xtime(a0) ^ gmul3(a1) ^ a2 ^ a3;
        state[4 * c + 1] = a0 ^ xtime(a1) ^ gmul3(a2) ^ a3;
        state[4 * c + 2] = a0 ^ a1 ^ xtime(a2) ^ gmul3(a3);
        state[4 * c + 3] = gmul3(a0) ^ a1 ^ a2 ^ xtime(a3);
    }
}

fn add_round_key(state: &mut [u8; 16], round_key: &[u8; 16]) {
    for i in 0..16 {
        state[i] ^= round_key[i];
    }
}

fn sub_word(w: [u8; 4]) -> [u8; 4] {
    [sbox(w[0]), sbox(w[1]), sbox(w[2]), sbox(w[3])]
}

fn rot_word(w: [u8; 4]) -> [u8; 4] {
    [w[1], w[2], w[3], w[0]]
}

/// Packs a flat word schedule (`Nb*(Nr+1)` 4-byte words) into `Nr+1` 16-byte
/// round keys, column by column (word `4*round + col` is column `col` of round
/// key `round`), matching this module's column-major `state[r + 4*c]` layout.
fn words_to_round_keys<const WORDS: usize, const ROUNDS: usize>(
    w: &[[u8; 4]; WORDS],
) -> [[u8; 16]; ROUNDS] {
    let mut rk = [[0u8; 16]; ROUNDS];
    for (round, key) in rk.iter_mut().enumerate() {
        for c in 0..4 {
            key[4 * c..4 * c + 4].copy_from_slice(&w[4 * round + c]);
        }
    }
    rk
}

/// `Rcon[i]` for AES's key schedule: `x^(i-1)` in GF(2^8), `i = 1..=10`
/// (AES-128 uses all ten; AES-256 uses only the first seven).
const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1B, 0x36];

/// An expanded AES-128 key schedule: 11 round keys.
#[derive(Clone, Copy)]
pub struct RoundKeys128([[u8; 16]; 11]);

impl RoundKeys128 {
    /// FIPS 197 section 5.2's key expansion for `Nk = 4`, `Nr = 10`.
    pub fn new(key: &[u8; 16]) -> Self {
        let mut w = [[0u8; 4]; 44];
        for (i, word) in w.iter_mut().enumerate().take(4) {
            *word = key[4 * i..4 * i + 4].try_into().unwrap();
        }
        for i in 4..44 {
            let mut temp = w[i - 1];
            if i % 4 == 0 {
                temp = sub_word(rot_word(temp));
                temp[0] ^= RCON[i / 4 - 1];
            }
            for j in 0..4 {
                w[i][j] = w[i - 4][j] ^ temp[j];
            }
        }
        RoundKeys128(words_to_round_keys(&w))
    }

    pub(crate) fn round_key(&self, round: usize) -> &[u8; 16] {
        &self.0[round]
    }
}

/// An expanded AES-256 key schedule: 15 round keys.
#[derive(Clone, Copy)]
pub struct RoundKeys256([[u8; 16]; 15]);

impl RoundKeys256 {
    /// FIPS 197 section 5.2's key expansion for `Nk = 8`, `Nr = 14` (the
    /// `Nk > 6` variant: an extra `SubWord`-only step at `i % Nk == 4`).
    pub fn new(key: &[u8; 32]) -> Self {
        let mut w = [[0u8; 4]; 60];
        for (i, word) in w.iter_mut().enumerate().take(8) {
            *word = key[4 * i..4 * i + 4].try_into().unwrap();
        }
        for i in 8..60 {
            let mut temp = w[i - 1];
            if i % 8 == 0 {
                temp = sub_word(rot_word(temp));
                temp[0] ^= RCON[i / 8 - 1];
            } else if i % 8 == 4 {
                temp = sub_word(temp);
            }
            for j in 0..4 {
                w[i][j] = w[i - 8][j] ^ temp[j];
            }
        }
        RoundKeys256(words_to_round_keys(&w))
    }

    pub(crate) fn round_key(&self, round: usize) -> &[u8; 16] {
        &self.0[round]
    }
}

/// Encrypts one 16-byte block in place under an AES-128 schedule.
pub fn encrypt_block_128(rk: &RoundKeys128, block: &mut [u8; 16]) {
    add_round_key(block, rk.round_key(0));
    for round in 1..10 {
        sub_bytes(block);
        shift_rows(block);
        mix_columns(block);
        add_round_key(block, rk.round_key(round));
    }
    sub_bytes(block);
    shift_rows(block);
    add_round_key(block, rk.round_key(10));
}

/// Encrypts one 16-byte block in place under an AES-256 schedule.
pub fn encrypt_block_256(rk: &RoundKeys256, block: &mut [u8; 16]) {
    add_round_key(block, rk.round_key(0));
    for round in 1..14 {
        sub_bytes(block);
        shift_rows(block);
        mix_columns(block);
        add_round_key(block, rk.round_key(round));
    }
    sub_bytes(block);
    shift_rows(block);
    add_round_key(block, rk.round_key(14));
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
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

    #[test]
    fn sbox_known_values() {
        // FIPS 197's own worked example (section 5.1.1) computes S(0x53) = 0xed;
        // 0x00/0x01 are the S-box table's first two published entries.
        assert_eq!(sbox(0x00), 0x63);
        assert_eq!(sbox(0x01), 0x7c);
        assert_eq!(sbox(0x53), 0xed);
    }

    #[test]
    fn gf256_inverse_edge_cases() {
        assert_eq!(gf256_inv(0), 0);
        assert_eq!(gf256_inv(1), 1);
        // Every nonzero element's inverse, multiplied back by itself, is 1.
        for x in 1u8..=255 {
            assert_eq!(gf256_mul(x, gf256_inv(x)), 1, "x={x:#x}");
        }
    }

    #[test]
    fn fips_197_appendix_c1_aes128() {
        let key: [u8; 16] =
            hex_decode("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let mut block: [u8; 16] =
            hex_decode("00112233445566778899aabbccddeeff").try_into().unwrap();
        let rk = RoundKeys128::new(&key);
        encrypt_block_128(&rk, &mut block);
        assert_eq!(hex(&block), "69c4e0d86a7b0430d8cdb78070b4c55a");
    }

    #[test]
    fn fips_197_appendix_c3_aes256() {
        let key: [u8; 32] = hex_decode(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        )
        .try_into()
        .unwrap();
        let mut block: [u8; 16] =
            hex_decode("00112233445566778899aabbccddeeff").try_into().unwrap();
        let rk = RoundKeys256::new(&key);
        encrypt_block_256(&rk, &mut block);
        assert_eq!(hex(&block), "8ea2b7ca516745bfeafc49904b496089");
    }

    #[test]
    fn round_key_zero_is_the_raw_key() {
        let key = [0x2bu8, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
        let rk = RoundKeys128::new(&key);
        assert_eq!(*rk.round_key(0), key);
    }
}
