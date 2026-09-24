#![allow(clippy::excessive_precision)]
#![allow(clippy::approx_constant)]
//! no_std float math: compute in f64, round to f32 once.

const LN2_HI: f64 = 6.93147180369123816490e-01;
const LN2_LO: f64 = 1.90821492927058770002e-10;
const PIO2_HI: f64 = 1.57079632673412561417e+00;
const PIO2_LO: f64 = 6.07710050650619224932e-11;
const SQRT_2: f64 = 1.41421356237309504880;

fn trunc_f64(x: f64) -> f64 { (x as i64) as f64 }
fn fract_f64(x: f64) -> f64 { x - trunc_f64(x) }
fn round_f64(x: f64) -> f64 {
    if x >= 0.0 { trunc_f64(x + 0.5) } else { trunc_f64(x - 0.5) }
}

fn exp64(x: f64) -> f64 {
    let k = round_f64(x / LN2_HI);
    let r = x - k * LN2_HI - k * LN2_LO;
    let r2 = r * r;
    let p = 1.0 + r + 0.5*r2 + r*r2/6.0 + r2*r2/24.0 + r*r2*r2/120.0 + 
        r2*r2*r2/720.0 + r*r2*r2*r2/5040.0 + r2*r2*r2*r2/40320.0 + 
        r*r2*r2*r2*r2/362880.0 + r2*r2*r2*r2*r2/3628800.0;
    
    if k > 1024.0 { return f64::INFINITY; }
    if k < -1074.0 { return 0.0; }
    
    let ki = k as i32;
    let exp_bits = ((ki + 1023) as u64) << 52;
    f64::from_bits(exp_bits) * p
}

fn log64(x: f64) -> f64 {
    if x <= 0.0 { return if x == 0.0 { f64::NEG_INFINITY } else { f64::NAN }; }
    
    // x = m * 2^e with m in [1, 2) (exponent bits 0x3ff), then fold m into [sqrt(2)/2, sqrt(2)].
    // Inputs come from f32 or from exp/pow internals, so f64 subnormals do not occur here.
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7ff) as i64 - 1023;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);

    if m > SQRT_2 { m *= 0.5; e += 1; }

    // log(m) = 2 * atanh(s), s = (m - 1) / (m + 1), |s| <= 0.1716: the odd series through s^19
    // leaves a truncation error below 1e-16 relative.
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let mut poly = 1.0 / 19.0;
    for d in [17.0, 15.0, 13.0, 11.0, 9.0, 7.0, 5.0, 3.0, 1.0] {
        poly = poly * s2 + 1.0 / d;
    }
    let log_m = 2.0 * s * poly;

    let ef = e as f64;
    ef * LN2_HI + (ef * LN2_LO + log_m)
}

/// sin(r) and cos(r) for |r| <= pi/4 (plus a little slack) by Taylor series in Horner form:
/// sin through r^17, cos through r^18, truncation error below 1e-17 on that range.
fn sincos_reduced(r: f64) -> (f64, f64) {
    let r2 = r * r;
    // sin: r * (1 - r^2/3! + r^4/5! - ...), coefficients (-1)^n / (2n+1)!
    let mut s = 1.0 / 355_687_428_096_000.0; // 1/17!
    for c in [
        -1.0 / 1_307_674_368_000.0, // 1/15!
        1.0 / 6_227_020_800.0,      // 1/13!
        -1.0 / 39_916_800.0,        // 1/11!
        1.0 / 362_880.0,            // 1/9!
        -1.0 / 5_040.0,             // 1/7!
        1.0 / 120.0,                // 1/5!
        -1.0 / 6.0,                 // 1/3!
        1.0,
    ] {
        s = s * r2 + c;
    }
    // cos: 1 - r^2/2! + r^4/4! - ..., coefficients (-1)^n / (2n)!
    let mut c = 1.0 / 6_402_373_705_728_000.0; // 1/18!
    for k in [
        -1.0 / 20_922_789_888_000.0, // 1/16!
        1.0 / 87_178_291_200.0,      // 1/14!
        -1.0 / 479_001_600.0,        // 1/12!
        1.0 / 3_628_800.0,           // 1/10!
        -1.0 / 40_320.0,             // 1/8!
        1.0 / 720.0,                 // 1/6!
        -1.0 / 24.0,                 // 1/4!
        1.0 / 2.0,                   // 1/2! (subtracted below)
    ] {
        c = c * r2 + k;
    }
    (r * s, 1.0 - r2 * c)
}

/// Reduce x by multiples of pi/2: returns (quadrant, r) with x = k*pi/2 + r, |r| <= ~pi/4.
/// PIO2_HI has trailing zero bits, so k * PIO2_HI is exact for |k| < 2^20 (|x| up to ~1.6e6).
fn reduce_pio2(x: f64) -> (i64, f64) {
    let k = round_f64(x / PIO2_HI);
    let r = (x - k * PIO2_HI) - k * PIO2_LO;
    ((k as i64) & 3, r)
}

fn sqrt_f64(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 { return f64::NAN; }
    if x == 0.0 || x.is_infinite() { return x; }
    // Halving the biased exponent gives a start within a factor of two of the root; Newton's
    // method then doubles the correct bits each step, so six steps exceed f64 precision.
    let bits = x.to_bits();
    let mut guess = f64::from_bits((bits >> 1) + (0x3ff_u64 << 51));
    for _ in 0..6 {
        guess = 0.5 * (guess + x / guess);
    }
    guess
}

pub fn expf(x: f32) -> f32 {
    let xd = x as f64;
    if xd.is_nan() { return f32::NAN; }
    if xd > 88.7228 { return f32::INFINITY; }
    if xd < -103.97 { return 0.0; }
    exp64(xd) as f32
}

pub fn logf(x: f32) -> f32 {
    let xd = x as f64;
    if xd < 0.0 || xd.is_nan() { return f32::NAN; }
    if xd == 0.0 { return f32::NEG_INFINITY; }
    if xd.is_infinite() { return f32::INFINITY; }
    log64(xd) as f32
}

/// Accurate to within the f32 rounding of an f64 result for |x| <= ~1.6e6 (see `reduce_pio2`).
pub fn sinf(x: f32) -> f32 {
    if !x.is_finite() { return f32::NAN; }
    let (q, r) = reduce_pio2(x as f64);
    let (s, c) = sincos_reduced(r);
    let res = match q { 0 => s, 1 => c, 2 => -s, _ => -c };
    res as f32
}

/// Accurate to within the f32 rounding of an f64 result for |x| <= ~1.6e6 (see `reduce_pio2`).
pub fn cosf(x: f32) -> f32 {
    if !x.is_finite() { return f32::NAN; }
    let (q, r) = reduce_pio2(x as f64);
    let (s, c) = sincos_reduced(r);
    let res = match q { 0 => c, 1 => -s, 2 => -c, _ => s };
    res as f32
}

pub fn sqrtf(x: f32) -> f32 {
    sqrt_f64(x as f64) as f32
}

pub fn powf(x: f32, y: f32) -> f32 {
    let xd = x as f64;
    let yd = y as f64;
    if xd == 1.0 { return 1.0; }
    if yd == 0.0 { return 1.0; }
    if xd == 0.0 { return if yd > 0.0 { 0.0 } else { f32::INFINITY }; }
    let frac = yd - trunc_f64(yd);
    if xd < 0.0 && frac != 0.0 { return f32::NAN; }
    let magnitude = exp64(yd * log64(xd.abs()));
    // A negative base with an odd integer exponent keeps its sign: (-2)^3 = -8.
    let odd_integer = frac == 0.0 && yd.abs() < 9.0e15 && (trunc_f64(yd) as i64) & 1 == 1;
    (if xd < 0.0 && odd_integer { -magnitude } else { magnitude }) as f32
}

pub fn tanhf(x: f32) -> f32 {
    let xd = x as f64;
    if xd.abs() > 9.0 { x.signum() } else if xd.abs() < 1e-4 { x } else {
        let e2x = exp64(2.0 * xd);
        ((e2x - 1.0) / (e2x + 1.0)) as f32
    }
}

pub fn silu(x: f32) -> f32 {
    let xd = x as f64;
    let sig = 1.0 / (1.0 + exp64(-xd));
    (xd * sig) as f32
}

pub fn softmax(logits: &[f32], out: &mut [f32]) -> Result<(), &'static str> {
    if logits.len() != out.len() { return Err("len"); }
    if logits.is_empty() { return Ok(()); }
    
    let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0_f64;
    for (i, &l) in logits.iter().enumerate() {
        out[i] = expf(l - max_logit);
        sum += out[i] as f64;
    }
    
    if sum > 0.0 {
        let norm = 1.0 / sum;
        for v in out.iter_mut() { *v = (*v as f64 * norm) as f32; }
    }
    Ok(())
}
