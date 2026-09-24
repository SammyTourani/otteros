#![no_std]
#![deny(unsafe_code)]
#![allow(clippy::type_complexity)]

//! SIMD kernels for matrix-vector products (f32 and Q8_0) with scalar/SSE2/AVX2 dispatch.
//! DECISIONS.md D2 (no external deps), D21 (Q8_0, CPUID dispatch), D27 (pure crate).

extern crate alloc;

use core::ops::Range;

#[cfg(target_arch = "x86_64")]
pub mod hw;

pub mod math;
pub mod scalar;

#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
pub mod sse2;

#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
pub mod avx2;

pub use scalar::{
    dot_f32 as dot_f32_scalar,
    matvec_f32 as matvec_f32_scalar,
    matvec_f32_rows as matvec_f32_rows_scalar,
    matvec_q8 as matvec_q8_scalar,
    matvec_q8_rows as matvec_q8_rows_scalar,
    rmsnorm as rmsnorm_scalar,
    axpy as axpy_scalar,
    softmax_inplace as softmax_inplace_scalar,
};

/// Per-ISA function pointers for dynamic dispatch.
pub struct Kernels {
    pub dot_f32: fn(&[f32], &[f32]) -> f32,
    pub matvec_f32: fn(&[f32], usize, usize, &[f32], &mut [f32]),
    pub matvec_f32_rows: fn(Range<usize>, &[f32], usize, usize, &[f32], &mut [f32]),
    pub matvec_q8: fn(&[u8], usize, usize, &[f32], &mut [f32]),
    pub matvec_q8_rows: fn(Range<usize>, &[u8], usize, usize, &[f32], &mut [f32]),
    pub rmsnorm: fn(&mut [f32], f32),
    pub axpy: fn(f32, &[f32], &mut [f32]),
    pub softmax_inplace: fn(&mut [f32]),
}

impl Kernels {
    /// Detect available ISA at runtime and select kernels.
    pub fn detect() -> Self {
        // Try AVX2 first (requires SSE2 as baseline)
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        {
            if hw::avx2_supported() {
                return Self {
                    dot_f32: avx2::dot_f32,
                    matvec_f32: avx2::matvec_f32,
                    matvec_f32_rows: avx2::matvec_f32_rows,
                    matvec_q8: avx2::matvec_q8,
                    matvec_q8_rows: avx2::matvec_q8_rows,
                    rmsnorm: avx2::rmsnorm,
                    axpy: avx2::axpy,
                    softmax_inplace: avx2::softmax_inplace,
                };
            }

            // Fall back to SSE2
            if hw::sse2_supported() {
                return Self {
                    dot_f32: sse2::dot_f32,
                    matvec_f32: sse2::matvec_f32,
                    matvec_f32_rows: sse2::matvec_f32_rows,
                    matvec_q8: sse2::matvec_q8,
                    matvec_q8_rows: sse2::matvec_q8_rows,
                    rmsnorm: sse2::rmsnorm,
                    axpy: sse2::axpy,
                    softmax_inplace: sse2::softmax_inplace,
                };
            }
        }

        // Scalar fallback
        Self::scalar()
    }

    /// Always use scalar kernels (for testing).
    pub fn scalar() -> Self {
        Self {
            dot_f32: scalar::dot_f32,
            matvec_f32: scalar::matvec_f32,
            matvec_f32_rows: scalar::matvec_f32_rows,
            matvec_q8: scalar::matvec_q8,
            matvec_q8_rows: scalar::matvec_q8_rows,
            rmsnorm: scalar::rmsnorm,
            axpy: scalar::axpy,
            softmax_inplace: scalar::softmax_inplace,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernels_select_correctly() {
        let k = Kernels::detect();
        let scalar = Kernels::scalar();
        // Just verify the detect path doesn't panic and returns a valid struct.
        let a = [1.0, 2.0, 3.0];
        let b = [4.0, 5.0, 6.0];
        let _ = (k.dot_f32)(&a, &b);
        let _ = (scalar.dot_f32)(&a, &b);
    }
}
