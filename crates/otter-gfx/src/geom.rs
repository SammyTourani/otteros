//! Shared geometry primitives: points, integer rectangles and 2D affine transforms.

/// A 2D point with `f32` coordinates, used by paths and analytic shape math.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: f32,
    /// Vertical coordinate.
    pub y: f32,
}

impl Point {
    /// Builds a point from `x`/`y`.
    pub const fn new(x: f32, y: f32) -> Self {
        Point { x, y }
    }

    /// Vector addition. Named `add` (not `+`) throughout this crate for symmetry
    /// with `sub`/`scale`/`dot`, so intentionally not `core::ops::Add`.
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, other: Point) -> Point {
        Point::new(self.x + other.x, self.y + other.y)
    }

    /// Vector subtraction (`self - other`).
    #[allow(clippy::should_implement_trait)]
    pub fn sub(self, other: Point) -> Point {
        Point::new(self.x - other.x, self.y - other.y)
    }

    /// Scales both coordinates by `s`.
    pub fn scale(self, s: f32) -> Point {
        Point::new(self.x * s, self.y * s)
    }

    /// Dot product.
    pub fn dot(self, other: Point) -> f32 {
        self.x * other.x + self.y * other.y
    }

    /// Euclidean length.
    pub fn length(self) -> f32 {
        libm_sqrt(self.dot(self))
    }
}

// core has no `sqrt` (that lives in `std`'s libm bindings), and DECISIONS.md D2
// forbids pulling in a math crate for OS logic. A few Newton-Raphson iterations on
// the hardware-free bit trick below are exact enough for anti-aliasing math (which
// only ever feeds an 8-bit coverage value) and keep this crate dependency-free.
pub(crate) fn libm_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x.is_nan() || x.is_infinite() {
        return x;
    }
    // Fast inverse-square-root-style bit hack for an initial guess, refined by
    // Newton-Raphson on f(y) = 1/y^2 - x (avoids a division per iteration), then one
    // more Newton step on the direct sqrt for extra precision.
    let i = x.to_bits();
    let guess = f32::from_bits(0x5f37_5a86_u32.wrapping_sub(i >> 1));
    let mut y = guess;
    y = y * (1.5 - 0.5 * x * y * y);
    y = y * (1.5 - 0.5 * x * y * y);
    y = y * (1.5 - 0.5 * x * y * y);
    let mut r = x * y;
    // Final Newton-Raphson step on r = sqrt(x): r_{n+1} = 0.5*(r_n + x/r_n).
    if r > 0.0 {
        r = 0.5 * (r + x / r);
    }
    r
}

/// `floor`/`ceil`/`round` for `f32`, implemented from scratch: `core` (no `std`)
/// omits them because a general implementation needs libm, but a float-to-int
/// truncating cast (which *is* built into the language) is all rounding to a
/// whole number needs. Named `f*` to avoid clashing with the standard methods
/// (unavailable here, but still name-resolved against during method lookup).
pub(crate) trait FloatExt {
    /// Largest integer `<= self`.
    fn ffloor(self) -> f32;
    /// Smallest integer `>= self`.
    fn fceil(self) -> f32;
    /// Nearest integer to `self`, ties away from zero (matches `std::f32::round`).
    fn fround(self) -> f32;
}

impl FloatExt for f32 {
    fn ffloor(self) -> f32 {
        // `as i32` is a saturating truncation-toward-zero cast (safe, no UB) --
        // exact for every magnitude this crate ever rounds (pixel/font-unit
        // coordinates, always far inside i32 range).
        let truncated = self as i32 as f32;
        if self < 0.0 && truncated != self {
            truncated - 1.0
        } else {
            truncated
        }
    }

    fn fceil(self) -> f32 {
        let truncated = self as i32 as f32;
        if self > 0.0 && truncated != self {
            truncated + 1.0
        } else {
            truncated
        }
    }

    fn fround(self) -> f32 {
        if self >= 0.0 {
            (self + 0.5).ffloor()
        } else {
            (self - 0.5).fceil()
        }
    }
}

/// An axis-aligned integer rectangle (`w`/`h` are extents, not far corners).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width; zero or negative means empty.
    pub w: i32,
    /// Height; zero or negative means empty.
    pub h: i32,
}

impl Rect {
    /// Builds a rectangle from position and size.
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }

    /// True if the rectangle has no area.
    pub fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// Right edge (`x + w`).
    pub fn right(self) -> i32 {
        self.x + self.w
    }

    /// Bottom edge (`y + h`).
    pub fn bottom(self) -> i32 {
        self.y + self.h
    }

    /// The largest rectangle contained in both `self` and `other`; empty (w=h=0) if
    /// they do not overlap.
    pub fn intersect(self, other: Rect) -> Rect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        if x1 <= x0 || y1 <= y0 {
            Rect::new(x0, y0, 0, 0)
        } else {
            Rect::new(x0, y0, x1 - x0, y1 - y0)
        }
    }

    /// True if the point `(px, py)` lies inside the rectangle.
    pub fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.right() && py < self.bottom()
    }
}

/// A 2D affine transform `[a c e; b d f]` applied as `x' = a*x + c*y + e`,
/// `y' = b*x + d*y + f`. Used for composite-glyph transforms and general path
/// transforms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    #[allow(missing_docs)]
    pub a: f32,
    #[allow(missing_docs)]
    pub b: f32,
    #[allow(missing_docs)]
    pub c: f32,
    #[allow(missing_docs)]
    pub d: f32,
    #[allow(missing_docs)]
    pub e: f32,
    #[allow(missing_docs)]
    pub f: f32,
}

impl Affine {
    /// The identity transform.
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// A pure translation.
    pub fn translate(tx: f32, ty: f32) -> Affine {
        Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: tx, f: ty }
    }

    /// A pure (possibly non-uniform) scale about the origin.
    pub fn scale(sx: f32, sy: f32) -> Affine {
        Affine { a: sx, b: 0.0, c: 0.0, d: sy, e: 0.0, f: 0.0 }
    }

    /// Applies the transform to a point.
    pub fn apply(self, p: Point) -> Point {
        Point::new(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f)
    }

    /// Composes `self` then `other` (i.e. `other.apply(self.apply(p))`).
    pub fn then(self, other: Affine) -> Affine {
        Affine {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_matches_known_values() {
        assert!((libm_sqrt(4.0) - 2.0).abs() < 1e-4);
        assert!((libm_sqrt(2.0) - core::f32::consts::SQRT_2).abs() < 1e-4);
        assert_eq!(libm_sqrt(0.0), 0.0);
        assert_eq!(libm_sqrt(-1.0), 0.0);
        assert!((libm_sqrt(65536.0) - 256.0).abs() < 1e-2);
    }

    #[test]
    fn rect_intersect_overlap() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(b), Rect::new(5, 5, 5, 5));
    }

    #[test]
    fn rect_intersect_disjoint_is_empty() {
        let a = Rect::new(0, 0, 5, 5);
        let b = Rect::new(10, 10, 5, 5);
        assert!(a.intersect(b).is_empty());
    }

    #[test]
    fn affine_translate_then_scale() {
        let t = Affine::translate(1.0, 2.0).then(Affine::scale(2.0, 2.0));
        let p = t.apply(Point::new(0.0, 0.0));
        // translate first -> (1,2), then scale -> (2,4).
        assert_eq!(p, Point::new(2.0, 4.0));
    }

    #[test]
    fn affine_identity_is_noop() {
        let p = Point::new(3.5, -2.25);
        assert_eq!(Affine::IDENTITY.apply(p), p);
    }
}
