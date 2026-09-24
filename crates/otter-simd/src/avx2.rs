//! AVX2+FMA kernels (8-wide f32 SIMD).
//! CPUID checked before use; only reachable if avx2_supported() && fma_supported() == true.
//!
//! All kernels wrap unsafe inner functions marked #[target_feature(enable = "avx2,fma")].
//! SAFETY: Kernels::detect() only selects these after CPUID+XGETBV confirm both features.

#![allow(unsafe_code)]

use core::arch::x86_64::*;
use core::ops::Range;

// ===== BOUNDS CHECKING =====

fn check_bounds(w: &[f32], rows: usize, cols: usize, x: &[f32], out: &[f32]) {
    assert!(
        w.len() >= rows.saturating_mul(cols),
        "avx2 f32: w too small ({} < {}*{})",
        w.len(),
        rows,
        cols
    );
    assert!(
        x.len() >= cols,
        "avx2 f32: x too small ({} < {})",
        x.len(),
        cols
    );
    assert!(
        out.len() >= rows,
        "avx2 f32: out too small ({} < {})",
        out.len(),
        rows
    );
}

fn check_bounds_q8(w_blocks: &[u8], rows: usize, cols: usize, x: &[f32], out: &[f32]) {
    let needed_blocks = (rows.saturating_mul(cols).saturating_add(31)) / 32;
    let needed_bytes = needed_blocks.saturating_mul(36);
    assert!(
        w_blocks.len() >= needed_bytes,
        "avx2 q8: w_blocks too small ({} < {})",
        w_blocks.len(),
        needed_bytes
    );
    assert!(
        x.len() >= cols,
        "avx2 q8: x too small ({} < {})",
        x.len(),
        cols
    );
    assert!(
        out.len() >= rows,
        "avx2 q8: out too small ({} < {})",
        out.len(),
        rows
    );
}

// ===== DOT PRODUCT =====

#[inline]
#[target_feature(enable = "avx2,fma")]
unsafe fn dot_f32_inner(a: &[f32], b: &[f32]) -> f32 {
    // SAFETY: the loop reads a[i..i+W] and b[i..i+W] only while i + W <= len = min(a.len(), b.len()),
    // and the scalar tail indexes below len; the target features are guaranteed by Kernels::detect().
    unsafe {
        let len = a.len().min(b.len());
        let mut acc = _mm256_setzero_ps();

        let chunks = len / 8;
        for i in 0..chunks {
            let i8 = i * 8;
            let av = _mm256_loadu_ps(a.as_ptr().add(i8));
            let bv = _mm256_loadu_ps(b.as_ptr().add(i8));
            acc = _mm256_add_ps(acc, _mm256_mul_ps(av, bv));
        }

        // Horizontal sum
        let mut result = [0.0; 8];
        _mm256_storeu_ps(result.as_mut_ptr(), acc);
        let hsum = result.iter().sum::<f32>();

        hsum + crate::scalar::dot_f32(&a[chunks * 8..], &b[chunks * 8..])
    }
}

pub fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
    // SAFETY: Kernels::detect() has already verified AVX2+FMA support via CPUID+XGETBV.
    unsafe { dot_f32_inner(a, b) }
}

// ===== F32 MATRIX-VECTOR =====

#[target_feature(enable = "avx2,fma")]
unsafe fn matvec_f32_rows_inner(
    row_range: Range<usize>,
    w: &[f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    out: &mut [f32],
) {
    // SAFETY: the caller checked w.len() >= rows*cols, x.len() >= cols and out.len() >= rows; every
    // pointer offset below is r*cols + c with r < rows and c + W <= cols (or c < cols in the tail).
    unsafe {
        let x_len = x.len().min(cols);

        for r in row_range {
            if r >= rows {
                break;
            }

            let row_start = r * cols;

            // Four 8-lane accumulators for latency hiding
            let mut acc0 = _mm256_setzero_ps();
            let mut acc1 = _mm256_setzero_ps();
            let mut acc2 = _mm256_setzero_ps();
            let mut acc3 = _mm256_setzero_ps();

            // Process 32 columns at a time
            let chunks32 = x_len / 32;
            for c32 in 0..chunks32 {
                let c = c32 * 32;
                // Four 8-lane FMAs per iteration
                for i in 0..4 {
                    let c8 = c + i * 8;
                    let wv = _mm256_loadu_ps(w.as_ptr().add(row_start + c8));
                    let xv = _mm256_loadu_ps(x.as_ptr().add(c8));
                    match i {
                        0 => acc0 = _mm256_fmadd_ps(wv, xv, acc0),
                        1 => acc1 = _mm256_fmadd_ps(wv, xv, acc1),
                        2 => acc2 = _mm256_fmadd_ps(wv, xv, acc2),
                        3 => acc3 = _mm256_fmadd_ps(wv, xv, acc3),
                        _ => unreachable!(),
                    }
                }
            }

            // Reduce four accumulators
            let sum_acc = _mm256_add_ps(_mm256_add_ps(acc0, acc1), _mm256_add_ps(acc2, acc3));
            let mut sum_vals = [0.0; 8];
            _mm256_storeu_ps(sum_vals.as_mut_ptr(), sum_acc);
            let mut sum = sum_vals.iter().sum::<f32>();

            // Scalar tail
            let tail_start = chunks32 * 32;
            for c in tail_start..x_len {
                sum += *w.get_unchecked(row_start + c) * *x.get_unchecked(c);
            }

            if r < out.len() {
                out[r] = sum;
            }
        }
    }
}

pub fn matvec_f32(w: &[f32], rows: usize, cols: usize, x: &[f32], out: &mut [f32]) {
    check_bounds(w, rows, cols, x, out);
    // SAFETY: Bounds checked above; AVX2+FMA verified by Kernels::detect().
    unsafe { matvec_f32_rows_inner(0..rows, w, rows, cols, x, out) }
}

pub fn matvec_f32_rows(
    row_range: Range<usize>,
    w: &[f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    out: &mut [f32],
) {
    check_bounds(w, rows, cols, x, out);
    // SAFETY: Bounds checked above; AVX2+FMA verified by Kernels::detect().
    unsafe { matvec_f32_rows_inner(row_range, w, rows, cols, x, out) }
}

// ===== Q8_0 MATRIX-VECTOR =====

#[target_feature(enable = "avx2,fma")]
unsafe fn matvec_q8_rows_inner(
    row_range: Range<usize>,
    w_blocks: &[u8],
    rows: usize,
    cols: usize,
    x: &[f32],
    out: &mut [f32],
) {
    // Row r covers flat indices [r*cols, (r+1)*cols). It is split at 32-element block boundaries:
    // whole blocks run fully vectorised (one scale per block), and only the ends of a row that
    // straddle a block (cols % 32 != 0) take the short scalar path.
    let wp = w_blocks.as_ptr();
    let xp = x.as_ptr();
    for r in row_range {
        if r >= rows {
            break;
        }
        let start = r * cols;
        let end = start + cols;
        let mut acc = _mm256_setzero_ps();
        let mut tail = 0.0f32;
        let mut idx = start;
        while idx < end {
            let lane = idx % 32;
            let n = (32 - lane).min(end - idx);
            // SAFETY: the caller checked w_blocks.len() >= ceil(rows*cols/32)*36 and x.len() >= cols;
            // idx < rows*cols, so block idx/32 and its 4-byte scale are in bounds, and the n weights
            // and x[idx-start .. idx-start+n] read below stay inside the block and inside x.
            unsafe {
                let block = wp.add((idx / 32) * 36);
                let scale = f32::from_le_bytes(core::ptr::read_unaligned(block.add(32) as *const [u8; 4]));
                let xs = xp.add(idx - start);
                if n == 32 {
                    let q0 = _mm256_cvtepi32_ps(_mm256_cvtepi8_epi32(_mm_loadl_epi64(block as *const __m128i)));
                    let q1 = _mm256_cvtepi32_ps(_mm256_cvtepi8_epi32(_mm_loadl_epi64(block.add(8) as *const __m128i)));
                    let q2 = _mm256_cvtepi32_ps(_mm256_cvtepi8_epi32(_mm_loadl_epi64(block.add(16) as *const __m128i)));
                    let q3 = _mm256_cvtepi32_ps(_mm256_cvtepi8_epi32(_mm_loadl_epi64(block.add(24) as *const __m128i)));
                    let p0 = _mm256_mul_ps(q0, _mm256_loadu_ps(xs));
                    let p1 = _mm256_mul_ps(q1, _mm256_loadu_ps(xs.add(8)));
                    let p2 = _mm256_fmadd_ps(q2, _mm256_loadu_ps(xs.add(16)), p0);
                    let p3 = _mm256_fmadd_ps(q3, _mm256_loadu_ps(xs.add(24)), p1);
                    acc = _mm256_fmadd_ps(_mm256_add_ps(p2, p3), _mm256_set1_ps(scale), acc);
                } else {
                    let mut s = 0.0f32;
                    for i in 0..n {
                        s += (*block.add(lane + i) as i8) as f32 * *xs.add(i);
                    }
                    tail += s * scale;
                }
            }
            idx += n;
        }
        out[r] = hsum256_ps(acc) + tail;
    }
}

#[target_feature(enable = "avx2,fma")]
fn hsum256_ps(v: __m256) -> f32 {
    let s = _mm_add_ps(_mm256_castps256_ps128(v), _mm256_extractf128_ps(v, 1));
    let s = _mm_add_ps(s, _mm_movehl_ps(s, s));
    _mm_cvtss_f32(_mm_add_ss(s, _mm_shuffle_ps(s, s, 1)))
}

pub fn matvec_q8(w_blocks: &[u8], rows: usize, cols: usize, x: &[f32], out: &mut [f32]) {
    check_bounds_q8(w_blocks, rows, cols, x, out);
    // SAFETY: Bounds checked above; AVX2+FMA verified by Kernels::detect().
    unsafe { matvec_q8_rows_inner(0..rows, w_blocks, rows, cols, x, out) }
}

pub fn matvec_q8_rows(
    row_range: Range<usize>,
    w_blocks: &[u8],
    rows: usize,
    cols: usize,
    x: &[f32],
    out: &mut [f32],
) {
    check_bounds_q8(w_blocks, rows, cols, x, out);
    // SAFETY: Bounds checked above; AVX2+FMA verified by Kernels::detect().
    unsafe { matvec_q8_rows_inner(row_range, w_blocks, rows, cols, x, out) }
}

// ===== HELPER FUNCTIONS =====

pub fn rmsnorm(x: &mut [f32], eps: f32) {
    if x.is_empty() {
        return;
    }
    // Use scalar implementation for now
    crate::scalar::rmsnorm(x, eps);
}

pub fn axpy(alpha: f32, x: &[f32], y: &mut [f32]) {
    let len = x.len().min(y.len());
    let alpha_vec = unsafe { _mm256_set1_ps(alpha) };

    let chunks = len / 8;
    for i in 0..chunks {
        let i8 = i * 8;
        // SAFETY: Bounds checked by len calculation.
        let xv = unsafe { _mm256_loadu_ps(x.as_ptr().add(i8)) };
        let yv = unsafe { _mm256_loadu_ps(y.as_ptr().add(i8)) };
        let prod = unsafe { _mm256_mul_ps(alpha_vec, xv) };
        let result = unsafe { _mm256_add_ps(prod, yv) };
        unsafe { _mm256_storeu_ps(y.as_mut_ptr().add(i8), result) };
    }

    for i in (chunks * 8)..len {
        y[i] += alpha * x[i];
    }
}

pub fn softmax_inplace(x: &mut [f32]) {
    if x.is_empty() {
        return;
    }
    // Use scalar implementation for now
    crate::scalar::softmax_inplace(x);
}
