//! Shared elliptic-curve point arithmetic and ECDSA verification for
//! [`crate::p256`] and [`crate::p384`]: short Weierstrass curves
//! `y^2 = x^3 - 3x + b` over a prime field, points in projective coordinates
//! using the complete addition formulas of Renes, Costello and Batina
//! ("Complete addition formulas for prime order elliptic curves", 2016,
//! Algorithm 4 for curves with `a = -3` -- both NIST P-256 and P-384 have
//! `a = -3`). "Complete" means the *same* formula correctly handles point
//! doubling, adding two distinct points, and either input being the point at
//! infinity, with no case analysis to get wrong -- the brief's own reason for
//! choosing them, and exactly what Project Wycheproof's `PointDuplication`
//! test cases probe for.
//!
//! As in [`crate::rsa`]: this is verification only, every value (curve point,
//! signature, digest) is public, so none of this needs to be constant-time
//! (see [`crate::bigint`]'s module doc). [`scalar_mul_shamir`] computes
//! `u1*G + u2*Q` via Shamir's trick (one combined double-and-add pass instead
//! of two separate scalar multiplications), exactly as the brief asks.
//!
//! Coordinates are kept in Montgomery form (see [`crate::bigint::Modulus`])
//! across a whole point operation, converting to/from normal form only at
//! the two boundaries: decoding a SEC1 public key in, and reading out the
//! final affine x-coordinate to compare against `r`.

use crate::bigint::{BigUint, Modulus, add_mod, sub_mod};

/// A point in projective coordinates `(X : Y : Z)`, all three coordinates in
/// Montgomery form modulo the curve's field prime. The point at infinity is
/// `(0 : 1 : 0)` (Montgomery form of 1, i.e. [`Modulus::r_mod_n`] of the field).
#[derive(Clone, Copy)]
pub(crate) struct Point {
    x: BigUint,
    y: BigUint,
    z: BigUint,
}

/// A short Weierstrass curve with `a = -3`: its field, its group order, its
/// `b` coefficient and base point (both already in Montgomery form), and its
/// coordinate size in bytes (32 for P-256, 48 for P-384). Built fresh by
/// [`crate::p256`]/[`crate::p384`] for each `PublicKey` (cheap: a handful of
/// microseconds of Montgomery setup, no caching machinery needed -- see
/// their module docs).
pub(crate) struct CurveParams {
    pub(crate) byte_len: usize,
    pub(crate) field: Modulus,
    pub(crate) order: Modulus,
    b_mont: BigUint,
    generator: Point,
}

impl CurveParams {
    /// Builds the Montgomery context and constants for a curve from its
    /// standard (SEC 2 / FIPS 186-4) big-endian parameters. Panics only if
    /// `p256`/`p384`'s own hardcoded constant tables are wrong (an odd prime
    /// and order, values that fit `byte_len` bytes) -- a bug the Wycheproof
    /// test suites would catch immediately as "every valid signature fails".
    pub(crate) fn new(p: &[u8], n: &[u8], b: &[u8], gx: &[u8], gy: &[u8], byte_len: usize) -> CurveParams {
        let field = Modulus::new(BigUint::from_be_bytes(p).expect("curve prime constant")).expect("curve prime is odd");
        let order = Modulus::new(BigUint::from_be_bytes(n).expect("curve order constant")).expect("curve order is odd");
        let b_mont = field.montgomery_encode(&BigUint::from_be_bytes(b).expect("curve b constant"));
        let gx_mont = field.montgomery_encode(&BigUint::from_be_bytes(gx).expect("curve gx constant"));
        let gy_mont = field.montgomery_encode(&BigUint::from_be_bytes(gy).expect("curve gy constant"));
        let generator = Point { x: gx_mont, y: gy_mont, z: field.r_mod_n() };
        CurveParams { byte_len, field, order, b_mont, generator }
    }

    fn identity(&self) -> Point {
        Point { x: BigUint::ZERO, y: self.field.r_mod_n(), z: BigUint::ZERO }
    }
}

/// A verification failed: a malformed public key or signature encoding, or a
/// signature that does not verify. One flat error type, since a TLS client
/// treats every one of these identically (abort the handshake).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcdsaError {
    /// The SEC1 encoding was not the uncompressed form of a valid, on-curve,
    /// non-infinity point for this curve.
    InvalidPublicKey,
    /// The DER `ECDSA-Sig-Value` was not strict minimal DER (non-minimal
    /// length or INTEGER encoding, a negative or zero `r`/`s`, or trailing
    /// bytes), or `r`/`s` was not in `[1, n-1]`.
    InvalidSignatureEncoding,
    /// The signature encoded and range-checked fine but does not verify.
    InvalidSignature,
}

/// Renes-Costello-Batina Algorithm 4 (`a = -3`, their paper section 3.2,
/// 12M + 2 multiplications by `b` + 29 additions): unified point addition
/// that is also correct when `p1 == p2` (doubling) or either input is the
/// point at infinity -- see the module doc. Every `t0..t4`/`x3`/`y3`/`z3`
/// step below is exactly the paper's Algorithm 4 in order (each `M` a
/// [`Modulus::mont_mul`], each add/sub a linear [`add_mod`]/[`sub_mod`] mod
/// the curve's field prime), transcribed and independently cross-checked
/// (host-side, during development) against the textbook chord-and-tangent
/// formula over many random point pairs on a small curve -- see the brief's
/// report.
fn point_add(curve: &CurveParams, p1: &Point, p2: &Point) -> Point {
    let fp = &curve.field;
    let p = fp.value();
    let b = &curve.b_mont;
    let (x1, y1, z1) = (p1.x, p1.y, p1.z);
    let (x2, y2, z2) = (p2.x, p2.y, p2.z);

    let t0 = fp.mont_mul(&x1, &x2); // 1
    let t1 = fp.mont_mul(&y1, &y2); // 2
    let t2 = fp.mont_mul(&z1, &z2); // 3
    let t3 = add_mod(&x1, &y1, p); // 4
    let t4 = add_mod(&x2, &y2, p); // 5
    let t3 = fp.mont_mul(&t3, &t4); // 6
    let t4 = add_mod(&t0, &t1, p); // 7
    let t3 = sub_mod(&t3, &t4, p); // 8
    let t4 = add_mod(&y1, &z1, p); // 9
    let x3 = add_mod(&y2, &z2, p); // 10
    let t4 = fp.mont_mul(&t4, &x3); // 11
    let x3 = add_mod(&t1, &t2, p); // 12
    let t4 = sub_mod(&t4, &x3, p); // 13
    let x3 = add_mod(&x1, &z1, p); // 14
    let y3 = add_mod(&x2, &z2, p); // 15
    let x3 = fp.mont_mul(&x3, &y3); // 16
    let y3 = add_mod(&t0, &t2, p); // 17
    let y3 = sub_mod(&x3, &y3, p); // 18
    let z3 = fp.mont_mul(b, &t2); // 19
    let x3 = sub_mod(&y3, &z3, p); // 20
    let z3 = add_mod(&x3, &x3, p); // 21
    let x3 = add_mod(&x3, &z3, p); // 22
    let z3 = sub_mod(&t1, &x3, p); // 23
    let x3 = add_mod(&t1, &x3, p); // 24
    let y3 = fp.mont_mul(b, &y3); // 25
    let t1 = add_mod(&t2, &t2, p); // 26
    let t2 = add_mod(&t1, &t2, p); // 27
    let y3 = sub_mod(&y3, &t2, p); // 28
    let y3 = sub_mod(&y3, &t0, p); // 29
    let t1 = add_mod(&y3, &y3, p); // 30
    let y3 = add_mod(&t1, &y3, p); // 31
    let t1 = add_mod(&t0, &t0, p); // 32
    let t0 = add_mod(&t1, &t0, p); // 33
    let t0 = sub_mod(&t0, &t2, p); // 34
    let t1 = fp.mont_mul(&t4, &y3); // 35
    let t2 = fp.mont_mul(&t0, &y3); // 36
    let y3 = fp.mont_mul(&x3, &z3); // 37
    let y3 = add_mod(&y3, &t2, p); // 38
    let x3 = fp.mont_mul(&t3, &x3); // 39
    let x3 = sub_mod(&x3, &t1, p); // 40
    let z3 = fp.mont_mul(&t4, &z3); // 41
    let t1 = fp.mont_mul(&t3, &t0); // 42
    let z3 = add_mod(&z3, &t1, p); // 43

    Point { x: x3, y: y3, z: z3 }
}

/// `u1*G + u2*Q` (Shamir's trick): one double-and-add pass over the bits of
/// `max(u1, u2)`, doubling the accumulator once per bit and adding whichever
/// of `{nothing, G, Q, G+Q}` that bit position of `(u1, u2)` selects, instead
/// of two independent scalar multiplications. Not constant-time (branches on
/// the bits of `u1`/`u2`) -- both are public per-verification values derived
/// from the public `r`, `s` and digest, never a secret.
fn scalar_mul_shamir(curve: &CurveParams, q: &Point, u1: &BigUint, u2: &BigUint) -> Point {
    let g_plus_q = point_add(curve, &curve.generator, q);
    let bits = core::cmp::max(u1.bit_len(), u2.bit_len());
    let mut acc = curve.identity();
    for i in (0..bits).rev() {
        acc = point_add(curve, &acc, &acc);
        acc = match (u1.bit(i), u2.bit(i)) {
            (false, false) => acc,
            (true, false) => point_add(curve, &acc, &curve.generator),
            (false, true) => point_add(curve, &acc, q),
            (true, true) => point_add(curve, &acc, &g_plus_q),
        };
    }
    acc
}

/// `a^-1 mod m.value()` by Fermat's little theorem (`m.value()` prime), for
/// `a` and the result both in *normal* (non-Montgomery) form -- used both for
/// a field-element inverse (converting the final point back to affine) and
/// for `s^-1 mod n` in ECDSA verification itself.
fn mod_inverse(m: &Modulus, a: &BigUint) -> BigUint {
    m.pow_mod(a, &m.value().sub_u64(2))
}

/// The affine x-coordinate of a projective point, or `None` if it is the
/// point at infinity (`Z == 0`) -- FIPS 186-5 requires rejecting a computed
/// `R = O` during verification.
fn affine_x(fp: &Modulus, point: &Point) -> Option<BigUint> {
    if point.z.is_zero() {
        return None;
    }
    let z_inv_normal = mod_inverse(fp, &fp.montgomery_decode(&point.z));
    let z_inv_mont = fp.montgomery_encode(&z_inv_normal);
    Some(fp.montgomery_decode(&fp.mont_mul(&point.x, &z_inv_mont)))
}

/// `y^2 == x^3 - 3x + b (mod p)`, all in Montgomery form.
fn is_on_curve(curve: &CurveParams, x_mont: &BigUint, y_mont: &BigUint) -> bool {
    let fp = &curve.field;
    let p = fp.value();
    let lhs = fp.mont_mul(y_mont, y_mont);
    let x2 = fp.mont_mul(x_mont, x_mont);
    let x3 = fp.mont_mul(&x2, x_mont);
    let three_x = add_mod(&add_mod(x_mont, x_mont, p), x_mont, p);
    let rhs = add_mod(&sub_mod(&x3, &three_x, p), &curve.b_mont, p);
    lhs == rhs
}

/// Decodes a SEC1 uncompressed point (`0x04 || x || y`), checks it is on the
/// curve, and rejects the point at infinity (SEC1's single-byte `0x00`
/// encoding, or any other malformed input).
pub(crate) fn point_from_sec1(curve: &CurveParams, bytes: &[u8]) -> Result<Point, EcdsaError> {
    if bytes.len() != 1 + 2 * curve.byte_len || bytes[0] != 0x04 {
        return Err(EcdsaError::InvalidPublicKey);
    }
    let x = BigUint::from_be_bytes(&bytes[1..1 + curve.byte_len]).map_err(|_| EcdsaError::InvalidPublicKey)?;
    let y = BigUint::from_be_bytes(&bytes[1 + curve.byte_len..]).map_err(|_| EcdsaError::InvalidPublicKey)?;
    if x >= *curve.field.value() || y >= *curve.field.value() {
        return Err(EcdsaError::InvalidPublicKey);
    }
    let x_mont = curve.field.montgomery_encode(&x);
    let y_mont = curve.field.montgomery_encode(&y);
    if !is_on_curve(curve, &x_mont, &y_mont) {
        return Err(EcdsaError::InvalidPublicKey);
    }
    Ok(Point { x: x_mont, y: y_mont, z: curve.field.r_mod_n() })
}

/// Parses one DER `INTEGER` TLV from the front of `buf`: rejects long-form
/// lengths (never needed for the <= 49-byte integers ECDSA signatures on
/// these curves carry -- seeing one at all means non-minimal/BER, not DER),
/// a non-minimal leading `0x00`, and a negative value (this module's
/// integers, `r` and `s`, are always positive). Returns the value bytes and
/// whatever of `buf` follows the TLV.
fn parse_der_integer(buf: &[u8]) -> Option<(&[u8], &[u8])> {
    if buf.len() < 3 || buf[0] != 0x02 {
        return None;
    }
    let len = buf[1];
    if len & 0x80 != 0 || len == 0 {
        return None;
    }
    let len = len as usize;
    if buf.len() < 2 + len {
        return None;
    }
    let value = &buf[2..2 + len];
    if value[0] & 0x80 != 0 {
        return None; // Negative per DER's two's-complement INTEGER convention.
    }
    if len > 1 && value[0] == 0x00 && value[1] & 0x80 == 0 {
        return None; // Non-minimal: the leading zero was not needed.
    }
    Some((value, &buf[2 + len..]))
}

/// Strictly parses `SEQUENCE { INTEGER r, INTEGER s }` (RFC 3279's
/// `ECDSA-Sig-Value`): no trailing bytes after the sequence or after `s`
/// inside it, short-form (minimal) length encoding throughout.
fn parse_der_signature(sig: &[u8]) -> Result<(BigUint, BigUint), EcdsaError> {
    if sig.len() < 8 || sig[0] != 0x30 {
        return Err(EcdsaError::InvalidSignatureEncoding);
    }
    let seq_len = sig[1];
    if seq_len & 0x80 != 0 {
        return Err(EcdsaError::InvalidSignatureEncoding);
    }
    if sig.len() != 2 + seq_len as usize {
        return Err(EcdsaError::InvalidSignatureEncoding);
    }
    let body = &sig[2..];
    let (r_bytes, rest) = parse_der_integer(body).ok_or(EcdsaError::InvalidSignatureEncoding)?;
    let (s_bytes, rest) = parse_der_integer(rest).ok_or(EcdsaError::InvalidSignatureEncoding)?;
    if !rest.is_empty() {
        return Err(EcdsaError::InvalidSignatureEncoding);
    }
    let r = BigUint::from_be_bytes(r_bytes).map_err(|_| EcdsaError::InvalidSignatureEncoding)?;
    let s = BigUint::from_be_bytes(s_bytes).map_err(|_| EcdsaError::InvalidSignatureEncoding)?;
    Ok((r, s))
}

/// FIPS 186-5 section 6.4.2's truncation: the leftmost `min(8*digest.len(),
/// n_bits)` bits of the digest, as an integer (*not* further reduced mod the
/// order -- [`Modulus::mont_mul`]'s bound only needs the value `< R`, which
/// this always is; see [`crate::bigint::Modulus::mont_mul`]'s doc).
fn truncate_hash(digest: &[u8], n_bits: usize) -> BigUint {
    let digest_bits = digest.len() * 8;
    if digest_bits <= n_bits {
        return BigUint::from_be_bytes(digest).unwrap_or(BigUint::ZERO);
    }
    let take_bytes = n_bits.div_ceil(8);
    let excess_bits = take_bytes * 8 - n_bits;
    let v = BigUint::from_be_bytes(&digest[..take_bytes]).unwrap_or(BigUint::ZERO);
    v.shr(excess_bits)
}

/// ECDSA signature verification (FIPS 186-5 section 6.4.2): `digest` is the
/// caller's already-computed hash of the signed data (its algorithm is a
/// cipher-suite/certificate detail this function does not need to know --
/// only its length, via [`truncate_hash`]); `der_sig` is the DER
/// `ECDSA-Sig-Value`.
pub(crate) fn ecdsa_verify(curve: &CurveParams, public_key: &Point, digest: &[u8], der_sig: &[u8]) -> Result<(), EcdsaError> {
    let (r, s) = parse_der_signature(der_sig)?;
    let n = curve.order.value();
    if r.is_zero() || r >= *n || s.is_zero() || s >= *n {
        return Err(EcdsaError::InvalidSignatureEncoding);
    }

    let z = truncate_hash(digest, n.bit_len());
    let w = mod_inverse(&curve.order, &s);
    let u1 = curve.order.mul_mod(&z, &w);
    let u2 = curve.order.mul_mod(&r, &w);

    let result_point = scalar_mul_shamir(curve, public_key, &u1, &u2);
    let x_affine = affine_x(&curve.field, &result_point).ok_or(EcdsaError::InvalidSignature)?;

    let mut v = x_affine;
    while v >= *n {
        v = sub_mod(&v, n, n);
    }
    if v == r { Ok(()) } else { Err(EcdsaError::InvalidSignature) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny toy curve to exercise the group-law and Shamir's-trick code
    // paths independent of the real (and much slower to hand-verify) NIST
    // curves: y^2 = x^3 - 3x + 3 over F_37, a small prime field, base point
    // (1, 1) (on-curve: 1^2 = 1 = 1^3 - 3*1 + 3 mod 37) -- the same point
    // every test below also builds by hand via `to_point`, so
    // `scalar_mul_shamir`'s internal use of `curve.generator` (via
    // `CurveParams`) agrees with what the tests compute independently.
    // `31` (odd, otherwise unchecked here) stands in for the group order,
    // which these particular tests never exercise.
    fn toy_curve() -> CurveParams {
        CurveParams::new(&[37], &[31], &[3], &[1], &[1], 1)
    }

    fn toy_point_add_affine(p: (u64, u64), q: (u64, u64)) -> Option<(u64, u64)> {
        // Independent, direct-from-the-textbook-formula affine reference
        // (mod 37), to cross-check `point_add`'s projective result against.
        let m = 37i64;
        let inv = |a: i64| -> i64 {
            let a = a.rem_euclid(m);
            for cand in 1..m {
                if (a * cand).rem_euclid(m) == 1 {
                    return cand;
                }
            }
            unreachable!()
        };
        let (x1, y1) = (p.0 as i64, p.1 as i64);
        let (x2, y2) = (q.0 as i64, q.1 as i64);
        if x1 == x2 && (y1 + y2).rem_euclid(m) == 0 {
            return None; // P + (-P) = infinity.
        }
        let lambda = if p == q {
            (3 * x1 * x1 - 3) * inv(2 * y1)
        } else {
            (y2 - y1) * inv(x2 - x1)
        };
        let x3 = (lambda * lambda - x1 - x2).rem_euclid(m);
        let y3 = (lambda * (x1 - x3) - y1).rem_euclid(m);
        Some((x3 as u64, y3 as u64))
    }

    fn to_point(curve: &CurveParams, affine: (u64, u64)) -> Point {
        Point {
            x: curve.field.montgomery_encode(&BigUint::from_u64(affine.0)),
            y: curve.field.montgomery_encode(&BigUint::from_u64(affine.1)),
            z: curve.field.r_mod_n(),
        }
    }

    #[test]
    fn point_add_matches_textbook_affine_formula() {
        let curve = toy_curve();
        let g = (1u64, 1u64); // On-curve: 1^2 = 1 = 1^3 - 3*1 + 3 (mod 37).
        assert!(is_on_curve(&curve, &curve.field.montgomery_encode(&BigUint::from_u64(g.0)), &curve.field.montgomery_encode(&BigUint::from_u64(g.1))));

        let g2_expected = toy_point_add_affine(g, g).unwrap();
        let g2 = point_add(&curve, &to_point(&curve, g), &to_point(&curve, g));
        let g2_x = affine_x(&curve.field, &g2).unwrap();
        assert_eq!(g2_x, BigUint::from_u64(g2_expected.0));

        let g3_expected = toy_point_add_affine(g2_expected, g).unwrap();
        let g3 = point_add(&curve, &g2, &to_point(&curve, g));
        let g3_x = affine_x(&curve.field, &g3).unwrap();
        assert_eq!(g3_x, BigUint::from_u64(g3_expected.0));
    }

    #[test]
    fn point_add_identity_is_neutral() {
        let curve = toy_curve();
        let g = to_point(&curve, (1, 1));
        let sum = point_add(&curve, &g, &curve.identity());
        assert_eq!(affine_x(&curve.field, &sum), affine_x(&curve.field, &g));
    }

    #[test]
    fn point_doubling_via_add_matches_repeated_addition() {
        let curve = toy_curve();
        let g = to_point(&curve, (1, 1));
        let doubled = point_add(&curve, &g, &g);
        let via_scalar = scalar_mul_shamir(&curve, &curve.identity(), &BigUint::from_u64(2), &BigUint::ZERO);
        assert_eq!(affine_x(&curve.field, &doubled), affine_x(&curve.field, &via_scalar));
    }

    #[test]
    fn shamir_trick_matches_separate_scalar_multiplications() {
        let curve = toy_curve();
        let g = to_point(&curve, (1, 1));
        let q = point_add(&curve, &g, &g); // Q = 2G, a second point to combine.

        let u1 = BigUint::from_u64(5);
        let u2 = BigUint::from_u64(7);
        let combined = scalar_mul_shamir(&curve, &q, &u1, &u2);

        // Reference: u1*G computed by repeated addition, then + u2*Q likewise.
        let mut u1_g = curve.identity();
        for _ in 0..5 {
            u1_g = point_add(&curve, &u1_g, &g);
        }
        let mut u2_q = curve.identity();
        for _ in 0..7 {
            u2_q = point_add(&curve, &u2_q, &q);
        }
        let expected = point_add(&curve, &u1_g, &u2_q);
        assert_eq!(affine_x(&curve.field, &combined), affine_x(&curve.field, &expected));
    }

    #[test]
    fn parse_der_integer_rejects_non_minimal_and_negative() {
        assert!(parse_der_integer(&[0x02, 0x02, 0x00, 0x01]).is_none()); // unneeded leading zero.
        assert!(parse_der_integer(&[0x02, 0x01, 0x80]).is_none()); // negative (top bit set, no zero pad).
        assert!(parse_der_integer(&[0x02, 0x00]).is_none()); // empty INTEGER.
        assert!(parse_der_integer(&[0x02, 0x02, 0x00, 0x80]).is_some()); // correctly zero-padded.
        assert!(parse_der_integer(&[0x02, 0x01, 0x05]).is_some());
    }

    #[test]
    fn parse_der_signature_rejects_trailing_bytes() {
        let mut sig = vec![0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x02];
        assert!(parse_der_signature(&sig).is_ok());
        sig.push(0xff); // trailing garbage after the sequence.
        assert_eq!(parse_der_signature(&sig), Err(EcdsaError::InvalidSignatureEncoding));
    }

    #[test]
    fn truncate_hash_uses_whole_digest_when_shorter_than_order() {
        let digest = [0xAAu8; 32];
        let z = truncate_hash(&digest, 384); // P-384 order length, longer than a SHA-256 digest.
        assert_eq!(z, BigUint::from_be_bytes(&digest).unwrap());
    }

    #[test]
    fn truncate_hash_drops_low_bits_when_longer_than_order() {
        let mut digest = [0u8; 32];
        digest[31] = 0xff;
        // Order length one bit short of the full 256 bits: the single
        // overhanging low bit must be dropped, i.e. the result is the whole
        // digest shifted right by exactly one bit.
        let z = truncate_hash(&digest, 255);
        assert_eq!(z, BigUint::from_be_bytes(&digest).unwrap().shr(1));
    }
}
