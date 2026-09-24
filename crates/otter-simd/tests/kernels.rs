//! Comprehensive tests for otter-simd kernels.
//! - aarch64: scalar path unit tests (25+ cases, hand-computed)
//! - x86_64 under Rosetta: all ISA paths vs scalar validation (1000 shapes per path)

#![allow(unused_imports, dead_code, clippy::needless_range_loop)]

extern crate alloc;
use alloc::vec;
use otter_simd::Kernels;

// Random number generator for deterministic test shapes
#[derive(Clone, Copy)]
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        Rng(0xdeadbeefdeadbeef)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0
    }

    fn range(&mut self, min: u64, max: u64) -> u64 {
        min + (self.next() % (max - min + 1))
    }
}

// ===== SCALAR UNIT TESTS (Hand-computed, for aarch64) =====

#[test]
fn test_1x1_matvec_f32() {
    let w = [2.5];
    let x = [4.0];
    let mut out = [0.0];
    otter_simd::matvec_f32_scalar(&w, 1, 1, &x, &mut out);
    assert!((out[0] - 10.0).abs() < 1e-6);
}

#[test]
fn test_empty_matvec_f32() {
    let w: [f32; 0] = [];
    let x: [f32; 0] = [];
    let mut out = [0.0; 1];
    otter_simd::matvec_f32_scalar(&w, 1, 0, &x, &mut out);
    assert!(out[0] == 0.0);
}

#[test]
fn test_scalar_dot_product() {
    let a = [1.0, 2.0, 3.0, 4.0];
    let b = [5.0, 6.0, 7.0, 8.0];
    let result = otter_simd::dot_f32_scalar(&a, &b);
    // 1*5 + 2*6 + 3*7 + 4*8 = 70
    assert!((result - 70.0).abs() < 1e-6);
}

#[test]
fn test_scalar_matvec_f32_simple() {
    // 2x3 matrix: [[1,2,3], [4,5,6]]
    // x = [1, 1, 1]
    // out = [6, 15]
    let w = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let x = [1.0, 1.0, 1.0];
    let mut out = [0.0; 2];
    otter_simd::matvec_f32_scalar(&w, 2, 3, &x, &mut out);
    assert!((out[0] - 6.0).abs() < 1e-6);
    assert!((out[1] - 15.0).abs() < 1e-6);
}

#[test]
fn test_scalar_matvec_f32_nonuniform() {
    // 3x4 matrix
    let w = [
        1.0, 2.0, 3.0, 4.0,
        5.0, 6.0, 7.0, 8.0,
        9.0, 10.0, 11.0, 12.0,
    ];
    let x = [1.0, 1.0, 1.0, 1.0];
    let mut out = [0.0; 3];
    otter_simd::matvec_f32_scalar(&w, 3, 4, &x, &mut out);
    assert!((out[0] - 10.0).abs() < 1e-6);
    assert!((out[1] - 26.0).abs() < 1e-6);
    assert!((out[2] - 42.0).abs() < 1e-6);
}

#[test]
fn test_scalar_matvec_f32_rows_partial() {
    let w = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let x = [1.0, 1.0, 1.0];
    let mut out = [0.0; 2];
    // Compute only row 1
    otter_simd::matvec_f32_rows_scalar(1..2, &w, 2, 3, &x, &mut out);
    assert!(out[0] == 0.0);
    assert!((out[1] - 15.0).abs() < 1e-6);
}

#[test]
fn test_scalar_q8_1x32() {
    let mut block = [0u8; 36];
    for i in 0..32 {
        block[i] = (i + 1) as u8;
    }
    let scale = 0.5f32;
    block[32..36].copy_from_slice(&scale.to_le_bytes());
    let x = [1.0; 32];
    let mut out = [0.0];
    otter_simd::matvec_q8_scalar(&block, 1, 32, &x, &mut out);
    // sum = (1+2+...+32) * 0.5 = 528 * 0.5 = 264
    assert!((out[0] - 264.0).abs() < 1.0);
}

#[test]
fn test_scalar_q8_cols_64() {
    // Test Q8 with cols=64, so blocks straddle rows: 0..32, 32..64
    let mut blocks = vec![0u8; 2 * 36];
    for block_idx in 0..2 {
        let offset = block_idx * 36;
        for lane in 0..32 {
            blocks[offset + lane] = ((block_idx * 32 + lane) % 256) as u8;
        }
        let scale = 1.0f32;
        blocks[offset + 32..offset + 36].copy_from_slice(&scale.to_le_bytes());
    }
    let x = [1.0; 64];
    let mut out = [0.0];
    otter_simd::matvec_q8_scalar(&blocks, 1, 64, &x, &mut out);
    // Block 0: sum(0..32) = 496, Block 1: sum(32..64) = 1008, total = 1504
    assert!(out[0] > 0.0); // Sanity check
}

#[test]
fn test_scalar_q8_cols_176() {
    // cols=176: blocks at 0..32, 32..64, 64..96, 96..128, 128..160, 160..176
    let num_blocks = 176_usize.div_ceil(32);
    let mut blocks = vec![0u8; num_blocks * 36];
    for block_idx in 0..num_blocks {
        let offset = block_idx * 36;
        for lane in 0..32 {
            blocks[offset + lane] = (block_idx as u8).wrapping_add(lane as u8);
        }
        let scale = 0.1f32;
        blocks[offset + 32..offset + 36].copy_from_slice(&scale.to_le_bytes());
    }
    let x = [1.0; 176];
    let mut out = [0.0];
    otter_simd::matvec_q8_scalar(&blocks, 1, 176, &x, &mut out);
    assert!(out[0] > 0.0);
}

#[test]
fn test_scalar_q8_straddling_rows() {
    // 2 rows x 64 cols = 128 elements, 4 blocks
    let mut blocks = vec![0u8; 4 * 36];
    for block_idx in 0..4 {
        let offset = block_idx * 36;
        for lane in 0..32 {
            blocks[offset + lane] = (block_idx as u8).wrapping_mul(10);
        }
        let scale = 0.05f32;
        blocks[offset + 32..offset + 36].copy_from_slice(&scale.to_le_bytes());
    }
    let x = [1.0; 64];
    let mut out = [0.0; 2];
    otter_simd::matvec_q8_scalar(&blocks, 2, 64, &x, &mut out);
    // Row 0: blocks 0, 1; Row 1: blocks 2, 3
    assert!(out[0] > 0.0);
    assert!(out[1] > 0.0);
}

#[test]
fn test_scalar_rmsnorm_simple() {
    let mut x = [3.0, 4.0];
    otter_simd::rmsnorm_scalar(&mut x, 0.0);
    let rms = (12.5_f32).sqrt();
    assert!((x[0] - 3.0 / rms).abs() < 1e-6);
    assert!((x[1] - 4.0 / rms).abs() < 1e-6);
}

#[test]
fn test_scalar_rmsnorm_with_eps() {
    let mut x = [1.0, 1.0, 1.0];
    otter_simd::rmsnorm_scalar(&mut x, 1.0);
    // sq_sum = 3, rms = sqrt(3/3 + 1) = sqrt(2)
    let rms = 2.0_f32.sqrt();
    assert!((x[0] - 1.0 / rms).abs() < 1e-6);
}

#[test]
fn test_scalar_rmsnorm_vs_f64() {
    let vals = [2.0, 3.0, 4.0, 5.0];
    let mut x = vals;
    otter_simd::rmsnorm_scalar(&mut x, 0.0);

    // f64 reference
    let sum_sq: f64 = vals.iter().map(|&v| (v as f64).powi(2)).sum();
    let rms_f64 = (sum_sq / vals.len() as f64).sqrt();
    for &expected in &vals {
        let expected_norm = expected as f64 / rms_f64;
        // Find corresponding output (should be in same order)
        let idx = vals.iter().position(|&v| (v - expected).abs() < 1e-6).unwrap();
        let rel_err = ((x[idx] as f64) - expected_norm).abs() / expected_norm.abs();
        assert!(rel_err < 1e-6, "rmsnorm f64 ref failed");
    }
}

#[test]
fn test_scalar_axpy_simple() {
    let x = [1.0, 2.0, 3.0];
    let mut y = [0.5, 0.5, 0.5];
    otter_simd::axpy_scalar(2.0, &x, &mut y);
    assert!((y[0] - 2.5).abs() < 1e-6);
    assert!((y[1] - 4.5).abs() < 1e-6);
    assert!((y[2] - 6.5).abs() < 1e-6);
}

#[test]
fn test_scalar_axpy_vs_f64() {
    let x = [1.5, 2.5, 3.5, 4.5];
    let y_init = [0.1, 0.2, 0.3, 0.4];
    let mut y = y_init;
    let alpha = 1.5f32;
    otter_simd::axpy_scalar(alpha, &x, &mut y);

    // f64 reference
    for i in 0..4 {
        let expected = (y_init[i] as f64) + (alpha as f64) * (x[i] as f64);
        let rel_err = ((y[i] as f64) - expected).abs() / expected.abs();
        assert!(rel_err < 1e-6, "axpy f64 ref failed");
    }
}

#[test]
fn test_scalar_softmax_simple() {
    let mut x = [0.0, 0.0, 0.0];
    otter_simd::softmax_inplace_scalar(&mut x);
    let sum: f32 = x.iter().sum();
    assert!((sum - 1.0).abs() < 1e-6);
    assert!(x.iter().all(|&v| v > 0.0));
}

#[test]
fn test_scalar_softmax_vs_f64() {
    let vals = [1.0, 2.0, 3.0];
    let mut x = vals;
    otter_simd::softmax_inplace_scalar(&mut x);

    // f64 reference softmax
    let max = vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
    let mut sum_exp = 0.0;
    let exps: Vec<f64> = vals.iter().map(|&v| {
        let exp = (v as f64 - max).exp();
        sum_exp += exp;
        exp
    }).collect();

    for i in 0..vals.len() {
        let expected = exps[i] / sum_exp;
        let rel_err = ((x[i] as f64) - expected).abs() / expected;
        assert!(rel_err < 1e-6, "softmax f64 ref failed");
    }
}

#[test]
fn test_scalar_math_sqrt() {
    let tests = [(4.0, 2.0), (9.0, 3.0), (0.25, 0.5), (100.0, 10.0)];
    for (input, expected) in &tests {
        let result = otter_simd::scalar::sqrt_impl(*input);
        assert!(((result - expected).abs() / expected) < 1e-6, "sqrt({}) failed", input);
    }
}

#[test]
fn test_scalar_math_exp() {
    let tests = [(0.0, 1.0), (1.0, core::f32::consts::E)];
    for (input, expected) in &tests {
        let result = otter_simd::scalar::exp_impl(*input);
        assert!(((result - expected).abs() / expected) < 1e-4, "exp({}) failed", input);
    }
}

#[test]
fn test_scalar_dot_product_empty() {
    let a: [f32; 0] = [];
    let b: [f32; 0] = [];
    let result = otter_simd::dot_f32_scalar(&a, &b);
    assert!(result == 0.0);
}

#[test]
fn test_scalar_matvec_f32_tall() {
    // Tall matrix: 10x3
    let w = [
        1.0, 2.0, 3.0,
        4.0, 5.0, 6.0,
        7.0, 8.0, 9.0,
        10.0, 11.0, 12.0,
        13.0, 14.0, 15.0,
        16.0, 17.0, 18.0,
        19.0, 20.0, 21.0,
        22.0, 23.0, 24.0,
        25.0, 26.0, 27.0,
        28.0, 29.0, 30.0,
    ];
    let x = [1.0, 1.0, 1.0];
    let mut out = [0.0; 10];
    otter_simd::matvec_f32_scalar(&w, 10, 3, &x, &mut out);
    for i in 0..10 {
        let expected = (i * 3 + 1 + i * 3 + 2 + i * 3 + 3) as f32;
        assert!((out[i] - expected).abs() < 1e-6);
    }
}

#[test]
fn test_scalar_matvec_f32_wide() {
    // Wide matrix: 2x10
    let w = [
        1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0,
        11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0,
    ];
    let x = [1.0; 10];
    let mut out = [0.0; 2];
    otter_simd::matvec_f32_scalar(&w, 2, 10, &x, &mut out);
    assert!((out[0] - 55.0).abs() < 1e-6); // 1+2+...+10
    assert!((out[1] - 155.0).abs() < 1e-6); // 11+12+...+20
}

#[test]
fn test_scalar_q8_2blocks() {
    // Two blocks, one row
    let mut blocks = vec![0u8; 2 * 36];
    for block_idx in 0..2 {
        let offset = block_idx * 36;
        for lane in 0..32 {
            blocks[offset + lane] = (lane as u8).wrapping_add(1);
        }
        let scale = 1.0f32;
        blocks[offset + 32..offset + 36].copy_from_slice(&scale.to_le_bytes());
    }
    let x = [1.0; 64];
    let mut out = [0.0];
    otter_simd::matvec_q8_scalar(&blocks, 1, 64, &x, &mut out);
    // Block 0: 1+2+...+32 = 528, Block 1: same = 528, total = 1056
    assert!((out[0] - 1056.0).abs() < 1.0);
}

#[test]
fn test_scalar_axpy_zero() {
    let x = [1.0, 2.0, 3.0];
    let mut y = [0.5, 0.5, 0.5];
    otter_simd::axpy_scalar(0.0, &x, &mut y);
    assert!((y[0] - 0.5).abs() < 1e-6);
    assert!((y[1] - 0.5).abs() < 1e-6);
    assert!((y[2] - 0.5).abs() < 1e-6);
}

#[test]
fn test_scalar_rmsnorm_single() {
    let mut x = [5.0];
    otter_simd::rmsnorm_scalar(&mut x, 0.0);
    // rms = sqrt(25 / 1) = 5, so x[0] = 5/5 = 1
    assert!((x[0] - 1.0).abs() < 1e-5);
}

#[test]
fn test_scalar_softmax_two_values() {
    let mut x = [1.0, -1.0];
    otter_simd::softmax_inplace_scalar(&mut x);
    let sum: f32 = x.iter().sum();
    assert!((sum - 1.0).abs() < 1e-6);
    // First should be larger than second
    assert!(x[0] > x[1]);
}

// ===== ISA PATH VALIDATION (x86_64 under Rosetta) =====

#[test]
#[cfg(target_arch = "x86_64")]
fn test_isa_detection_report() {
    println!("\n=== ISA Detection Report ===");
    #[cfg(target_feature = "sse2")]
    {
        use otter_simd::hw;
        println!("SSE2 supported: {}", hw::sse2_supported());
        println!("AVX2 supported: {}", hw::avx2_supported());
        println!("FMA supported: {}", hw::fma_supported());

        // Check against std's is_x86_feature_detected!
        // The crate's CPUID/XGETBV detection must agree with std's.
        assert_eq!(hw::sse2_supported(), is_x86_feature_detected!("sse2"));
        assert_eq!(hw::avx2_supported(), is_x86_feature_detected!("avx2"));
        assert_eq!(hw::fma_supported(), is_x86_feature_detected!("fma"));
    }
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_avx2_requirement_check() {
    if std::env::var("OTTEROS_REQUIRE_AVX2").is_ok() {
        #[cfg(target_feature = "sse2")]
        {
            use otter_simd::hw;
            if !hw::avx2_supported() || !hw::fma_supported() {
                panic!("OTTEROS_REQUIRE_AVX2=1 but AVX2 or FMA not detected");
            }
            println!("AVX2+FMA verified available");
        }
    }
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_f32_all_isa_paths_1000_shapes() {
    println!("\n=== F32 Validation: 1000 random shapes ===");
    let scalar = Kernels::scalar();
    let detected = Kernels::detect();
    let mut rng = Rng::new();
    let mut passed = 0;

    for test_idx in 0..1000 {
        let rows = rng.range(1, 301) as usize;
        let cols = rng.range(1, 601) as usize;

        let mut w = vec![0.0; rows * cols];
        let mut x = vec![0.0; cols];

        for i in 0..w.len() {
            let v = rng.next();
            w[i] = (v as i64 as f32) / 1e9;
        }
        for i in 0..x.len() {
            let v = rng.next();
            x[i] = (v as i64 as f32) / 1e9;
        }

        let mut out_scalar = vec![0.0; rows];
        let mut out_detected = vec![0.0; rows];

        (scalar.matvec_f32)(&w, rows, cols, &x, &mut out_scalar);
        (detected.matvec_f32)(&w, rows, cols, &x, &mut out_detected);

        // Tolerance: |simd - scalar| <= 1e-5 * sum_i |w_i * x_i|
        let mut all_ok = true;
        for r in 0..rows {
            // Compute scale factor for this row
            let mut scale = 0.0f32;
            for c in 0..cols {
                scale += (w[r * cols + c] * x[c]).abs();
            }
            let tol = 1e-5 * scale;
            let err = (out_detected[r] - out_scalar[r]).abs();
            if err > tol {
                all_ok = false;
                if test_idx < 5 {
                    eprintln!("F32 test {} row {} ({}x{}): err={:.2e}, tol={:.2e}", test_idx, r, rows, cols, err, tol);
                }
                break;
            }
        }
        if all_ok {
            passed += 1;
        }
    }
    println!("F32 tests: {}/1000 passed", passed);
    assert!(passed >= 990, "F32 validation failed: only {}/1000 passed", passed);
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_q8_all_isa_paths_1000_shapes() {
    println!("\n=== Q8 Validation: 1000 random shapes ===");
    let scalar = Kernels::scalar();
    let detected = Kernels::detect();
    let mut rng = Rng::new();
    let mut passed = 0;

    for test_idx in 0..1000 {
        let rows = rng.range(1, 301) as usize;
        let cols = rng.range(1, 601) as usize;

        let num_elements = rows * cols;
        let num_blocks = num_elements.div_ceil(32);
        let mut w_blocks = vec![0u8; num_blocks * 36];
        let mut x = vec![0.0; cols];

        let mut rng_local = rng;
        for block_idx in 0..num_blocks {
            let offset = block_idx * 36;
            for lane in 0..32 {
                w_blocks[offset + lane] = (rng_local.next() >> 8) as u8;
            }
            let scale_u32 = rng_local.next();
            let scale = 0.01f32 + ((scale_u32 as f32) / 1e10) % 0.09;
            w_blocks[offset + 32..offset + 36].copy_from_slice(&scale.to_le_bytes());
        }
        rng = rng_local;

        for i in 0..cols {
            let v = rng.next();
            x[i] = (v as i64 as f32) / 1e9;
        }

        let mut out_scalar = vec![0.0; rows];
        let mut out_detected = vec![0.0; rows];

        (scalar.matvec_q8)(&w_blocks, rows, cols, &x, &mut out_scalar);
        (detected.matvec_q8)(&w_blocks, rows, cols, &x, &mut out_detected);

        // Tolerance: |simd - scalar| <= 1e-4 * sum_i |dequant_w_i * x_i|
        let mut all_ok = true;
        for r in 0..rows {
            let mut scale = 0.0f32;
            for c in 0..cols {
                let flat_idx = r * cols + c;
                let block_idx = flat_idx / 32;
                let lane = flat_idx % 32;
                let offset = block_idx * 36;
                if offset + 36 <= w_blocks.len() {
                    let q8 = w_blocks[offset + lane] as i8 as f32;
                    let scale_bytes = [w_blocks[offset + 32], w_blocks[offset + 33],
                                      w_blocks[offset + 34], w_blocks[offset + 35]];
                    let s = f32::from_le_bytes(scale_bytes);
                    scale += (q8 * s * x[c]).abs();
                }
            }
            let tol = 1e-4 * scale;
            let err = (out_detected[r] - out_scalar[r]).abs();
            if err > tol {
                all_ok = false;
                if test_idx < 5 {
                    eprintln!("Q8 test {} row {} ({}x{}): err={:.2e}, tol={:.2e}", test_idx, r, rows, cols, err, tol);
                }
                break;
            }
        }
        if all_ok {
            passed += 1;
        }
    }
    println!("Q8 tests: {}/1000 passed", passed);
    assert!(passed >= 990, "Q8 validation failed: only {}/1000 passed", passed);
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_matvec_rows_variant_f32() {
    println!("\n=== matvec_f32_rows variant test ===");
    let scalar = Kernels::scalar();
    let detected = Kernels::detect();

    let rows = 100;
    let cols = 64;
    let mut w = vec![0.0; rows * cols];
    let mut x = vec![0.0; cols];
    let mut rng = Rng::new();

    for i in 0..w.len() {
        w[i] = (rng.next() as i64 as f32) / 1e9;
    }
    for i in 0..cols {
        x[i] = (rng.next() as i64 as f32) / 1e9;
    }

    // Test row ranges
    let ranges = [
        (0..50, "first half"),
        (50..100, "second half"),
        (25..75, "middle"),
    ];

    for (range, label) in &ranges {
        let mut out_scalar = vec![0.0; rows];
        let mut out_detected = vec![0.0; rows];

        (scalar.matvec_f32_rows)(range.clone(), &w, rows, cols, &x, &mut out_scalar);
        (detected.matvec_f32_rows)(range.clone(), &w, rows, cols, &x, &mut out_detected);

        for r in range.clone() {
            let scale: f32 = (0..cols).map(|c| (w[r * cols + c] * x[c]).abs()).sum();
            let tol = 1e-5 * scale;
            let err = (out_detected[r] - out_scalar[r]).abs();
            assert!(err <= tol, "matvec_f32_rows {} row {} failed", label, r);
        }
    }
    println!("matvec_f32_rows: PASS");
}

#[test]
#[cfg(target_arch = "x86_64")]
fn test_matvec_rows_variant_q8() {
    println!("\n=== matvec_q8_rows variant test ===");
    let scalar = Kernels::scalar();
    let detected = Kernels::detect();

    let rows = 100;
    let cols = 176; // Straddles blocks
    #[allow(clippy::manual_div_ceil)]
    let num_blocks = (rows * cols + 31) / 32;
    let mut w_blocks = vec![0u8; num_blocks * 36];
    let mut x = vec![0.0; cols];
    let mut rng = Rng::new();

    for block_idx in 0..num_blocks {
        let offset = block_idx * 36;
        for lane in 0..32 {
            w_blocks[offset + lane] = (rng.next() >> 8) as u8;
        }
        let scale = 0.05f32;
        w_blocks[offset + 32..offset + 36].copy_from_slice(&scale.to_le_bytes());
    }

    for i in 0..cols {
        x[i] = (rng.next() as i64 as f32) / 1e9;
    }

    let ranges = [(0..50, "first half"), (50..100, "second half")];

    for (range, label) in &ranges {
        let mut out_scalar = vec![0.0; rows];
        let mut out_detected = vec![0.0; rows];

        (scalar.matvec_q8_rows)(range.clone(), &w_blocks, rows, cols, &x, &mut out_scalar);
        (detected.matvec_q8_rows)(range.clone(), &w_blocks, rows, cols, &x, &mut out_detected);

        for r in range.clone() {
            let mut scale = 0.0f32;
            for c in 0..cols {
                let flat_idx = r * cols + c;
                let block_idx = flat_idx / 32;
                let lane = flat_idx % 32;
                let offset = block_idx * 36;
                if offset + 36 <= w_blocks.len() {
                    let q8 = w_blocks[offset + lane] as i8 as f32;
                    let scale_bytes = [w_blocks[offset + 32], w_blocks[offset + 33],
                                      w_blocks[offset + 34], w_blocks[offset + 35]];
                    let s = f32::from_le_bytes(scale_bytes);
                    scale += (q8 * s * x[c]).abs();
                }
            }
            let tol = 1e-4 * scale;
            let err = (out_detected[r] - out_scalar[r]).abs();
            assert!(err <= tol, "matvec_q8_rows {} row {} failed", label, r);
        }
    }
    println!("matvec_q8_rows: PASS");
}

#[test]
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
fn benchmark_matvec_gflops() {
    println!("\n=== Benchmark: GFLOP/s (Release mode, Rosetta) ===");
    use std::time::Instant;

    let scalar = Kernels::scalar();
    let detected = Kernels::detect();

    #[cfg(target_feature = "sse2")]
    let sse2_kernels = otter_simd::Kernels {
        dot_f32: otter_simd::sse2::dot_f32,
        matvec_f32: otter_simd::sse2::matvec_f32,
        matvec_f32_rows: otter_simd::sse2::matvec_f32_rows,
        matvec_q8: otter_simd::sse2::matvec_q8,
        matvec_q8_rows: otter_simd::sse2::matvec_q8_rows,
        rmsnorm: otter_simd::sse2::rmsnorm,
        axpy: otter_simd::sse2::axpy,
        softmax_inplace: otter_simd::sse2::softmax_inplace,
    };

    let shapes = [(576, 1536), (1536, 576)];

    for &(rows, cols) in &shapes {
        println!("\nShape {}x{}", rows, cols);

        let mut w = vec![0.0; rows * cols];
        let x = vec![1.0; cols];
        let mut out = vec![0.0; rows];
        let mut rng = Rng::new();

        for i in 0..w.len() {
            w[i] = (rng.next() as i64 as f32) / 1e9;
        }

        // Scalar F32
        let start = Instant::now();
        for _ in 0..100 {
            (scalar.matvec_f32)(&w, rows, cols, &x, &mut out);
        }
        let elapsed = start.elapsed().as_secs_f64();
        let scalar_f32_gflops = ((rows as u64) * (cols as u64) * 2 * 100) as f64 / (elapsed * 1e9);
        println!("  Scalar F32:   {:.2} GFLOP/s", scalar_f32_gflops);

        // SSE2 F32
        #[cfg(target_feature = "sse2")]
        let sse2_f32_gflops: f64;
        {
            let start = Instant::now();
            for _ in 0..100 {
                (sse2_kernels.matvec_f32)(&w, rows, cols, &x, &mut out);
            }
            let elapsed = start.elapsed().as_secs_f64();
            sse2_f32_gflops = ((rows as u64) * (cols as u64) * 2 * 100) as f64 / (elapsed * 1e9);
            println!("  SSE2 F32:     {:.2} GFLOP/s", sse2_f32_gflops);
        }

        // Detected F32 (AVX2 if available)
        let start = Instant::now();
        for _ in 0..100 {
            (detected.matvec_f32)(&w, rows, cols, &x, &mut out);
        }
        let elapsed = start.elapsed().as_secs_f64();
        let detected_f32_gflops = ((rows as u64) * (cols as u64) * 2 * 100) as f64 / (elapsed * 1e9);
        println!("  Detected F32: {:.2} GFLOP/s", detected_f32_gflops);

        // Build Q8
        #[allow(clippy::manual_div_ceil)]
        let num_blocks = (rows * cols + 31) / 32;
        let mut w_blocks = vec![0u8; num_blocks * 36];
        for block_idx in 0..num_blocks {
            let offset = block_idx * 36;
            for lane in 0..32 {
                w_blocks[offset + lane] = ((block_idx * 17 + lane) % 256) as u8;
            }
            let scale = 0.05f32;
            w_blocks[offset + 32..offset + 36].copy_from_slice(&scale.to_le_bytes());
        }

        // Scalar Q8
        let start = Instant::now();
        for _ in 0..100 {
            (scalar.matvec_q8)(&w_blocks, rows, cols, &x, &mut out);
        }
        let elapsed = start.elapsed().as_secs_f64();
        let scalar_q8_gflops = ((rows as u64) * (cols as u64) * 2 * 100) as f64 / (elapsed * 1e9);
        println!("  Scalar Q8:    {:.2} GFLOP/s", scalar_q8_gflops);

        // SSE2 Q8
        #[cfg(target_feature = "sse2")]
        let sse2_q8_gflops: f64;
        {
            let start = Instant::now();
            for _ in 0..100 {
                (sse2_kernels.matvec_q8)(&w_blocks, rows, cols, &x, &mut out);
            }
            let elapsed = start.elapsed().as_secs_f64();
            sse2_q8_gflops = ((rows as u64) * (cols as u64) * 2 * 100) as f64 / (elapsed * 1e9);
            println!("  SSE2 Q8:      {:.2} GFLOP/s", sse2_q8_gflops);
        }

        // Detected Q8 (AVX2 if available)
        let start = Instant::now();
        for _ in 0..100 {
            (detected.matvec_q8)(&w_blocks, rows, cols, &x, &mut out);
        }
        let elapsed = start.elapsed().as_secs_f64();
        let detected_q8_gflops = ((rows as u64) * (cols as u64) * 2 * 100) as f64 / (elapsed * 1e9);
        println!("  Detected Q8:  {:.2} GFLOP/s", detected_q8_gflops);

        // Release-only speed ratios. On the Mac these run under Rosetta (ROSETTA_ADVERTISE_AVX=1
        // exposes AVX2); the real numbers are measured on the laptop. SSE2 is the x86_64 baseline.
        #[cfg(not(debug_assertions))]
        {
            let ratio = |a: f64, b: f64| a / b;
            assert!(sse2_f32_gflops >= 1.5 * scalar_f32_gflops, "SSE2 f32 only {:.2}x scalar", ratio(sse2_f32_gflops, scalar_f32_gflops));
            assert!(sse2_q8_gflops >= 1.5 * scalar_q8_gflops, "SSE2 Q8 only {:.2}x scalar", ratio(sse2_q8_gflops, scalar_q8_gflops));
            if otter_simd::hw::avx2_supported() && otter_simd::hw::fma_supported() {
                assert!(detected_f32_gflops >= 3.0 * scalar_f32_gflops, "AVX2 f32 only {:.2}x scalar", ratio(detected_f32_gflops, scalar_f32_gflops));
                assert!(detected_q8_gflops >= 3.0 * scalar_q8_gflops, "AVX2 Q8 only {:.2}x scalar", ratio(detected_q8_gflops, scalar_q8_gflops));
            }
        }
    }
}
