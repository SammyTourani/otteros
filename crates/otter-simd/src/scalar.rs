//! Scalar (scalar-only) kernels for f32 and Q8_0 matrix-vector products.

#![allow(clippy::needless_range_loop)]

use core::ops::Range;
use crate::math;

// Re-export math functions for testing
pub use crate::math::sqrt as sqrt_impl;
pub use crate::math::exp as exp_impl;

/// Dot product of two f32 vectors.
#[inline]
pub fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    let mut sum = 0.0;
    for i in 0..len {
        sum += a[i] * b[i];
    }
    sum
}

/// F32 matrix-vector product: out = W @ x, where W is rows x cols (row-major).
pub fn matvec_f32(w: &[f32], rows: usize, cols: usize, x: &[f32], out: &mut [f32]) {
    assert!(
        w.len() >= rows.saturating_mul(cols),
        "scalar f32: w too small ({} < {}*{})",
        w.len(),
        rows,
        cols
    );
    assert!(
        x.len() >= cols,
        "scalar f32: x too small ({} < {})",
        x.len(),
        cols
    );
    assert!(
        out.len() >= rows,
        "scalar f32: out too small ({} < {})",
        out.len(),
        rows
    );
    matvec_f32_rows(0..rows, w, rows, cols, x, out);
}

/// F32 matrix-vector product for a row range: out[row_range] = W[row_range, :] @ x.
pub fn matvec_f32_rows(
    row_range: Range<usize>,
    w: &[f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    out: &mut [f32],
) {
    let x_len = x.len().min(cols);
    for r in row_range {
        if r >= rows {
            break;
        }
        let row_start = r * cols;
        let mut sum = 0.0;
        for c in 0..x_len {
            sum += w[row_start + c] * x[c];
        }
        if r < out.len() {
            out[r] = sum;
        }
    }
}

/// Q8_0 matrix-vector product: out = W @ x, where W is quantized.
///
/// Q8_0 layout: blocks of 32 int8 weights + 1 f32 scale, 36 bytes per block.
/// Tensor flattened row-major; blocks straddle rows.
pub fn matvec_q8(w_blocks: &[u8], rows: usize, cols: usize, x: &[f32], out: &mut [f32]) {
    let needed_blocks = (rows.saturating_mul(cols).saturating_add(31)) / 32;
    let needed_bytes = needed_blocks.saturating_mul(36);
    assert!(
        w_blocks.len() >= needed_bytes,
        "scalar q8: w_blocks too small ({} < {})",
        w_blocks.len(),
        needed_bytes
    );
    assert!(
        x.len() >= cols,
        "scalar q8: x too small ({} < {})",
        x.len(),
        cols
    );
    assert!(
        out.len() >= rows,
        "scalar q8: out too small ({} < {})",
        out.len(),
        rows
    );
    matvec_q8_rows(0..rows, w_blocks, rows, cols, x, out);
}

/// Q8_0 matrix-vector product for a row range.
pub fn matvec_q8_rows(
    row_range: Range<usize>,
    w_blocks: &[u8],
    rows: usize,
    cols: usize,
    x: &[f32],
    out: &mut [f32],
) {
    let x_len = x.len().min(cols);

    for r in row_range {
        if r >= rows {
            break;
        }

        let mut sum = 0.0;

        for c in 0..x_len {
            let flat_idx = r * cols + c;
            let block_idx = flat_idx / 32;
            let lane = flat_idx % 32;

            let block_offset = block_idx * 36;

            // Extract the int8 weight and f32 scale
            if block_offset + 32 < w_blocks.len() {
                let q8_val = w_blocks[block_offset + lane] as i8 as f32;

                // Scale is at bytes 32..36 of the block (little-endian f32)
                if block_offset + 36 <= w_blocks.len() {
                    let scale_bytes = [
                        w_blocks[block_offset + 32],
                        w_blocks[block_offset + 33],
                        w_blocks[block_offset + 34],
                        w_blocks[block_offset + 35],
                    ];
                    let scale = f32::from_le_bytes(scale_bytes);
                    sum += q8_val * scale * x[c];
                }
            }
        }

        if r < out.len() {
            out[r] = sum;
        }
    }
}

/// Root mean square normalization: x = x / (RMS(x) + eps).
pub fn rmsnorm(x: &mut [f32], eps: f32) {
    if x.is_empty() {
        return;
    }

    let mut sq_sum = 0.0;
    for &v in x.iter() {
        sq_sum += v * v;
    }
    let rms = math::sqrt(sq_sum / x.len() as f32 + eps);

    for v in x.iter_mut() {
        *v /= rms;
    }
}

/// AXPY: y = alpha * x + y.
pub fn axpy(alpha: f32, x: &[f32], y: &mut [f32]) {
    let len = x.len().min(y.len());
    for i in 0..len {
        y[i] += alpha * x[i];
    }
}

/// Softmax in place: x[i] = exp(x[i] - max) / sum(exp(x[j] - max)).
pub fn softmax_inplace(x: &mut [f32]) {
    if x.is_empty() {
        return;
    }

    // Find max for numerical stability
    let max = x.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));

    // Compute exp(x[i] - max) and sum
    let mut sum = 0.0;
    for v in x.iter_mut() {
        *v = math::exp(*v - max);
        sum += *v;
    }

    // Normalize
    if sum > 0.0 {
        for v in x.iter_mut() {
            *v /= sum;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_dot_f32() {
        let a = [1.0, 2.0, 3.0];
        let b = [4.0, 5.0, 6.0];
        let result = dot_f32(&a, &b);
        // 1*4 + 2*5 + 3*6 = 4 + 10 + 18 = 32
        assert!((result - 32.0).abs() < 1e-6);
    }

    #[test]
    fn test_matvec_f32_simple() {
        // 2x3 matrix:
        // [1 2 3]
        // [4 5 6]
        // x = [1, 1, 1]
        // out = [6, 15]
        let w = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let x = [1.0, 1.0, 1.0];
        let mut out = [0.0; 2];
        matvec_f32(&w, 2, 3, &x, &mut out);
        assert!((out[0] - 6.0).abs() < 1e-6);
        assert!((out[1] - 15.0).abs() < 1e-6);
    }

    #[test]
    fn test_matvec_f32_rows() {
        let w = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let x = [1.0, 1.0, 1.0];
        let mut out = [0.0; 2];
        // Compute only row 1
        matvec_f32_rows(1..2, &w, 2, 3, &x, &mut out);
        assert!(out[0] == 0.0); // row 0 not computed
        assert!((out[1] - 15.0).abs() < 1e-6);
    }

    #[test]
    fn test_matvec_q8_simple() {
        // Build a simple Q8_0 block: 32 int8 values + 1 f32 scale
        // Let's use scale = 1.0 and weights = [1, 2, 3, ..., 32, 0, 0, 0, ...]
        // Row 0: dot([1, 2, 3, ...], [1, 1, 1, ...])

        let mut block = [0u8; 64]; // 36 for block 0, 28 padding

        // Block 0: int8 values 1..32 and scale 1.0
        for i in 0..32 {
            block[i] = (i + 1) as u8;
        }
        // f32 scale = 1.0 at bytes 32..36
        let scale_bytes = 1.0f32.to_le_bytes();
        block[32..36].copy_from_slice(&scale_bytes);

        let x = [1.0; 32];
        let mut out = [0.0];
        matvec_q8(&block, 1, 32, &x, &mut out);

        // sum = 1*1 + 2*1 + ... + 32*1 = 1+2+...+32 = 32*33/2 = 528
        let expected = 528.0;
        assert!((out[0] - expected).abs() < 1e-6, "got {}, expected {}", out[0], expected);
    }

    #[test]
    fn test_matvec_q8_straddling_blocks() {
        // Test that Q8_0 blocks can straddle row boundaries.
        // 2 rows x 64 cols = 128 elements total
        // Block 0: elements 0..32 (row 0, cols 0..32)
        // Block 1: elements 32..64 (row 0, cols 32..64)
        // Block 2: elements 64..96 (row 1, cols 0..32)
        // Block 3: elements 96..128 (row 1, cols 32..64)

        // Build 4 blocks of Q8_0 data
        let mut blocks = vec![0u8; 4 * 36];
        for block_idx in 0..4 {
            let offset = block_idx * 36;
            // Fill with uint8(block_idx+1) for each lane
            for lane in 0..32 {
                blocks[offset + lane] = (block_idx + 1) as u8;
            }
            // Scale = 1.0 for all blocks
            let scale = 1.0f32.to_le_bytes();
            blocks[offset + 32..offset + 36].copy_from_slice(&scale);
        }

        let x = [1.0; 64];
        let mut out = [0.0; 2];
        matvec_q8(&blocks, 2, 64, &x, &mut out);

        // Row 0: block 0 (32x1) + block 1 (32x2) = 32 + 64 = 96
        // Row 1: block 2 (32x3) + block 3 (32x4) = 96 + 128 = 224
        assert!((out[0] - 96.0).abs() < 1e-6, "row 0: got {}", out[0]);
        assert!((out[1] - 224.0).abs() < 1e-6, "row 1: got {}", out[1]);
    }

    #[test]
    fn test_rmsnorm() {
        let mut x = [3.0, 4.0];
        // rms = sqrt((9 + 16) / 2) = sqrt(12.5) ≈ 3.535
        rmsnorm(&mut x, 0.0);
        let rms = (12.5f32).sqrt();
        assert!((x[0] - 3.0 / rms).abs() < 1e-6);
        assert!((x[1] - 4.0 / rms).abs() < 1e-6);
    }

    #[test]
    fn test_axpy() {
        let x = [1.0, 2.0, 3.0];
        let mut y = [1.0, 1.0, 1.0];
        axpy(2.0, &x, &mut y);
        // y = 2*[1,2,3] + [1,1,1] = [3, 5, 7]
        assert!((y[0] - 3.0).abs() < 1e-6);
        assert!((y[1] - 5.0).abs() < 1e-6);
        assert!((y[2] - 7.0).abs() < 1e-6);
    }

    #[test]
    fn test_softmax_simple() {
        let mut x = [0.0, 0.0, 0.0];
        softmax_inplace(&mut x);
        // All equal, so each should be 1/3
        assert!((x[0] - 1.0 / 3.0).abs() < 1e-6);
        assert!((x[1] - 1.0 / 3.0).abs() < 1e-6);
        assert!((x[2] - 1.0 / 3.0).abs() < 1e-6);
    }
}
