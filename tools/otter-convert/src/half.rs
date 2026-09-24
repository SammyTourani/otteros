//! Scalar BF16/F16 -> f32 widening, from scratch (zero dependencies --
//! DECISIONS.md D27). Both are exact, lossless conversions (widening a
//! smaller float format into a strictly larger one never rounds), which is
//! what makes the converter's "f32 export is bit-exact against the
//! safetensors source" self-check meaningful: there is only one correct
//! answer.

/// BF16 is simply the top 16 bits of an f32 (same 8-bit exponent, a
/// truncated 7-bit mantissa instead of 23), so widening is a zero-extending
/// shift.
#[allow(dead_code)]
pub fn bf16_to_f32(bits: u16) -> f32 {
    f32::from_bits(u32::from(bits) << 16)
}

/// IEEE 754 binary16 -> binary32, handling zero, subnormals, normals,
/// infinity and NaN. Standard scalar algorithm (e.g. as used in the `half`
/// crate's portable fallback and Fabian Giesen's public-domain notes on
/// fp16): binary16 subnormals need re-normalizing since binary32's larger
/// exponent range makes them representable as binary32 normals.
#[allow(dead_code)]
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exp = u32::from((bits >> 10) & 0x1F);
    let frac = u32::from(bits & 0x3FF);

    let bits32 = if exp == 0 {
        if frac == 0 {
            sign // +/- zero
        } else {
            // Subnormal half: normalize the mantissa by shifting its leading
            // 1 bit up to bit 10, tracking how many shifts that took so the
            // f32 exponent can be adjusted to compensate.
            let mut mantissa = frac;
            let mut shift = 0u32;
            while mantissa & 0x400 == 0 {
                mantissa <<= 1;
                shift += 1;
            }
            mantissa &= 0x3FF; // drop the now-implicit leading 1
            let exp32 = 127 - 15 - shift + 1;
            sign | (exp32 << 23) | (mantissa << 13)
        }
    } else if exp == 0x1F {
        // Infinity (frac == 0) or NaN (frac != 0); f32's exponent field is
        // 8 bits of all-ones (0xFF) either way, mantissa widened the same
        // way as the normal case below.
        sign | (0xFFu32 << 23) | (frac << 13)
    } else {
        let exp32 = exp + (127 - 15);
        sign | (exp32 << 23) | (frac << 13)
    };
    f32::from_bits(bits32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bf16_known_values() {
        assert_eq!(bf16_to_f32(0x3F80), 1.0); // 1.0
        assert_eq!(bf16_to_f32(0xBF80), -1.0);
        assert_eq!(bf16_to_f32(0x0000), 0.0);
        assert_eq!(bf16_to_f32(0x8000).to_bits(), (-0.0f32).to_bits());
        assert_eq!(bf16_to_f32(0x4049), 3.140625); // pi truncated to bf16
    }

    #[test]
    fn f16_known_values() {
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xBC00), -1.0);
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x8000).to_bits(), (-0.0f32).to_bits());
        assert_eq!(f16_to_f32(0x4000), 2.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        // Smallest positive subnormal: 2^-24.
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        // Largest subnormal: (1023/1024) * 2^-14.
        assert_eq!(f16_to_f32(0x03FF), (1023.0 / 1024.0) * 2f32.powi(-14));
        assert!(f16_to_f32(0x7C00).is_infinite() && f16_to_f32(0x7C00) > 0.0);
        assert!(f16_to_f32(0xFC00).is_infinite() && f16_to_f32(0xFC00) < 0.0);
        assert!(f16_to_f32(0x7E00).is_nan());
        // 65504.0 is f16::MAX.
        assert_eq!(f16_to_f32(0x7BFF), 65504.0);
    }
}
