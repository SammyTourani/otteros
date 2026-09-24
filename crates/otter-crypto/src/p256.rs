//! NIST P-256 (secp256r1) ECDSA signature verification, built on
//! [`crate::ecc`]'s shared point arithmetic and verify algorithm.
//!
//! The domain parameters below (FIPS 186-4 Appendix D.1.2.3 / SEC 2
//! `secp256r1`) were generated from values independently cross-checked
//! (host-side, during development, not part of this crate) against the
//! field prime's closed form `2^256 - 2^224 + 2^192 + 2^96 - 1`, confirming
//! the base point is on the curve and has the stated order (`n * G` is the
//! point at infinity) and that both `p` and `n` are prime -- see the brief's
//! report for how; not re-derived at build time since that would need a
//! bignum library this crate does not otherwise have a use for.
//!
//! [`ecc::CurveParams`] is rebuilt fresh on every [`PublicKey::from_sec1`]
//! call rather than cached: this crate has zero dependencies (D2/D22/D27),
//! so no `lazy_static`/`once_cell`, and a hand-rolled `spin`-based
//! lazy-static would be one more piece of unsafe machinery to justify for
//! what a handful of Montgomery-setup doublings (microseconds, see
//! [`crate::bigint::Modulus::new`]) does not need.

use crate::ecc::{self, CurveParams, Point};

pub use crate::ecc::EcdsaError;

/// Coordinate/field size in bytes.
const BYTE_LEN: usize = 32;

// SEC 2 secp256r1 domain parameters, big-endian.
const P: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
];
const N: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xbc, 0xe6, 0xfa,
    0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51,
];
const B: [u8; 32] = [
    0x5a, 0xc6, 0x35, 0xd8, 0xaa, 0x3a, 0x93, 0xe7, 0xb3, 0xeb, 0xbd, 0x55, 0x76, 0x98, 0x86, 0xbc, 0x65, 0x1d, 0x06,
    0xb0, 0xcc, 0x53, 0xb0, 0xf6, 0x3b, 0xce, 0x3c, 0x3e, 0x27, 0xd2, 0x60, 0x4b,
];
const GX: [u8; 32] = [
    0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42, 0x47, 0xf8, 0xbc, 0xe6, 0xe5, 0x63, 0xa4, 0x40, 0xf2, 0x77, 0x03, 0x7d,
    0x81, 0x2d, 0xeb, 0x33, 0xa0, 0xf4, 0xa1, 0x39, 0x45, 0xd8, 0x98, 0xc2, 0x96,
];
const GY: [u8; 32] = [
    0x4f, 0xe3, 0x42, 0xe2, 0xfe, 0x1a, 0x7f, 0x9b, 0x8e, 0xe7, 0xeb, 0x4a, 0x7c, 0x0f, 0x9e, 0x16, 0x2b, 0xce, 0x33,
    0x57, 0x6b, 0x31, 0x5e, 0xce, 0xcb, 0xb6, 0x40, 0x68, 0x37, 0xbf, 0x51, 0xf5,
];

fn curve_params() -> CurveParams {
    CurveParams::new(&P, &N, &B, &GX, &GY, BYTE_LEN)
}

/// A P-256 public key: a validated, on-curve point, ready to verify ECDSA
/// signatures against.
pub struct PublicKey {
    point: Point,
}

impl PublicKey {
    /// Decodes a SEC1 uncompressed point (`0x04 || x || y`, 65 bytes), checks
    /// it is on the curve and is not the point at infinity.
    pub fn from_sec1(bytes: &[u8]) -> Result<PublicKey, EcdsaError> {
        let curve = curve_params();
        let point = ecc::point_from_sec1(&curve, bytes)?;
        Ok(PublicKey { point })
    }

    /// Verifies an ECDSA signature (DER `ECDSA-Sig-Value`) over `digest`
    /// (the caller's own hash -- SHA-256, conventionally, but this function
    /// only needs its length -- of the signed data).
    pub fn verify(&self, digest: &[u8], der_sig: &[u8]) -> Result<(), EcdsaError> {
        let curve = curve_params();
        ecc::ecdsa_verify(&curve, &self.point, digest, der_sig)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_params_build_without_panicking() {
        let _ = curve_params();
    }

    #[test]
    fn generator_round_trips_through_sec1_decode() {
        // Full verify correctness is Wycheproof's job (see
        // tests/ecdsa_p256_wycheproof.rs); this just confirms the hardcoded
        // domain parameters parse and the base point is accepted as a valid
        // on-curve public key.
        let mut sec1 = [0u8; 65];
        sec1[0] = 0x04;
        sec1[1..33].copy_from_slice(&GX);
        sec1[33..65].copy_from_slice(&GY);
        assert!(PublicKey::from_sec1(&sec1).is_ok());
    }

    #[test]
    fn from_sec1_rejects_point_at_infinity_encoding() {
        assert_eq!(PublicKey::from_sec1(&[0x00]).err(), Some(EcdsaError::InvalidPublicKey));
    }

    #[test]
    fn from_sec1_rejects_wrong_length() {
        let short = [0x04u8; 10];
        assert_eq!(PublicKey::from_sec1(&short).err(), Some(EcdsaError::InvalidPublicKey));
    }

    #[test]
    fn from_sec1_rejects_off_curve_point() {
        let mut sec1 = [0u8; 65];
        sec1[0] = 0x04;
        sec1[1..33].copy_from_slice(&GX);
        sec1[33..65].copy_from_slice(&GX); // (Gx, Gx) is not on the curve.
        assert_eq!(PublicKey::from_sec1(&sec1).err(), Some(EcdsaError::InvalidPublicKey));
    }
}
