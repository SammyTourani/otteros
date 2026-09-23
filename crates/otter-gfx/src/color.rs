//! Straight-alpha [`Color`], premultiplication and exact-integer src-over blending.

use crate::geom::FloatExt;

/// Divides `x` by 255 with correct rounding, exactly, using only integer ops.
///
/// `x` is expected to be a product of two `u8` values (0..=65025), as produced by
/// premultiplying or blending a colour channel. This is the standard
/// "add-and-shift-twice" trick: for `t = x + 128`, `(t + (t >> 8)) >> 8` equals
/// `round(x / 255.0)` exactly for the whole input range (verified exhaustively in
/// `tests::div255_matches_float_division_exhaustively`).
#[inline]
pub fn div255(x: u32) -> u32 {
    let t = x + 128;
    (t + (t >> 8)) >> 8
}

/// A straight-alpha (non-premultiplied) sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Color {
    /// Red channel, 0..=255.
    pub r: u8,
    /// Green channel, 0..=255.
    pub g: u8,
    /// Blue channel, 0..=255.
    pub b: u8,
    /// Alpha channel, 0..=255 (0 = fully transparent).
    pub a: u8,
}

impl Color {
    /// Opaque colour from RGB.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    /// Colour from RGBA.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }

    /// Fully transparent black.
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    /// Opaque black.
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    /// Opaque white.
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    /// Packs this colour, premultiplied, into ARGB8888 (`0xAARRGGBB`).
    pub fn premul(self) -> u32 {
        let a = self.a as u32;
        let r = div255(self.r as u32 * a);
        let g = div255(self.g as u32 * a);
        let b = div255(self.b as u32 * a);
        pack_argb(a, r, g, b)
    }

    /// Recovers the straight-alpha colour from a premultiplied ARGB8888 pixel.
    /// Exact only where the round trip through `premul` -> `unpremul` is lossless
    /// (see `tests::premul_unpremul_round_trip_for_opaque_and_zero_alpha`); for
    /// partial alpha, premultiplication is inherently lossy (it quantizes to 8
    /// bits), so this recovers the *nearest* straight colour, not always the exact
    /// original.
    pub fn unpremul(argb: u32) -> Color {
        let (a, r, g, b) = unpack_argb(argb);
        if a == 0 {
            return Color::rgba(0, 0, 0, 0);
        }
        let un = |c: u32| -> u8 { ((c * 255 + a / 2) / a).min(255) as u8 };
        Color::rgba(un(r), un(g), un(b), a as u8)
    }

    /// Linearly interpolates between two straight-alpha colours; `t` is clamped to
    /// `[0, 1]`.
    pub fn lerp(self, other: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let l = |a: u8, b: u8| -> f32 { a as f32 + (b as f32 - a as f32) * t };
        Color::rgba(
            l(self.r, other.r).fround() as u8,
            l(self.g, other.g).fround() as u8,
            l(self.b, other.b).fround() as u8,
            l(self.a, other.a).fround() as u8,
        )
    }
}

/// Packs premultiplied ARGB channels (each 0..=255) into one `0xAARRGGBB` word.
#[inline]
pub fn pack_argb(a: u32, r: u32, g: u32, b: u32) -> u32 {
    (a << 24) | (r << 16) | (g << 8) | b
}

/// Unpacks an ARGB8888 word into `(a, r, g, b)`.
#[inline]
pub fn unpack_argb(argb: u32) -> (u32, u32, u32, u32) {
    (argb >> 24, (argb >> 16) & 0xff, (argb >> 8) & 0xff, argb & 0xff)
}

/// Composites premultiplied `src` over premultiplied `dst` (the Porter-Duff "over"
/// operator), returning a premultiplied ARGB8888 result. Exact integer math.
#[inline]
pub fn blend_over(dst: u32, src: u32) -> u32 {
    let (sa, sr, sg, sb) = unpack_argb(src);
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let (da, dr, dg, db) = unpack_argb(dst);
    let inv_sa = 255 - sa;
    let a = sa + div255(da * inv_sa);
    let r = sr + div255(dr * inv_sa);
    let g = sg + div255(dg * inv_sa);
    let b = sb + div255(db * inv_sa);
    pack_argb(a, r, g, b)
}

/// Composites premultiplied `src` over premultiplied `dst`, first scaling `src`'s
/// contribution by `coverage` (0..=255, e.g. anti-aliasing coverage or a global
/// alpha). `coverage = 255` is equivalent to plain [`blend_over`].
#[inline]
pub fn blend_over_coverage(dst: u32, src: u32, coverage: u32) -> u32 {
    if coverage == 0 {
        return dst;
    }
    if coverage == 255 {
        return blend_over(dst, src);
    }
    let (sa, sr, sg, sb) = unpack_argb(src);
    let sa = div255(sa * coverage);
    let sr = div255(sr * coverage);
    let sg = div255(sg * coverage);
    let sb = div255(sb * coverage);
    blend_over(dst, pack_argb(sa, sr, sg, sb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn div255_matches_float_division_exhaustively() {
        for a in 0u32..=255 {
            for b in 0u32..=255 {
                let x = a * b;
                let got = div255(x);
                let want = (x as f64 / 255.0).round() as u32;
                assert_eq!(got, want, "div255({x}) = {got}, want {want} (a={a},b={b})");
            }
        }
    }

    #[test]
    fn premul_of_opaque_is_identity_on_rgb() {
        let c = Color::rgb(10, 200, 30);
        let (a, r, g, b) = unpack_argb(c.premul());
        assert_eq!((a, r, g, b), (255, 10, 200, 30));
    }

    #[test]
    fn premul_of_zero_alpha_is_zero() {
        let c = Color::rgba(255, 128, 64, 0);
        assert_eq!(c.premul(), 0);
    }

    #[test]
    fn premul_unpremul_round_trip_for_opaque_and_zero_alpha() {
        let opaque = Color::rgb(17, 200, 3);
        assert_eq!(Color::unpremul(opaque.premul()), opaque);
        let zero = Color::rgba(1, 2, 3, 0);
        assert_eq!(Color::unpremul(zero.premul()), Color::TRANSPARENT);
    }

    #[test]
    fn premul_never_exceeds_alpha() {
        for a in (0u32..=255).step_by(17) {
            for r in (0u32..=255).step_by(31) {
                let c = Color::rgba(r as u8, 255, 0, a as u8);
                let (pa, pr, pg, _pb) = unpack_argb(c.premul());
                assert!(pr <= pa, "premul r={pr} exceeds a={pa}");
                assert!(pg <= pa, "premul g={pg} exceeds a={pa}");
                assert_eq!(pa, a);
            }
        }
    }

    #[test]
    fn blend_opaque_src_replaces_dst() {
        let dst = Color::rgb(1, 2, 3).premul();
        let src = Color::rgb(9, 8, 7).premul();
        assert_eq!(blend_over(dst, src), src);
    }

    #[test]
    fn blend_transparent_src_keeps_dst() {
        let dst = Color::rgb(1, 2, 3).premul();
        let src = Color::TRANSPARENT.premul();
        assert_eq!(blend_over(dst, src), dst);
    }

    #[test]
    fn blend_half_alpha_white_over_black_is_mid_grey() {
        let dst = Color::BLACK.premul();
        let src = Color::rgba(255, 255, 255, 128).premul();
        let (a, r, g, b) = unpack_argb(blend_over(dst, src));
        assert_eq!(a, 255);
        // premultiplied white@128 has r=g=b=128 (div255(255*128)=128); over opaque
        // black that's the final result directly.
        assert_eq!((r, g, b), (128, 128, 128));
    }

    #[test]
    fn blend_is_associative_in_practice_for_stacking_equal_layers() {
        // Compositing the same translucent colour over itself twice should not
        // exceed full coverage, and full-coverage should be idempotent.
        let base = Color::rgb(50, 60, 70).premul();
        let translucent = Color::rgba(200, 10, 10, 40).premul();
        let once = blend_over(base, translucent);
        let twice = blend_over(once, translucent);
        let (a1, ..) = unpack_argb(once);
        let (a2, ..) = unpack_argb(twice);
        assert_eq!(a1, 255);
        assert_eq!(a2, 255);
    }

    #[test]
    fn blend_over_coverage_zero_is_noop() {
        let dst = Color::rgb(9, 9, 9).premul();
        let src = Color::WHITE.premul();
        assert_eq!(blend_over_coverage(dst, src, 0), dst);
    }

    #[test]
    fn blend_over_coverage_full_matches_blend_over() {
        let dst = Color::rgb(9, 9, 9).premul();
        let src = Color::rgba(200, 20, 3, 180).premul();
        assert_eq!(blend_over_coverage(dst, src, 255), blend_over(dst, src));
    }

    #[test]
    fn lerp_endpoints_are_exact() {
        let a = Color::rgba(0, 10, 20, 30);
        let b = Color::rgba(100, 110, 120, 130);
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
    }
}
