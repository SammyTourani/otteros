//! AES-128/256 round-key expansion and single-block encryption via the AES-NI
//! instruction set (`AESKEYGENASSIST`, `AESENC`, `AESENCLAST`), the standard
//! sequence Intel's whitepaper "Advanced Encryption Standard (AES) Instructions
//! Set" documents for turning a raw key into a round-key schedule.
//!
//! Both `RoundKeys::new` constructors return `Option`, `None` when AES-NI is
//! unavailable, and are the *only* way to obtain a `RoundKeys` value -- so simply
//! holding one is proof CPUID already confirmed AES-NI support, and
//! `encrypt_block` (and the key-expansion internals) never need to check again.
//! `crate::gcm` uses this typestate directly: `Aes128RoundKeys::new(key)` returning
//! `None` is exactly "fall back to the software path".
//!
//! Every function below except the two `new` constructors and the two
//! `encrypt_block` methods carries `#[target_feature(enable = "aes")]` and is
//! *not* individually `unsafe fn`: calling one `aes`-gated intrinsic from
//! another function that itself has `#[target_feature(enable = "aes")]` needs
//! no `unsafe` (the compiler already knows the feature is enabled for that
//! whole call graph). The four `unsafe` blocks that remain are exactly the
//! boundary crossings from ordinary code into that `aes`-enabled call graph,
//! each justified by the typestate the module doc describes.

use core::arch::x86_64::{
    __m128i, _mm_aesenc_si128, _mm_aesenclast_si128, _mm_aeskeygenassist_si128, _mm_loadu_si128,
    _mm_shuffle_epi32, _mm_slli_si128, _mm_storeu_si128, _mm_xor_si128,
};

/// One expanded AES-128 key: 11 round keys (the whitened input plus 10 rounds).
#[derive(Clone, Copy)]
pub struct Aes128RoundKeys([__m128i; 11]);

/// One expanded AES-256 key: 15 round keys (the whitened input plus 14 rounds).
#[derive(Clone, Copy)]
pub struct Aes256RoundKeys([__m128i; 15]);

#[inline]
fn load(bytes: &[u8]) -> __m128i {
    // SAFETY: `_mm_loadu_si128` only requires `bytes` to have at least 16 readable
    // bytes (it is the *unaligned* load); every caller below passes a `&[u8; 16]`
    // or `&[u8]` slice of at least that length.
    unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) }
}

#[inline]
fn store(out: &mut [u8; 16], v: __m128i) {
    // SAFETY: `_mm_storeu_si128` only requires 16 writable bytes at the pointer
    // (unaligned store); `out` is exactly 16 bytes.
    unsafe { _mm_storeu_si128(out.as_mut_ptr().cast(), v) };
}

/// One step of the AES-128 key schedule (Intel whitepaper's `AES_128_ASSIST`):
/// folds `keygenassist`'s rotated-and-substituted last word (already broadcast
/// into every lane of `assisted` by the caller's `_mm_shuffle_epi32(_, 0xff)`)
/// into `prev`, XORing `prev` with itself shifted left by 4, 8 and 12 bytes so
/// every 32-bit word of the result depends on every earlier word of `prev`
/// (Rijndael's `w[i] = w[i-4] XOR w[i-1]` recurrence, done four words at a time).
#[target_feature(enable = "aes")]
fn expand_128_step(prev: __m128i, assisted: __m128i) -> __m128i {
    let assisted = _mm_shuffle_epi32(assisted, 0xff);
    let mut t = prev;
    let mut shifted = _mm_slli_si128(t, 4);
    t = _mm_xor_si128(t, shifted);
    shifted = _mm_slli_si128(shifted, 4);
    t = _mm_xor_si128(t, shifted);
    shifted = _mm_slli_si128(shifted, 4);
    t = _mm_xor_si128(t, shifted);
    _mm_xor_si128(t, assisted)
}

/// `_mm_aeskeygenassist_si128` takes its round constant as a `const` generic
/// immediate, so it cannot be called with a runtime `rcon` directly; this
/// dispatches the 10 values AES-128/256 key expansion actually uses (the
/// standard `RCON` table, `x^i` in GF(2^8) for `i = 0..=9`, plus one bit pattern
/// used only to request the SubWord-only variant AES-256 needs).
#[target_feature(enable = "aes")]
fn aeskeygenassist_rcon(w: __m128i, rcon: i32) -> __m128i {
    match rcon {
        0x00 => _mm_aeskeygenassist_si128::<0x00>(w),
        0x01 => _mm_aeskeygenassist_si128::<0x01>(w),
        0x02 => _mm_aeskeygenassist_si128::<0x02>(w),
        0x04 => _mm_aeskeygenassist_si128::<0x04>(w),
        0x08 => _mm_aeskeygenassist_si128::<0x08>(w),
        0x10 => _mm_aeskeygenassist_si128::<0x10>(w),
        0x20 => _mm_aeskeygenassist_si128::<0x20>(w),
        0x40 => _mm_aeskeygenassist_si128::<0x40>(w),
        0x80 => _mm_aeskeygenassist_si128::<0x80>(w),
        0x1b => _mm_aeskeygenassist_si128::<0x1b>(w),
        0x36 => _mm_aeskeygenassist_si128::<0x36>(w),
        _ => unreachable!("aeskeygenassist_rcon: rcon values are a fixed internal table"),
    }
}

/// AES-256 key schedule's other step (Intel whitepaper's `AES_256_ASSIST_2`):
/// unlike AES-128 and unlike the "high" half above, this half only runs
/// `SubWord` on the assist result (via `keygenassist` with `rcon = 0`, then
/// picking out its third 32-bit lane) with no `RotWord` and no XOR-in of an
/// `RCON` byte -- exactly Rijndael's extra `i % Nk == 4` case for `Nk > 6`.
#[target_feature(enable = "aes")]
fn expand_256_step2(high: __m128i, prev_low: __m128i) -> __m128i {
    let assisted = aeskeygenassist_rcon(high, 0x00);
    let sub_word = _mm_shuffle_epi32(assisted, 0xaa);
    let mut t = prev_low;
    let mut shifted = _mm_slli_si128(t, 4);
    t = _mm_xor_si128(t, shifted);
    shifted = _mm_slli_si128(shifted, 4);
    t = _mm_xor_si128(t, shifted);
    shifted = _mm_slli_si128(shifted, 4);
    t = _mm_xor_si128(t, shifted);
    _mm_xor_si128(t, sub_word)
}

#[target_feature(enable = "aes")]
fn expand_128(key: &[u8; 16]) -> [__m128i; 11] {
    const RCON: [i32; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];
    let mut rk = [load(&[0u8; 16]); 11];
    let mut w = load(key);
    rk[0] = w;
    for (i, &rcon) in RCON.iter().enumerate() {
        let assisted = aeskeygenassist_rcon(w, rcon);
        w = expand_128_step(w, assisted);
        rk[i + 1] = w;
    }
    rk
}

#[target_feature(enable = "aes")]
fn expand_256(key: &[u8; 32]) -> [__m128i; 15] {
    const RCON: [i32; 7] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40];
    let mut rk = [load(&[0u8; 16]); 15];
    let mut high = load(&key[0..16]); // w[0..4): the "even" round-key words
    let mut low = load(&key[16..32]); // w[4..8): the "odd" round-key words
    rk[0] = high;
    rk[1] = low;
    for (i, &rcon) in RCON.iter().enumerate() {
        let assisted = aeskeygenassist_rcon(low, rcon);
        high = expand_128_step(high, assisted);
        rk[2 * i + 2] = high;
        if 2 * i + 3 < 15 {
            low = expand_256_step2(high, low);
            rk[2 * i + 3] = low;
        }
    }
    rk
}

#[target_feature(enable = "aes")]
fn encrypt_128_impl(rk: &[__m128i; 11], block: __m128i) -> __m128i {
    let mut state = _mm_xor_si128(block, rk[0]);
    for &k in &rk[1..10] {
        state = _mm_aesenc_si128(state, k);
    }
    _mm_aesenclast_si128(state, rk[10])
}

#[target_feature(enable = "aes")]
fn encrypt_256_impl(rk: &[__m128i; 15], block: __m128i) -> __m128i {
    let mut state = _mm_xor_si128(block, rk[0]);
    for &k in &rk[1..14] {
        state = _mm_aesenc_si128(state, k);
    }
    _mm_aesenclast_si128(state, rk[14])
}

#[cfg(test)]
impl Aes128RoundKeys {
    /// Round key `round` as plain bytes, for the differential test against
    /// [`crate::aes`]'s software key schedule below.
    fn round_key_bytes(&self, round: usize) -> [u8; 16] {
        let mut out = [0u8; 16];
        store(&mut out, self.0[round]);
        out
    }
}

#[cfg(test)]
impl Aes256RoundKeys {
    /// See [`Aes128RoundKeys::round_key_bytes`].
    fn round_key_bytes(&self, round: usize) -> [u8; 16] {
        let mut out = [0u8; 16];
        store(&mut out, self.0[round]);
        out
    }
}

impl Aes128RoundKeys {
    /// Expands `key` into an AES-128 round-key schedule using AES-NI, or returns
    /// `None` if this CPU does not have AES-NI (`crate::hw::aes_ni_available()`).
    pub fn new(key: &[u8; 16]) -> Option<Self> {
        if !crate::hw::aes_ni_available() {
            return None;
        }
        // SAFETY: `aes_ni_available()` just confirmed CPUID leaf 1 ECX bit 25,
        // exactly `expand_128`'s (an `#[target_feature(enable = "aes")]`
        // function) precondition for actually executing AESKEYGENASSIST.
        Some(Aes128RoundKeys(unsafe { expand_128(key) }))
    }

    /// Encrypts one 16-byte block in place. Sound unconditionally: constructing
    /// `self` already proved AES-NI is present (see the module doc).
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        let input = load(block);
        // SAFETY: `self` exists, therefore AES-NI is present (typestate).
        let out = unsafe { encrypt_128_impl(&self.0, input) };
        store(block, out);
    }
}

impl Aes256RoundKeys {
    /// Expands `key` into an AES-256 round-key schedule using AES-NI, or returns
    /// `None` if this CPU does not have AES-NI (`crate::hw::aes_ni_available()`).
    pub fn new(key: &[u8; 32]) -> Option<Self> {
        if !crate::hw::aes_ni_available() {
            return None;
        }
        // SAFETY: see `Aes128RoundKeys::new`.
        Some(Aes256RoundKeys(unsafe { expand_256(key) }))
    }

    /// Encrypts one 16-byte block in place. Sound unconditionally: constructing
    /// `self` already proved AES-NI is present (see the module doc).
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        let input = load(block);
        // SAFETY: `self` exists, therefore AES-NI is present (typestate).
        let out = unsafe { encrypt_256_impl(&self.0, input) };
        store(block, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_decode(s: &str) -> alloc::vec::Vec<u8> {
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
    fn fips_197_appendix_c1_aes128() {
        let Some(rk) = Aes128RoundKeys::new(&[
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ]) else {
            eprintln!("skipping: AES-NI not available on this host");
            return;
        };
        let mut block: [u8; 16] = hex_decode("00112233445566778899aabbccddeeff").try_into().unwrap();
        rk.encrypt_block(&mut block);
        assert_eq!(hex(&block), "69c4e0d86a7b0430d8cdb78070b4c55a");
    }

    #[test]
    fn fips_197_appendix_c3_aes256() {
        let key: [u8; 32] = hex_decode(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        )
        .try_into()
        .unwrap();
        let Some(rk) = Aes256RoundKeys::new(&key) else {
            eprintln!("skipping: AES-NI not available on this host");
            return;
        };
        let mut block: [u8; 16] = hex_decode("00112233445566778899aabbccddeeff").try_into().unwrap();
        rk.encrypt_block(&mut block);
        assert_eq!(hex(&block), "8ea2b7ca516745bfeafc49904b496089");
    }

    /// Cross-checks the AES-NI key schedule byte-for-byte against
    /// `crate::aes`'s independently-derived-from-FIPS-197 software schedule:
    /// both must produce identical round keys from the same cipher key.
    #[test]
    fn aes128_round_keys_match_software_schedule() {
        let key = [0x2bu8, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c];
        let Some(hw) = Aes128RoundKeys::new(&key) else {
            eprintln!("skipping: AES-NI not available on this host");
            return;
        };
        let sw = crate::aes::RoundKeys128::new(&key);
        for round in 0..11 {
            assert_eq!(hw.round_key_bytes(round), *sw.round_key(round), "round {round} mismatch");
        }
    }

    #[test]
    fn aes256_round_keys_match_software_schedule() {
        // Any 32-byte key works here (this only cross-checks AES-NI's schedule
        // against `crate::aes`'s own for internal agreement, not against a
        // published table), so it is built from a byte pattern rather than a
        // hand-transcribed hex string.
        let key: [u8; 32] = core::array::from_fn(|i| (i as u8).wrapping_mul(17) ^ 0x5a);
        let Some(hw) = Aes256RoundKeys::new(&key) else {
            eprintln!("skipping: AES-NI not available on this host");
            return;
        };
        let sw = crate::aes::RoundKeys256::new(&key);
        for round in 0..15 {
            assert_eq!(hw.round_key_bytes(round), *sw.round_key(round), "round {round} mismatch");
        }
    }
}
