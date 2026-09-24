//! Accuracy of the no_std math against the host's libm, over a million deterministic samples per
//! function plus edge cases. Targets come from briefs/M7-T2a.md: expf, logf, sinf, cosf <= 2 ulp;
//! powf, tanhf, silu <= 4 ulp; sqrtf exact.

use otter_llm::mathf;

const SAMPLES: usize = 1_000_000;

fn ulp_distance(a: f32, b: f32) -> i64 {
    if a == b {
        return 0;
    }
    if a.is_nan() && b.is_nan() {
        return 0;
    }
    if a.is_nan() || b.is_nan() || a.is_infinite() || b.is_infinite() {
        return i64::MAX;
    }
    // Map the float line onto a monotonic integer line so distances across zero are correct.
    fn ordered(x: f32) -> i64 {
        let bits = x.to_bits() as i32 as i64;
        if bits < 0 { i32::MIN as i64 - bits } else { bits }
    }
    (ordered(a) - ordered(b)).abs()
}

/// Deterministic xorshift64* stream of uniform f64 in [0, 1).
struct Rng(u64);
impl Rng {
    fn next_unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (v >> 11) as f64 / (1u64 << 53) as f64
    }
    fn uniform(&mut self, lo: f64, hi: f64) -> f32 {
        (lo + (hi - lo) * self.next_unit()) as f32
    }
}

fn worst<F: Fn(f32) -> (f32, f32)>(inputs: impl Iterator<Item = f32>, f: F) -> (i64, f32) {
    let mut max = (0i64, 0.0f32);
    for x in inputs {
        let (actual, expected) = f(x);
        let d = ulp_distance(actual, expected);
        if d > max.0 {
            max = (d, x);
        }
    }
    max
}

fn report(name: &str, (ulp, at): (i64, f32), target: i64) {
    eprintln!("mathf test: {name} max ULP error = {ulp} (at x = {at:e}, target <= {target})");
    assert!(ulp <= target, "{name}: {ulp} ulp at x = {at:e} exceeds the target of {target}");
}

#[test]
fn mathf_expf_accuracy() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let inputs = (0..SAMPLES).map(|_| rng.uniform(-87.0, 88.0)).chain([0.0, 1.0, -1.0, 88.0, -87.0]);
    report("expf", worst(inputs, |x| (mathf::expf(x), x.exp())), 2);
    assert!(mathf::expf(f32::NAN).is_nan());
    assert_eq!(mathf::expf(1000.0), f32::INFINITY);
    assert_eq!(mathf::expf(-1000.0), 0.0);
}

#[test]
fn mathf_logf_accuracy() {
    let mut rng = Rng(0xd1b5_4a32_d192_ed03);
    // Log-uniform over the normal f32 range, plus a dense sweep around 1 where log is small.
    let wide = (0..SAMPLES / 2).map(|_| {
        let e = rng.uniform(-120.0, 120.0) as f64;
        (2.0f64).powf(e) as f32
    });
    let mut rng2 = Rng(0x94d0_49bb_1331_11eb);
    let near_one = (0..SAMPLES / 2).map(|_| rng2.uniform(0.5, 2.0));
    let inputs = wide.chain(near_one).chain([1.0, 2.0, 0.5, f32::MIN_POSITIVE, f32::MAX]);
    report("logf", worst(inputs, |x| (mathf::logf(x), x.ln())), 2);
    assert!(mathf::logf(-1.0).is_nan());
    assert_eq!(mathf::logf(0.0), f32::NEG_INFINITY);
}

#[test]
fn mathf_sinf_accuracy() {
    let mut rng = Rng(0xbf58_476d_1ce4_e5b9);
    let inputs = (0..SAMPLES)
        .map(|_| rng.uniform(-1.0e4, 1.0e4))
        .chain([0.0, 1e-8, -1e-8, core::f32::consts::PI]);
    report("sinf", worst(inputs, |x| (mathf::sinf(x), x.sin())), 2);
}

#[test]
fn mathf_cosf_accuracy() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let inputs = (0..SAMPLES).map(|_| rng.uniform(-1.0e4, 1.0e4)).chain([0.0, 1e-8, 1.5707964, -1.5707964]);
    report("cosf", worst(inputs, |x| (mathf::cosf(x), x.cos())), 2);
}

#[test]
fn mathf_sqrtf_exact() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    let inputs = (0..SAMPLES)
        .map(|_| (2.0f64).powf(rng.uniform(-126.0, 127.0) as f64) as f32)
        .chain([0.0, 1.0, 2.0, 4.0, f32::MIN_POSITIVE, f32::MAX]);
    report("sqrtf", worst(inputs, |x| (mathf::sqrtf(x), x.sqrt())), 0);
}

#[test]
fn mathf_powf_accuracy() {
    let mut rng = Rng(0xfeed_face_cafe_beef);
    let mut max = (0i64, 0.0f32);
    for _ in 0..SAMPLES {
        let base = rng.uniform(1e-3, 10.0);
        let exp = rng.uniform(-4.0, 4.0);
        let d = ulp_distance(mathf::powf(base, exp), base.powf(exp));
        if d > max.0 {
            max = (d, base);
        }
    }
    report("powf", max, 4);
    assert_eq!(mathf::powf(-2.0, 3.0), -8.0);
    assert_eq!(mathf::powf(-2.0, 2.0), 4.0);
    assert!(mathf::powf(-2.0, 0.5).is_nan());
    // The RoPE frequency case: theta^(-2i/d).
    for i in 0..32 {
        let e = -(2.0 * i as f32) / 64.0;
        let d = ulp_distance(mathf::powf(10000.0, e), 10000.0f32.powf(e));
        assert!(d <= 4, "rope powf at i = {i}: {d} ulp");
    }
}

#[test]
fn mathf_tanhf_accuracy() {
    let mut rng = Rng(0x1357_9bdf_2468_ace0);
    let inputs = (0..SAMPLES).map(|_| rng.uniform(-10.0, 10.0)).chain([0.0, 1e-5, -1e-5, 9.0, -9.0]);
    report("tanhf", worst(inputs, |x| (mathf::tanhf(x), x.tanh())), 4);
}

#[test]
fn mathf_silu_accuracy() {
    let mut rng = Rng(0x0f1e_2d3c_4b5a_6978);
    // Reference in f64, rounded once, so the check measures mathf rather than an f32 formula.
    let inputs = (0..SAMPLES).map(|_| rng.uniform(-20.0, 20.0)).chain([0.0, 1.0, -1.0]);
    report(
        "silu",
        worst(inputs, |x| {
            let xd = x as f64;
            (mathf::silu(x), (xd / (1.0 + (-xd).exp())) as f32)
        }),
        4,
    );
}

#[test]
fn mathf_softmax() {
    let logits = [1.0, 2.0, 3.0];
    let mut out = [0.0; 3];
    mathf::softmax(&logits, &mut out).unwrap();
    let sum: f32 = out.iter().sum();
    assert!((sum - 1.0).abs() < 1e-6);
    assert!(out[2] > out[1] && out[1] > out[0]);
    // Large inputs must not overflow.
    let big = [1000.0, 1001.0, 999.0];
    let mut out = [0.0; 3];
    mathf::softmax(&big, &mut out).unwrap();
    assert!(out.iter().all(|v| v.is_finite()));
    assert!((out.iter().sum::<f32>() - 1.0).abs() < 1e-6);
}
