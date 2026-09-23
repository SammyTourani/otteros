//! X25519 (RFC 7748): the Diffie-Hellman function on Curve25519, TLS 1.3's
//! (and this brief's) key exchange.
//!
//! Field arithmetic works in GF(2^255-19) using five 51-bit limbs held in
//! `u64`s (`u128` for the intermediate products a multiply/square needs) --
//! the standard "radix 2^51" representation, chosen (per the brief) because
//! `51*5 = 255` divides the field evenly and every partial product of two
//! limbs (`< 2^51 * 2^51 = 2^102`) fits comfortably in a `u128` accumulator
//! with room to spare for summing several of them before reducing. Elements
//! are kept in a *loose* form between operations (each limb a little over its
//! nominal 51 bits is fine -- every operation's `u128` accumulators have
//! headroom for that slack, and it never grows across chained operations,
//! since [`Fe::carry_reduce`] resets it after every multiply/add/sub); only
//! [`Fe::to_bytes`] fully canonicalizes (conditionally subtracting `p` once,
//! constant-time, the same masked-borrow-chain technique
//! [`crate::poly1305::Poly1305::finalize`] uses for its own modulus).
//!
//! The Montgomery ladder (RFC 7748 section 5) is section 5's pseudocode
//! directly: a constant-time conditional swap ([`Fe::cswap`]) driven by each
//! scalar bit, in a fixed 255 iterations regardless of the scalar's value.
//! Field inversion (needed once, at the very end, to turn `X2/Z2` into an
//! affine `u`-coordinate) is Fermat's little theorem, `z^(p-2)`, via ordinary
//! left-to-right square-and-multiply over the *public* constant `p-2`'s bits
//! (branching on a fixed public exponent leaks nothing about the secret base,
//! unlike branching on secret data -- the same reasoning [`crate::hw`]'s
//! typestate relies on, just applied to control flow instead of a type).

use crate::ct;

/// `2^51 - 1`: the mask that keeps one limb.
const MASK51: u64 = (1 << 51) - 1;

/// `p - 2 = 2^255 - 21`, as four 64-bit little-endian limbs, for the Fermat
/// inversion exponent below. Written out in full because deriving it via
/// shifts (unlike this module's other constants) would obscure, rather than
/// clarify, that it really is `p - 2`; instead it is cross-checked in a unit
/// test with genuine 256-bit subtraction from `2^255 - 1`.
const P_MINUS_2: [u64; 4] = [
    0xFFFF_FFFF_FFFF_FFEB,
    0xFFFF_FFFF_FFFF_FFFF,
    0xFFFF_FFFF_FFFF_FFFF,
    0x7FFF_FFFF_FFFF_FFFF,
];

/// A field element mod `2^255 - 19`, five `u64` limbs of (loosely) 51 bits
/// each, least-significant limb first.
#[derive(Clone, Copy, Debug)]
struct Fe([u64; 5]);

impl Fe {
    const ZERO: Fe = Fe([0, 0, 0, 0, 0]);
    const ONE: Fe = Fe([1, 0, 0, 0, 0]);

    /// Loads a field element from 32 little-endian bytes, per RFC 7748
    /// section 5's decode: bit 255 (the top bit of the last byte) is masked
    /// off, which this does simply by never reading it -- the loop below
    /// only ever touches bits `0..255`.
    fn from_bytes(b: &[u8; 32]) -> Fe {
        let mut limbs = [0u64; 5];
        for (limb_idx, limb) in limbs.iter_mut().enumerate() {
            for bit in 0..51 {
                let global_bit = limb_idx * 51 + bit;
                let byte = b[global_bit / 8];
                let bit_value = (byte >> (global_bit % 8)) & 1;
                *limb |= (bit_value as u64) << bit;
            }
        }
        Fe(limbs)
    }

    /// Fully canonicalizes (one more exact carry pass, then a constant-time
    /// conditional subtraction of `p` if the value is still `>= p`) and
    /// serializes as 32 little-endian bytes. Takes `self` by value (`Fe` is
    /// `Copy`) per Rust's `to_*`-methods-take-`self`-by-value convention.
    fn to_bytes(self) -> [u8; 32] {
        let l = Fe::carry_reduce([
            self.0[0] as u128,
            self.0[1] as u128,
            self.0[2] as u128,
            self.0[3] as u128,
            self.0[4] as u128,
        ]);

        const P: [u64; 5] = [(1u64 << 51) - 19, MASK51, MASK51, MASK51, MASK51];
        let mut diff = [0i64; 5];
        let mut borrow: i64 = 0;
        for i in 0..5 {
            let d = l[i] as i64 - P[i] as i64 - borrow;
            borrow = (d >> 63) & 1;
            diff[i] = d + ((d >> 63) & (1i64 << 51));
        }
        let use_diff = (1 - borrow) as u64; // 1 when l >= p (no final borrow).
        let select_mask = 0u64.wrapping_sub(use_diff);
        let mut canon = [0u64; 5];
        for i in 0..5 {
            canon[i] = (diff[i] as u64 & select_mask) | (l[i] & !select_mask);
        }

        let mut bytes = [0u8; 32];
        for (limb_idx, &limb) in canon.iter().enumerate() {
            for bit in 0..51 {
                let global_bit = limb_idx * 51 + bit;
                let bit_value = (limb >> bit) & 1;
                bytes[global_bit / 8] |= (bit_value as u8) << (global_bit % 8);
            }
        }
        bytes
    }

    /// Propagates carries out of five `u128` accumulators (as produced by a
    /// multiply's summed partial products, or simply widened limbs after an
    /// add/sub) into five clean-ish `u64` limbs, wrapping the top limb's
    /// carry back into the bottom one scaled by 19 (`2^255 = 19 (mod p)`).
    /// Two full rounds: the first brings every limb close to canonical, the
    /// second (whose carries are then tiny) finishes the job.
    fn carry_reduce(mut t: [u128; 5]) -> [u64; 5] {
        let mask = MASK51 as u128;
        for _ in 0..2 {
            let mut c;
            c = t[0] >> 51;
            t[0] &= mask;
            t[1] += c;
            c = t[1] >> 51;
            t[1] &= mask;
            t[2] += c;
            c = t[2] >> 51;
            t[2] &= mask;
            t[3] += c;
            c = t[3] >> 51;
            t[3] &= mask;
            t[4] += c;
            c = t[4] >> 51;
            t[4] &= mask;
            t[0] += c * 19;
        }
        [t[0] as u64, t[1] as u64, t[2] as u64, t[3] as u64, t[4] as u64]
    }

    fn add(&self, other: &Fe) -> Fe {
        let t = [
            self.0[0] as u128 + other.0[0] as u128,
            self.0[1] as u128 + other.0[1] as u128,
            self.0[2] as u128 + other.0[2] as u128,
            self.0[3] as u128 + other.0[3] as u128,
            self.0[4] as u128 + other.0[4] as u128,
        ];
        Fe(Fe::carry_reduce(t))
    }

    /// `self - other`: adds a generous multiple of `p` limb-wise first (so no
    /// limb subtraction can go negative even with this representation's
    /// normal looseness), then lets `carry_reduce` fold the extra `k*p` back
    /// out (`p = 0 (mod p)`, so it never changes the value).
    fn sub(&self, other: &Fe) -> Fe {
        const K: u128 = 16;
        const P0: u128 = (1u128 << 51) - 19;
        const PN: u128 = (1u128 << 51) - 1;
        let t = [
            self.0[0] as u128 + K * P0 - other.0[0] as u128,
            self.0[1] as u128 + K * PN - other.0[1] as u128,
            self.0[2] as u128 + K * PN - other.0[2] as u128,
            self.0[3] as u128 + K * PN - other.0[3] as u128,
            self.0[4] as u128 + K * PN - other.0[4] as u128,
        ];
        Fe(Fe::carry_reduce(t))
    }

    /// Schoolbook multiplication of two 5-limb polynomials, reducing degree
    /// `>= 5` terms with `2^(51*5) = 2^255 = 19 (mod p)` as they are formed
    /// (see the module doc for the derivation of each output limb's terms).
    fn mul(&self, other: &Fe) -> Fe {
        let a = self.0;
        let b = other.0;
        let b1_19 = b[1] * 19;
        let b2_19 = b[2] * 19;
        let b3_19 = b[3] * 19;
        let b4_19 = b[4] * 19;
        let m = |x: u64, y: u64| (x as u128) * (y as u128);

        let t = [
            m(a[0], b[0]) + m(a[1], b4_19) + m(a[2], b3_19) + m(a[3], b2_19) + m(a[4], b1_19),
            m(a[0], b[1]) + m(a[1], b[0]) + m(a[2], b4_19) + m(a[3], b3_19) + m(a[4], b2_19),
            m(a[0], b[2]) + m(a[1], b[1]) + m(a[2], b[0]) + m(a[3], b4_19) + m(a[4], b3_19),
            m(a[0], b[3]) + m(a[1], b[2]) + m(a[2], b[1]) + m(a[3], b[0]) + m(a[4], b4_19),
            m(a[0], b[4]) + m(a[1], b[3]) + m(a[2], b[2]) + m(a[3], b[1]) + m(a[4], b[0]),
        ];
        Fe(Fe::carry_reduce(t))
    }

    fn sq(&self) -> Fe {
        self.mul(self)
    }

    /// Multiplies by a small (< 2^17, in practice always 121665) public
    /// constant.
    fn mul_small(&self, k: u64) -> Fe {
        let t = [
            self.0[0] as u128 * k as u128,
            self.0[1] as u128 * k as u128,
            self.0[2] as u128 * k as u128,
            self.0[3] as u128 * k as u128,
            self.0[4] as u128 * k as u128,
        ];
        Fe(Fe::carry_reduce(t))
    }

    /// `self^(p-2) = self^-1` (Fermat), for `self != 0`; `0^(p-2) = 0`, which
    /// is exactly the convention the ladder needs when `Z2` legitimately ends
    /// up `0` (only possible for the low-order inputs `x25519` rejects anyway).
    fn invert(&self) -> Fe {
        let mut result = Fe::ONE;
        for i in (0..256).rev() {
            result = result.sq();
            if (P_MINUS_2[i / 64] >> (i % 64)) & 1 == 1 {
                result = result.mul(self);
            }
        }
        result
    }

    /// Constant-time conditional swap of `a` and `b` when `swap & 1 == 1`
    /// (RFC 7748 section 5's `cswap`).
    fn cswap(swap: u64, a: &mut Fe, b: &mut Fe) {
        let mask = 0u64.wrapping_sub(swap & 1);
        for i in 0..5 {
            let t = mask & (a.0[i] ^ b.0[i]);
            a.0[i] ^= t;
            b.0[i] ^= t;
        }
    }
}

/// `x25519`/`x25519_base` failed: the computed shared secret was the all-zero
/// string. RFC 7748 section 6.1 requires checking for this (it happens when
/// the peer supplied a low-order point) and TLS 1.3 (RFC 8446 section 7.4.1)
/// requires aborting the handshake if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X25519Error {
    /// The peer's `u`-coordinate was a low-order point.
    LowOrderPoint,
}

/// RFC 7748 section 5's clamping: clear the low 3 bits (a multiple of the
/// cofactor 8), clear the top bit and set the second-highest bit (keeps the
/// scalar in `[2^254, 2^255)`, fixed high bit for the ladder's constant
/// iteration count).
fn clamp(scalar: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;
    k
}

/// The Montgomery ladder (RFC 7748 section 5), scanning the clamped scalar's
/// 255 meaningful bits (254 down to 0) most significant first.
fn ladder(k: &[u8; 32], u: Fe) -> Fe {
    let x1 = u;
    let mut x2 = Fe::ONE;
    let mut z2 = Fe::ZERO;
    let mut x3 = u;
    let mut z3 = Fe::ONE;
    let mut swap: u64 = 0;

    for t in (0..255).rev() {
        let kt = ((k[t / 8] >> (t % 8)) & 1) as u64;
        swap ^= kt;
        Fe::cswap(swap, &mut x2, &mut x3);
        Fe::cswap(swap, &mut z2, &mut z3);
        swap = kt;

        let a = x2.add(&z2);
        let aa = a.sq();
        let b = x2.sub(&z2);
        let bb = b.sq();
        let e = aa.sub(&bb);
        let c = x3.add(&z3);
        let d = x3.sub(&z3);
        let da = d.mul(&a);
        let cb = c.mul(&b);
        x3 = da.add(&cb).sq();
        z3 = x1.mul(&da.sub(&cb).sq());
        x2 = aa.mul(&bb);
        z2 = e.mul(&aa.add(&e.mul_small(121665)));
    }
    Fe::cswap(swap, &mut x2, &mut x3);
    Fe::cswap(swap, &mut z2, &mut z3);

    x2.mul(&z2.invert())
}

/// `X25519(scalar, u)`: the raw Diffie-Hellman function. `scalar` is clamped
/// internally (callers pass their raw 32-byte private key, not a pre-clamped
/// one). Returns [`X25519Error::LowOrderPoint`] if the result is the all-zero
/// string instead of returning it (RFC 7748 section 6.1).
pub fn x25519(scalar: &[u8; 32], u_point: &[u8; 32]) -> Result<[u8; 32], X25519Error> {
    let k = clamp(scalar);
    let u = Fe::from_bytes(u_point);
    let out = ladder(&k, u).to_bytes();
    if ct::ct_eq(&out, &[0u8; 32]) {
        return Err(X25519Error::LowOrderPoint);
    }
    Ok(out)
}

/// `X25519(scalar, 9)`: derives the public key for `scalar` from the
/// Curve25519 base point.
pub fn x25519_base(scalar: &[u8; 32]) -> Result<[u8; 32], X25519Error> {
    let mut base = [0u8; 32];
    base[0] = 9;
    x25519(scalar, &base)
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

    fn arr32(s: &str) -> [u8; 32] {
        hex_decode(s).try_into().unwrap()
    }

    #[test]
    fn p_minus_2_is_really_2_pow_255_minus_21() {
        // 2^255 - 1 is 255 one-bits; subtracting 20 more (to reach p-2 =
        // 2^255-19-2 = 2^255-21) only ever borrows within the bottom limb
        // since 20 < 2^64, independently confirming the hardcoded constant.
        let all_ones_255 = [u64::MAX, u64::MAX, u64::MAX, (1u64 << 63) - 1];
        let mut expected = all_ones_255;
        expected[0] -= 20;
        assert_eq!(expected, P_MINUS_2);
    }

    #[test]
    fn field_round_trips_through_bytes() {
        let mut state = 0xabcdef0123456789u64;
        for _ in 0..200 {
            let mut b = [0u8; 32];
            for byte in b.iter_mut() {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                *byte = (state >> 24) as u8;
            }
            b[31] &= 0x7f; // canonical field elements never set the top bit.
            let fe = Fe::from_bytes(&b);
            // Values >= p do not round-trip byte-for-byte (they reduce), but a
            // value already < p (extremely likely for a random 255-bit value,
            // since p is only 19 less than 2^255) must.
            let back = fe.to_bytes();
            let mut is_canonical_input = true;
            // A cheap sufficient check: if any of the top bits that would make
            // it >= p aren't all set, it's certainly < p.
            if b[31] == 0x7f && b[0] >= 0xed {
                is_canonical_input = false; // near the boundary; skip (rare).
            }
            if is_canonical_input {
                assert_eq!(back, b, "round-trip mismatch");
            }
        }
    }

    #[test]
    fn field_arithmetic_sanity() {
        let mut two_bytes = [0u8; 32];
        two_bytes[0] = 2;
        let a = Fe::from_bytes(&two_bytes);
        let one = Fe::ONE;
        assert_eq!(a.add(&Fe::ZERO).to_bytes(), a.to_bytes());
        assert_eq!(a.mul(&one).to_bytes(), a.to_bytes());
        assert_eq!(a.sub(&a).to_bytes(), Fe::ZERO.to_bytes());
        // 2 * inv(2) == 1.
        assert_eq!(a.mul(&a.invert()).to_bytes(), one.to_bytes());
    }

    /// RFC 7748 section 5.2's iterated self-composition test: starting from
    /// `k = u = 9`, repeatedly set `(k, u) = (X25519(k, u), k)`. The expected
    /// values after 1 and 1000 iterations below are the ones RFC 7748 itself
    /// publishes, independently reconfirmed against a local libcrypto/openssl
    /// X25519 oracle during development (not committed; see the brief's
    /// report) rather than trusted from memory alone.
    #[test]
    fn rfc7748_section_5_2_iterated_1_and_1000() {
        let base = arr32("0900000000000000000000000000000000000000000000000000000000000000");
        let mut k = base;
        let mut u = base;

        let next = x25519(&k, &u).expect("not a low-order point");
        u = k;
        k = next;
        assert_eq!(hex(&k), "422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079", "after 1 iteration");

        for _ in 1..1000 {
            let next = x25519(&k, &u).expect("not a low-order point");
            u = k;
            k = next;
        }
        assert_eq!(hex(&k), "684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51", "after 1000 iterations");
    }

    /// As above, extended to 1,000,000 iterations. `#[ignore]`d: too slow for
    /// routine `cargo test` (run explicitly with `--ignored`). Unlike the 1-
    /// and 1000-iteration checks above, this specific expected value was not
    /// independently reconfirmed against the local oracle (1,000,000
    /// subprocess-based scalar multiplications was impractically slow to
    /// script); it is RFC 7748's own widely-republished published value.
    #[test]
    #[ignore] // slow: 1,000,000 scalar multiplications.
    fn rfc7748_section_5_2_iterated_1_000_000() {
        let mut k = arr32("0900000000000000000000000000000000000000000000000000000000000000");
        let mut u = k;
        for _ in 0..1_000_000 {
            let next = x25519(&k, &u).expect("not a low-order point");
            u = k;
            k = next;
        }
        assert_eq!(hex(&k), "7c3911e0ab2586fd864497297e575e6f3bc601c0883c30df5f4dd2d24f665424");
    }

    #[test]
    fn low_order_u_zero_is_rejected() {
        let scalar = [0x42u8; 32];
        assert_eq!(x25519(&scalar, &[0u8; 32]), Err(X25519Error::LowOrderPoint));
    }

    #[test]
    fn base_point_helper_matches_explicit_base_point() {
        let scalar = arr32("a046e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4");
        let mut base = [0u8; 32];
        base[0] = 9;
        assert_eq!(x25519_base(&scalar), x25519(&scalar, &base));
    }
}
