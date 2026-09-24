//! Fixed-capacity big integers for RSA and elliptic-curve signature
//! *verification* (brief M8-T3): big-endian byte conversion, comparison,
//! Montgomery multiplication/reduction and modular exponentiation.
//!
//! Every value this module ever handles is public (a modulus, a public
//! exponent, a signature, a curve coordinate) -- a TLS client verifies, it
//! never holds an RSA/EC private key -- so, unlike the rest of this crate,
//! none of this needs to be constant-time (DECISIONS.md D22 requires that
//! only for *secret-dependent* code). [`Modulus::pow_mod`] below is ordinary
//! square-and-multiply, branching on the exponent's bits directly, exactly
//! as the brief asks for.
//!
//! Storage: [`LIMBS`] little-endian `u64` limbs, sized for a 4096-bit value
//! (64 limbs) plus two spare limbs of margin so a single add/subtract never
//! has to worry about overflowing the array. A Montgomery multiply's
//! internal double-width product needs more room than that; it uses its own
//! fixed-size stack buffer ([`WIDE_LIMBS`]), never the heap -- consistent
//! with every other primitive in this crate (AES, SHA-2, ...) using
//! fixed-size arrays, not `Vec`, even though `alloc` is available (D27).
//!
//! [`Modulus`] precomputes everything Montgomery multiplication needs for a
//! given odd modulus once (`n0inv`, `R mod n`, `R^2 mod n`) and scopes its
//! inner loops to that modulus's own limb count, not the fixed 66-limb
//! capacity -- a 256-bit P-256 field operation does 4-limb arithmetic, not
//! 66-limb, even though the same `Modulus` type also serves 4096-bit RSA.

use core::cmp::Ordering;

/// Bits per limb.
const LIMB_BITS: usize = 64;
/// The largest modulus this module supports (brief: "4096-bit moduli").
pub const MAX_BITS: usize = 4096;
/// Limb capacity: 4096 bits (64 limbs) plus two spare limbs of margin, so a
/// single add/subtract of two in-range values never overflows the array.
pub const LIMBS: usize = MAX_BITS / LIMB_BITS + 2;
/// Scratch space for a Montgomery multiply's double-width product: two
/// operands of up to `LIMBS` limbs each, plus margin for REDC's carry-out.
const WIDE_LIMBS: usize = 2 * LIMBS + 2;

/// A big-integer operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BigIntError {
    /// `from_be_bytes`: more bytes than this type's capacity ([`MAX_BITS`] bits).
    TooLarge,
    /// `to_be_bytes`: the value does not fit in the caller's output buffer
    /// (RFC 8017's I2OSP "integer too large" failure).
    DoesNotFit,
    /// `Modulus::new`: zero, even (Montgomery reduction needs the modulus
    /// coprime to the limb radix, so it must be odd -- true of every RSA
    /// modulus and every prime field/group order this crate uses), or too
    /// large for this type's capacity.
    InvalidModulus,
}

/// A non-negative integer of up to [`MAX_BITS`] bits, as little-endian `u64`
/// limbs (limb 0 is least significant). Unused high limbs are always zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BigUint {
    limbs: [u64; LIMBS],
}

impl BigUint {
    /// The additive identity.
    pub const ZERO: BigUint = BigUint { limbs: [0u64; LIMBS] };
    /// The multiplicative identity.
    pub const ONE: BigUint = BigUint::from_u64(1);

    /// Builds a `BigUint` from a single `u64`.
    pub const fn from_u64(x: u64) -> BigUint {
        let mut limbs = [0u64; LIMBS];
        limbs[0] = x;
        BigUint { limbs }
    }

    /// Decodes a big-endian byte string (as RSA moduli, exponents and
    /// signatures, and SEC1 curve coordinates, are all encoded). Any number
    /// of leading zero bytes is accepted (they just contribute zero limbs);
    /// only the total length against this type's fixed capacity is checked
    /// -- "strict length checks" here means rejecting what does not fit, not
    /// requiring a minimal encoding on the way in (RFC 8017's I2OSP, used for
    /// the strict fixed-length direction, is [`BigUint::to_be_bytes`] below).
    pub fn from_be_bytes(bytes: &[u8]) -> Result<BigUint, BigIntError> {
        if bytes.len() > LIMBS * 8 {
            return Err(BigIntError::TooLarge);
        }
        let mut limbs = [0u64; LIMBS];
        for (i, chunk) in bytes.rchunks(8).enumerate() {
            let mut buf = [0u8; 8];
            buf[8 - chunk.len()..].copy_from_slice(chunk);
            limbs[i] = u64::from_be_bytes(buf);
        }
        Ok(BigUint { limbs })
    }

    /// Encodes as exactly `out.len()` big-endian bytes (RFC 8017's I2OSP):
    /// zero-padded on the left, or [`BigIntError::DoesNotFit`] if the value
    /// needs more bytes than `out` provides.
    pub fn to_be_bytes(&self, out: &mut [u8]) -> Result<(), BigIntError> {
        if self.bit_len() > out.len() * 8 {
            return Err(BigIntError::DoesNotFit);
        }
        for (i, byte) in out.iter_mut().rev().enumerate() {
            let limb_idx = i / 8;
            *byte = if limb_idx < LIMBS { (self.limbs[limb_idx] >> ((i % 8) * 8)) as u8 } else { 0 };
        }
        Ok(())
    }

    /// The number of bits needed to represent this value (0 for zero).
    pub fn bit_len(&self) -> usize {
        for i in (0..LIMBS).rev() {
            if self.limbs[i] != 0 {
                return i * LIMB_BITS + (LIMB_BITS - self.limbs[i].leading_zeros() as usize);
            }
        }
        0
    }

    /// Bit `i` (0 = least significant), `false` beyond this value's capacity.
    pub fn bit(&self, i: usize) -> bool {
        if i >= LIMBS * LIMB_BITS {
            return false;
        }
        (self.limbs[i / LIMB_BITS] >> (i % LIMB_BITS)) & 1 == 1
    }

    /// Whether this value is zero.
    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    /// `self - small`, assuming `self >= small` (used for tiny constant
    /// exponents like the Fermat-inverse exponent `p - 2`).
    pub fn sub_u64(&self, small: u64) -> BigUint {
        let mut r = *self;
        r.wrapping_sub_assign(&BigUint::from_u64(small));
        r
    }

    /// Right-shifts by `bits` (dividing by `2^bits`, discarding the low bits).
    pub fn shr(&self, bits: usize) -> BigUint {
        let limb_shift = bits / LIMB_BITS;
        let bit_shift = bits % LIMB_BITS;
        let mut out = BigUint::ZERO;
        for (i, out_limb) in out.limbs.iter_mut().enumerate() {
            let src = i + limb_shift;
            if src >= LIMBS {
                break;
            }
            let mut limb = self.limbs[src] >> bit_shift;
            if bit_shift > 0 && src + 1 < LIMBS {
                limb |= self.limbs[src + 1] << (LIMB_BITS - bit_shift);
            }
            *out_limb = limb;
        }
        out
    }

    /// `self += other`, wrapping silently past this type's capacity (callers
    /// keep operands within [`MAX_BITS`] plus the two-limb margin this type
    /// reserves precisely so a single add never needs to do this).
    fn wrapping_add_assign(&mut self, other: &BigUint) {
        let mut carry: u128 = 0;
        for i in 0..LIMBS {
            let sum = self.limbs[i] as u128 + other.limbs[i] as u128 + carry;
            self.limbs[i] = sum as u64;
            carry = sum >> 64;
        }
    }

    /// `self -= other`, assuming `self >= other` (unchecked: every call site
    /// below already established that with a `cmp`/`>=`, per this module's
    /// "public data, no need to hide the precondition behind a `Result`"
    /// style -- see the module doc).
    fn wrapping_sub_assign(&mut self, other: &BigUint) {
        let mut borrow: i128 = 0;
        for i in 0..LIMBS {
            let diff = self.limbs[i] as i128 - other.limbs[i] as i128 - borrow;
            if diff < 0 {
                self.limbs[i] = (diff + (1i128 << 64)) as u64;
                borrow = 1;
            } else {
                self.limbs[i] = diff as u64;
                borrow = 0;
            }
        }
    }
}

impl PartialOrd for BigUint {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Numeric order (most-significant limb first) -- *not* the little-endian
/// array's own lexicographic order, which would compare the least
/// significant limb first.
impl Ord for BigUint {
    fn cmp(&self, other: &Self) -> Ordering {
        for i in (0..LIMBS).rev() {
            let ord = self.limbs[i].cmp(&other.limbs[i]);
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    }
}

/// `out[offset..] += n * m` (`m` a single limb), propagating the carry past
/// `n`'s width for as long as `out` has room. The building block both
/// [`mul_wide`] (accumulating each partial product of a schoolbook multiply)
/// and [`Modulus::mont_mul`] (accumulating `m * n` at each REDC step) reduce to.
fn mac_add(out: &mut [u64], offset: usize, n: &[u64], m: u64) {
    let mut carry: u128 = 0;
    for (j, &nj) in n.iter().enumerate() {
        let idx = offset + j;
        let sum = out[idx] as u128 + (nj as u128) * (m as u128) + carry;
        out[idx] = sum as u64;
        carry = sum >> 64;
    }
    let mut idx = offset + n.len();
    while carry != 0 {
        let sum = out[idx] as u128 + carry;
        out[idx] = sum as u64;
        carry = sum >> 64;
        idx += 1;
    }
}

/// Schoolbook multiplication: `out = a * b` (`out` must be exactly
/// `a.len() + b.len()` limbs -- the exact width any product of those sizes
/// needs, never more).
fn mul_wide(a: &[u64], b: &[u64], out: &mut [u64]) {
    out.fill(0);
    for (i, &ai) in a.iter().enumerate() {
        mac_add(out, i, b, ai);
    }
}

/// `-n0^-1 mod 2^64` (`n0` the modulus's least significant limb, which must
/// be odd), Montgomery's per-modulus reduction constant. Computed by Newton's
/// iteration for the multiplicative inverse mod a power of two: `x` is
/// already correct mod 8 (`n0 * n0 = 1 mod 8` for any odd `n0`), and each
/// iteration `x <- x*(2 - n0*x)` doubles the number of correct low bits, so
/// five iterations (3 -> 6 -> 12 -> 24 -> 48 -> 96) comfortably clear 64.
fn mont_n0inv(n0: u64) -> u64 {
    let mut x = n0;
    for _ in 0..5 {
        x = x.wrapping_mul(2u64.wrapping_sub(n0.wrapping_mul(x)));
    }
    x.wrapping_neg()
}

/// `2^k mod n` via `k` conditional-doubling steps from 1. Used only to set up
/// a [`Modulus`] (computing `R mod n` and `R^2 mod n`), never in a hot loop,
/// so simplicity wins over the faster divide-based reductions a general
/// bignum library would use here.
fn pow2_mod(k: usize, n: &BigUint) -> BigUint {
    if *n == BigUint::ONE {
        return BigUint::ZERO;
    }
    let mut v = BigUint::ONE;
    for _ in 0..k {
        let doubled = v;
        v.wrapping_add_assign(&doubled);
        if v >= *n {
            v.wrapping_sub_assign(n);
        }
    }
    v
}

/// An odd modulus with its Montgomery reduction constants precomputed once,
/// used for both RSA's `s^e mod n` and elliptic-curve field/scalar
/// arithmetic (`mod p`, `mod n`) -- see the module doc. `Copy` (every field
/// is): [`crate::ecc`] builds one per curve operation without needing to
/// thread lifetimes through its point-arithmetic functions.
#[derive(Clone, Copy)]
pub struct Modulus {
    n: BigUint,
    limbs: usize,
    n0inv: u64,
    r_mod_n: BigUint,
    r2_mod_n: BigUint,
}

impl Modulus {
    /// Precomputes a Montgomery context for `n`. `n` must be odd, nonzero and
    /// within [`MAX_BITS`] bits -- true of every RSA modulus and every NIST
    /// prime-field/group-order modulus this crate uses.
    pub fn new(n: BigUint) -> Result<Modulus, BigIntError> {
        if n.is_zero() || !n.bit(0) || n.bit_len() > MAX_BITS {
            return Err(BigIntError::InvalidModulus);
        }
        let limbs = n.bit_len().div_ceil(LIMB_BITS).max(1);
        let n0inv = mont_n0inv(n.limbs[0]);
        let bits = limbs * LIMB_BITS;
        let r_mod_n = pow2_mod(bits, &n);
        let r2_mod_n = pow2_mod(2 * bits, &n);
        Ok(Modulus { n, limbs, n0inv, r_mod_n, r2_mod_n })
    }

    /// The modulus itself.
    pub fn value(&self) -> &BigUint {
        &self.n
    }

    /// `R mod n` where `R = 2^(64 * limb count)` -- the Montgomery form of 1.
    /// Exposed only so the differential host test can cross-check it
    /// directly against Python's `pow(2, 64*limbs, n)`.
    pub fn r_mod_n(&self) -> BigUint {
        self.r_mod_n
    }

    /// `R^2 mod n`, used to move a value into Montgomery form. Exposed for
    /// the same cross-check reason as [`Modulus::r_mod_n`].
    pub fn r2_mod_n(&self) -> BigUint {
        self.r2_mod_n
    }

    /// `a * b * R^-1 mod n` (Montgomery multiplication, "separated operand
    /// scanning": a full double-width product, then one REDC pass). Valid
    /// whenever `a * b < n * R`, which holds both for `a, b < n` (the normal
    /// case) and for one operand merely `< R` (e.g. ECDSA's hash-derived `z`,
    /// truncated to the group order's bit length but not necessarily reduced
    /// below it) -- see the module doc's constant-time note for why no
    /// special-casing of that is needed. Crate-internal: [`rsa`](crate::rsa)
    /// and [`ecc`](crate::ecc) both keep values in Montgomery form across a
    /// whole computation and only call [`Modulus::montgomery_encode`]/
    /// [`Modulus::montgomery_decode`] at the boundaries, rather than exposing
    /// this lower-level operation outside the crate.
    pub(crate) fn mont_mul(&self, a: &BigUint, b: &BigUint) -> BigUint {
        let k = self.limbs;
        let mut t = [0u64; WIDE_LIMBS];
        mul_wide(&a.limbs[..k], &b.limbs[..k], &mut t[..2 * k]);
        for i in 0..k {
            let m = t[i].wrapping_mul(self.n0inv);
            mac_add(&mut t, i, &self.n.limbs[..k], m);
        }
        let mut result = BigUint::ZERO;
        result.limbs[..=k].copy_from_slice(&t[k..=2 * k]);
        if result >= self.n {
            result.wrapping_sub_assign(&self.n);
        }
        result
    }

    /// Moves `a` into Montgomery form (`a * R mod n`).
    pub(crate) fn montgomery_encode(&self, a: &BigUint) -> BigUint {
        self.mont_mul(a, &self.r2_mod_n)
    }

    /// Moves a Montgomery-form value back to normal form (`aR * R^-1 mod n`).
    pub(crate) fn montgomery_decode(&self, a: &BigUint) -> BigUint {
        self.mont_mul(a, &BigUint::ONE)
    }

    /// `(a * b) mod n`, entirely in normal (non-Montgomery) form -- the
    /// black-box operation the differential host test checks against
    /// Python's `(a * b) % n`. [`rsa`](crate::rsa)/[`ecc`](crate::ecc) use
    /// [`Modulus::mont_mul`] directly instead, to stay in Montgomery form
    /// across a whole RSA exponentiation or EC point operation rather than
    /// paying this function's two extra conversions on every single multiply.
    pub fn mul_mod(&self, a: &BigUint, b: &BigUint) -> BigUint {
        let am = self.montgomery_encode(a);
        let bm = self.montgomery_encode(b);
        self.montgomery_decode(&self.mont_mul(&am, &bm))
    }

    /// `base^exponent mod n` by ordinary (branch-on-the-exponent-bit)
    /// square-and-multiply, most-significant bit first -- see the module doc
    /// for why this need not be constant-time here. `exponent = 0` correctly
    /// yields 1 (the loop over its zero bits never runs).
    pub fn pow_mod(&self, base: &BigUint, exponent: &BigUint) -> BigUint {
        let base_mont = self.montgomery_encode(base);
        let mut result = self.r_mod_n; // Montgomery form of 1.
        for i in (0..exponent.bit_len()).rev() {
            result = self.mont_mul(&result, &result);
            if exponent.bit(i) {
                result = self.mont_mul(&result, &base_mont);
            }
        }
        self.montgomery_decode(&result)
    }
}

/// `(a + b) mod m`. For elliptic-curve field/coordinate arithmetic, where
/// values are kept in Montgomery form across a whole point operation:
/// addition and subtraction are linear, so they commute with the Montgomery
/// scaling and need no conversion (`(a*R mod m) + (b*R mod m) mod m =
/// (a+b)*R mod m`).
pub fn add_mod(a: &BigUint, b: &BigUint, m: &BigUint) -> BigUint {
    let mut r = *a;
    r.wrapping_add_assign(b);
    if r >= *m {
        r.wrapping_sub_assign(m);
    }
    r
}

/// `(a - b) mod m`.
pub fn sub_mod(a: &BigUint, b: &BigUint, m: &BigUint) -> BigUint {
    let mut r = *a;
    if a >= b {
        r.wrapping_sub_assign(b);
    } else {
        r.wrapping_add_assign(m);
        r.wrapping_sub_assign(b);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn be_bytes_round_trip() {
        let bytes = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x01];
        let v = BigUint::from_be_bytes(&bytes).unwrap();
        let mut out = [0u8; 9];
        v.to_be_bytes(&mut out).unwrap();
        assert_eq!(out, bytes);
    }

    #[test]
    fn be_bytes_tolerates_leading_zeros() {
        let a = BigUint::from_be_bytes(&[0x00, 0x00, 0x2a]).unwrap();
        let b = BigUint::from_u64(42);
        assert_eq!(a, b);
    }

    #[test]
    fn from_be_bytes_rejects_too_large() {
        let huge = [0xffu8; LIMBS * 8 + 1];
        assert_eq!(BigUint::from_be_bytes(&huge), Err(BigIntError::TooLarge));
    }

    #[test]
    fn to_be_bytes_rejects_does_not_fit() {
        let v = BigUint::from_u64(256);
        let mut out = [0u8; 1];
        assert_eq!(v.to_be_bytes(&mut out), Err(BigIntError::DoesNotFit));
    }

    #[test]
    fn bit_len_and_bit() {
        assert_eq!(BigUint::ZERO.bit_len(), 0);
        assert_eq!(BigUint::ONE.bit_len(), 1);
        let v = BigUint::from_u64(0b1011);
        assert_eq!(v.bit_len(), 4);
        assert!(v.bit(0));
        assert!(v.bit(1));
        assert!(!v.bit(2));
        assert!(v.bit(3));
        assert!(!v.bit(4));
    }

    #[test]
    fn ordering_is_numeric_not_lexicographic_on_limbs() {
        let small = BigUint::from_u64(1);
        let mut big = BigUint::ZERO;
        big.limbs[1] = 1; // 2^64, numerically far bigger than 1.
        assert!(big > small);
        assert!(small < big);
    }

    #[test]
    fn mont_n0inv_is_correct() {
        for &n0 in &[1u64, 3, 0xffff_ffff_ffff_ffffu64, 0xdead_beef_0000_0001u64 | 1] {
            let inv = mont_n0inv(n0);
            // n0 * n0inv == -1 mod 2^64, by definition of n0inv.
            assert_eq!(n0.wrapping_mul(inv), 0u64.wrapping_sub(1));
        }
    }

    #[test]
    fn mul_mod_small_prime_field() {
        let m = Modulus::new(BigUint::from_u64(97)).unwrap();
        let a = BigUint::from_u64(45);
        let b = BigUint::from_u64(60);
        assert_eq!(m.mul_mod(&a, &b), BigUint::from_u64((45 * 60) % 97));
    }

    #[test]
    fn r_mod_n_represents_one_in_montgomery_form() {
        // r_mod_n = R mod n is, by definition, "1" in Montgomery form: taking
        // it back out of Montgomery form must give the value 1.
        let m = Modulus::new(BigUint::from_u64(97)).unwrap();
        assert_eq!(m.montgomery_decode(&m.r_mod_n()), BigUint::ONE);
        // Likewise r2_mod_n = R^2 mod n is "R mod n" in Montgomery form.
        assert_eq!(m.montgomery_decode(&m.r2_mod_n()), m.r_mod_n());
    }

    #[test]
    fn fermat_little_theorem_small_prime() {
        // a^(p-1) = 1 mod p for p prime, gcd(a, p) = 1.
        let p = 97u64;
        let m = Modulus::new(BigUint::from_u64(p)).unwrap();
        for a in [2u64, 5, 10, 96] {
            let result = m.pow_mod(&BigUint::from_u64(a), &BigUint::from_u64(p - 1));
            assert_eq!(result, BigUint::ONE, "{a}^{}, mod {p}", p - 1);
        }
    }

    #[test]
    fn textbook_rsa_example() {
        // The classic tiny RSA example: p=61, q=53, n=3233, e=17, d=2753.
        let n = Modulus::new(BigUint::from_u64(3233)).unwrap();
        let ciphertext = n.pow_mod(&BigUint::from_u64(65), &BigUint::from_u64(17));
        assert_eq!(ciphertext, BigUint::from_u64(2790));
        let plaintext = n.pow_mod(&ciphertext, &BigUint::from_u64(2753));
        assert_eq!(plaintext, BigUint::from_u64(65));
    }

    #[test]
    fn pow_mod_exponent_zero_is_one() {
        let m = Modulus::new(BigUint::from_u64(97)).unwrap();
        let result = m.pow_mod(&BigUint::from_u64(42), &BigUint::ZERO);
        assert_eq!(result, BigUint::ONE);
    }

    #[test]
    fn sub_u64_and_shr() {
        let v = BigUint::from_u64(1000);
        assert_eq!(v.sub_u64(3), BigUint::from_u64(997));
        assert_eq!(v.shr(3), BigUint::from_u64(125)); // 1000 / 8 = 125
        assert_eq!(v.shr(0), v);

        // A shift that crosses a limb boundary.
        let mut big = BigUint::ZERO;
        big.limbs[1] = 1; // value = 2^64
        assert_eq!(big.shr(64), BigUint::ONE);
        assert_eq!(big.shr(65), BigUint::ZERO);
    }

    #[test]
    fn add_mod_and_sub_mod_are_inverses() {
        let m = BigUint::from_u64(97);
        let a = BigUint::from_u64(50);
        let b = BigUint::from_u64(80);
        let sum = add_mod(&a, &b, &m);
        assert_eq!(sum, BigUint::from_u64((50 + 80) % 97));
        assert_eq!(sub_mod(&sum, &b, &m), a);
        assert_eq!(sub_mod(&a, &b, &m), BigUint::from_u64(50 + 97 - 80));
    }

    #[test]
    fn pow_mod_matches_manual_square_and_multiply_for_a_64_bit_modulus() {
        // A modulus/base near the top of `u64`'s range (so `u128`
        // intermediate products in the from-scratch reference below never
        // overflow), exercised with a bigger (30-bit) exponent than the tiny
        // textbook examples above, cross-checked against a completely
        // independent, deliberately naive `u128`-accumulator implementation.
        let n_val: u64 = 0xC0FF_EE00_DEAD_BEEF | 1; // odd.
        let n = Modulus::new(BigUint::from_be_bytes(&n_val.to_be_bytes()).unwrap()).unwrap();
        let base: u64 = 123_456_789;
        let exp: u64 = 987_654_321;

        fn naive_pow_mod(mut base: u128, mut exp: u64, m: u128) -> u128 {
            let mut result: u128 = 1 % m;
            base %= m;
            while exp > 0 {
                if exp & 1 == 1 {
                    result = (result * base) % m;
                }
                base = (base * base) % m;
                exp >>= 1;
            }
            result
        }
        let expected = naive_pow_mod(base as u128, exp, n_val as u128) as u64;
        let got = n.pow_mod(&BigUint::from_u64(base), &BigUint::from_u64(exp));
        assert_eq!(got, BigUint::from_u64(expected));
    }
}
