//! Project Wycheproof's X25519 test vectors
//! (`scripts/gen-crypto-vectors.py`'s `x25519_wycheproof.txt`). Every one of
//! this file's 518 cases is labeled `valid` or `acceptable` (Wycheproof has no
//! `invalid` X25519 cases: `X25519(k, u)` is a pure field computation with no
//! failure mode of its own -- "acceptable" covers edge/twist-point inputs
//! where the RFC 7748 formula still produces a well-defined result even
//! though the input was not a valid main-subgroup point). This test handles
//! every flag Wycheproof attaches the same way: decode, run `x25519`, and
//! check against the given `shared` value -- with exactly one exception,
//! spelled out by RFC 7748 section 6.1 itself: when the *given* `shared`
//! value is the all-zero string (the low-order-point cases, flagged
//! `ZeroSharedSecret`/`LowOrderPublic`/etc.), this crate's `x25519` returns
//! [`X25519Error::LowOrderPoint`] instead of `Ok`, and Wycheproof marks
//! exactly those cases `acceptable` (rejecting them is one of the two
//! documented acceptable behaviors).

mod common;

use common::{hex_decode, hex_encode, read_fixture};
use otter_crypto::{X25519Error, x25519};

#[test]
fn x25519_wycheproof() {
    let rows = read_fixture("x25519_wycheproof.txt");
    assert!(rows.len() >= 400, "expected a substantial number of X25519 cases, got {}", rows.len());

    let mut computed_matching_shared = 0;
    let mut rejected_zero_shared = 0;
    for row in &rows {
        let [tcid, result, flags, private_hex, public_hex, shared_hex] = row.as_slice() else {
            panic!("row has {} fields, expected 6: {row:?}", row.len());
        };
        assert_ne!(result, "invalid", "tcId={tcid}: this fixture is not expected to have invalid cases");

        let private: [u8; 32] = hex_decode(private_hex).try_into().unwrap();
        let public: [u8; 32] = hex_decode(public_hex).try_into().unwrap();
        let shared = hex_decode(shared_hex);
        let shared_is_zero = shared.iter().all(|&b| b == 0);

        match x25519(&private, &public) {
            Ok(out) => {
                assert!(
                    !shared_is_zero,
                    "tcId={tcid} flags={flags}: expected the all-zero-secret rejection, got Ok"
                );
                assert_eq!(hex_encode(&out), *shared_hex, "tcId={tcid} flags={flags}: shared secret mismatch");
                computed_matching_shared += 1;
            }
            Err(X25519Error::LowOrderPoint) => {
                assert!(
                    shared_is_zero,
                    "tcId={tcid} flags={flags}: rejected as low-order but the reference shared secret was not zero"
                );
                assert_eq!(
                    result.as_str(),
                    "acceptable",
                    "tcId={tcid} flags={flags}: a zero shared secret should be flagged acceptable, not valid"
                );
                rejected_zero_shared += 1;
            }
        }
    }

    assert_eq!(computed_matching_shared + rejected_zero_shared, rows.len());
    assert!(computed_matching_shared >= 200, "expected most cases to compute a matching shared secret");
    assert!(rejected_zero_shared >= 10, "expected several low-order-point rejections");
}
