//! Big-integer differential tests against Python's arbitrary-precision
//! integers (`scripts/gen-crypto-vectors.py`'s `bigint_python_cross.txt`,
//! brief M8-T3): random 2048- and 4096-bit odd moduli, checking
//! [`Modulus::mul_mod`], the Montgomery setup constants
//! [`Modulus::r_mod_n`]/[`Modulus::r2_mod_n`], and [`Modulus::pow_mod`]
//! against Python's own `(a*b) % n`, `pow(2, bits, n)`/`pow(2, 2*bits, n)`
//! and `pow(a, e, n)`.

mod common;

use common::{hex_decode, read_fixture};
use otter_crypto::bigint::{BigUint, Modulus};

fn big(hex: &str) -> BigUint {
    BigUint::from_be_bytes(&hex_decode(hex)).expect("fixture value fits this crate's bigint capacity")
}

#[test]
fn bigint_python_cross() {
    let rows = read_fixture("bigint_python_cross.txt");
    assert_eq!(rows.len(), 40, "expected 20 cases each for 2048- and 4096-bit moduli");

    let mut checked_2048 = 0;
    let mut checked_4096 = 0;
    for row in &rows {
        let [bits, n_hex, a_hex, b_hex, mul_expected, r_mod_n_expected, r2_mod_n_expected, exponent_hex, pow_expected] =
            row.as_slice()
        else {
            panic!("row has {} fields, expected 9: {row:?}", row.len());
        };
        let bits: usize = bits.parse().unwrap();
        assert!(bits == 2048 || bits == 4096, "unexpected bit size {bits}");

        let n = big(n_hex);
        assert_eq!(n.bit_len(), bits, "fixture modulus should have exactly {bits} bits");
        let modulus = Modulus::new(n).expect("fixture modulus is odd and in range");

        let a = big(a_hex);
        let b = big(b_hex);

        // Multiplication.
        assert_eq!(modulus.mul_mod(&a, &b), big(mul_expected), "bits={bits}: a*b mod n mismatch");

        // Montgomery reduction-constant setup.
        assert_eq!(modulus.r_mod_n(), big(r_mod_n_expected), "bits={bits}: R mod n mismatch");
        assert_eq!(modulus.r2_mod_n(), big(r2_mod_n_expected), "bits={bits}: R^2 mod n mismatch");

        // Exponentiation.
        let exponent = big(exponent_hex);
        assert_eq!(modulus.pow_mod(&a, &exponent), big(pow_expected), "bits={bits}: a^e mod n mismatch");

        if bits == 2048 {
            checked_2048 += 1;
        } else {
            checked_4096 += 1;
        }
    }
    assert_eq!(checked_2048, 20);
    assert_eq!(checked_4096, 20);
}
