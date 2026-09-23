//! GHASH's GF(2^128) block multiplication accelerated with PCLMULQDQ.
//!
//! GCM's bit convention (NIST SP 800-38D section 6.3) makes the *leftmost* bit
//! of a block the coefficient of the *lowest* power of the field polynomial --
//! the opposite of the usual "leftmost = most significant" reading of a number.
//! [`u128::reverse_bits`] (a plain, `const`-eligible core operation, not a
//! hand-written bit trick) converts a GCM-convention block into the "normal"
//! convention (bit 0 = lowest power) that a schoolbook carry-less multiply and a
//! schoolbook polynomial reduction expect; reversing the final result converts
//! back. This keeps every actual multiply/reduce step below expressed in the
//! ordinary convention, where it can be checked directly against the field
//! polynomial `x^128 + x^7 + x^2 + x + 1` (see `reduce`'s comment), rather than
//! against a memorized, already-reduced-and-reflected constant.

use core::arch::x86_64::{__m128i, _mm_clmulepi64_si128};

/// The raw 128x128 -> 256-bit carry-less product of `a` and `b` (schoolbook
/// multiplication of two 64-bit-limbed 128-bit polynomials: four 64x64 -> 128
/// PCLMULQDQ products combined the same way `u128 = u64 * u64` schoolbook
/// multiplication combines its four partial products, except every "+" is a
/// carry-less XOR here instead of an addition with carries). Returns
/// `(low_128_bits, high_128_bits)` of the 256-bit product, both in the "normal"
/// (non-GCM-reflected) convention.
#[target_feature(enable = "pclmulqdq")]
unsafe fn clmul_256(a: u128, b: u128) -> (u128, u128) {
    // SAFETY: only reachable through `mul`, which checks
    // `crate::hw::pclmulqdq_available()` (CPUID leaf 1 ECX bit 1) first.
    unsafe {
        let av: __m128i = core::mem::transmute(a);
        let bv: __m128i = core::mem::transmute(b);
        // Immediate bit 0 selects a's half (0 = low 64 bits, 1 = high), bit 4
        // selects b's half; see `core::arch::x86_64::_mm_clmulepi64_si128`.
        let lo_lo: u128 = core::mem::transmute(_mm_clmulepi64_si128::<0x00>(av, bv));
        let hi_hi: u128 = core::mem::transmute(_mm_clmulepi64_si128::<0x11>(av, bv));
        let lo_hi: u128 = core::mem::transmute(_mm_clmulepi64_si128::<0x01>(av, bv));
        let hi_lo: u128 = core::mem::transmute(_mm_clmulepi64_si128::<0x10>(av, bv));
        let mid = lo_hi ^ hi_lo; // both are x^64 * (cross term); combine before shifting.
        let low = lo_lo ^ (mid << 64);
        let high = hi_hi ^ (mid >> 64);
        (low, high)
    }
}

/// Reduces a 256-bit carry-less product `(lo, hi)` (normal convention: `lo`
/// holds coefficients of `x^0..x^127`, `hi` holds `x^128..x^255`) modulo
/// `f(x) = x^128 + x^7 + x^2 + x + 1`.
///
/// `f(x) = 0` in the field means `x^128 = x^7 + x^2 + x + 1`, so `hi`
/// (representing `hi_poly(x) * x^128` for the degree-<128 polynomial
/// `hi_poly` whose bits are `hi`) is congruent to `hi_poly(x) * (x^7+x^2+x+1)`,
/// i.e. `hi ^ (hi<<1) ^ (hi<<2) ^ (hi<<7)` -- except that shift can itself carry
/// up to 7 bits past position 127 (degree up to 134), so those overflow bits
/// (`t`, at most 7 of them) get the identical treatment once more; `t`'s own
/// shifted-out overflow is at most 13 bits, nowhere near degree 128, so a
/// second round always fully finishes the reduction.
fn reduce(lo: u128, hi: u128) -> u128 {
    let fold = hi ^ (hi << 1) ^ (hi << 2) ^ (hi << 7);
    let t = (hi >> 127) ^ (hi >> 126) ^ (hi >> 121);
    let correction = t ^ (t << 1) ^ (t << 2) ^ (t << 7);
    lo ^ fold ^ correction
}

/// Multiplies two GCM-convention GF(2^128) elements via PCLMULQDQ, or returns
/// `None` if this CPU has no PCLMULQDQ (`crate::hw::pclmulqdq_available()`) --
/// callers fall back to [`crate::ghash::gf128_mul`].
pub fn mul(x: u128, h: u128) -> Option<u128> {
    if !crate::hw::pclmulqdq_available() {
        return None;
    }
    let xr = x.reverse_bits();
    let hr = h.reverse_bits();
    // SAFETY: the check above is exactly `clmul_256`'s precondition.
    let (lo, hi) = unsafe { clmul_256(xr, hr) };
    Some(reduce(lo, hi).reverse_bits())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghash::gf128_mul;

    #[test]
    fn matches_software_ghash_on_random_inputs() {
        let Some(first) = mul(1, 1) else {
            eprintln!("skipping: PCLMULQDQ not available on this host");
            return;
        };
        assert_eq!(first, gf128_mul(1, 1));

        let mut state = 0x243f6a8885a308d313198a2e03707344u128; // arbitrary seed (public test data).
        for _ in 0..2000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let x = state;
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let h = state;
            assert_eq!(mul(x, h).unwrap(), gf128_mul(x, h), "mismatch for x={x:#x} h={h:#x}");
        }
    }
}
