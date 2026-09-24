//! Basic math functions for no_std environments (sqrt, exp, log, etc.).

/// Truncate x to integer (towards zero).
#[inline]
pub fn trunc(x: f32) -> f32 {
    (x as i32) as f32
}

/// Round x to the nearest integer.
#[inline]
pub fn round(x: f32) -> f32 {
    if x >= 0.0 {
        trunc(x + 0.5)
    } else {
        trunc(x - 0.5)
    }
}

/// Compute sqrt(x) using Newton's method.
/// Accurate to ~1e-7 for typical inputs.
#[inline]
pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        0.0
    } else if x == f32::INFINITY {
        f32::INFINITY
    } else {
        // Newton's method: x_{n+1} = (x_n + x/x_n) / 2
        // Start with a reasonable initial guess
        let mut y = (x + 1.0) * 0.5;
        // More iterations for better accuracy
        for _ in 0..10 {
            let y_next = (y + x / y) * 0.5;
            // Check for convergence
            if (y_next - y).abs() < 1e-8 {
                break;
            }
            y = y_next;
        }
        y
    }
}

/// Compute exp(x) using Taylor series approximation.
/// Accurate to ~1e-6 for typical range.
#[inline]
pub fn exp(mut x: f32) -> f32 {
    // Reduce to [-ln(2), ln(2)] for better convergence
    let n = round(x / core::f32::consts::LN_2);
    x -= n * core::f32::consts::LN_2;

    // Compute e^x using Taylor series: 1 + x + x^2/2! + x^3/3! + ...
    let mut result = 1.0;
    let mut term = 1.0;
    for i in 1..=10 {
        term *= x / i as f32;
        result += term;
    }

    // Scale by 2^n
    for _ in 0..n.abs() as i32 {
        if n > 0.0 {
            result *= 2.0;
        } else {
            result *= 0.5;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqrt() {
        assert!((sqrt(4.0) - 2.0).abs() < 1e-5);
        assert!((sqrt(9.0) - 3.0).abs() < 1e-5);
        assert!((sqrt(0.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_exp() {
        assert!((exp(0.0) - 1.0).abs() < 1e-5);
        assert!((exp(1.0) - core::f32::consts::E).abs() < 1e-4);
    }
}
