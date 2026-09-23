//! Analytic 2D shapes: exact-coverage axis-aligned rects, signed-distance-field
//! (SDF) rounded rects/circles/lines with supersampled antialiasing, and blits.
//!
//! Rounded corners, circles and lines all antialias the same way: a per-shape
//! signed distance function (negative inside) is sampled on a 4x4 grid of
//! subpixel positions per pixel, and coverage is the fraction of samples inside.
//! This converges to the true analytic area as the corner radius grows relative to
//! a pixel (verified by the area/symmetry tests below), and treats fills, strokes,
//! and round-capped lines uniformly.

use crate::color::{blend_over, pack_argb, unpack_argb, Color};
use crate::geom::{libm_sqrt, FloatExt, Point};
use crate::surface::{Canvas, Surface};

const SS: i32 = 4; // 4x4 = 16 subpixel samples per pixel.

fn to_u8(x: f32) -> u32 {
    (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u32
}

/// Fraction (0..=255) of a `SS x SS` subpixel grid inside pixel `(px,py)` for which
/// `inside` returns true.
pub(crate) fn supersample_coverage(px: i32, py: i32, inside: impl Fn(f32, f32) -> bool) -> u32 {
    let mut count = 0i32;
    for sy in 0..SS {
        let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
        for sx in 0..SS {
            let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
            if inside(x, y) {
                count += 1;
            }
        }
    }
    ((count * 255 + (SS * SS) / 2) / (SS * SS)) as u32
}

/// Signed distance from `p` to a rounded box centred at `center` with half-extents
/// `half` and corner radius `r` (negative inside). Standard analytic rounded-box
/// SDF (degenerates to a circle SDF when `half.x == half.y == r`, and to a plain
/// box SDF when `r == 0`).
pub(crate) fn sd_round_box(p: Point, center: Point, half: Point, r: f32) -> f32 {
    let qx = (p.x - center.x).abs() - (half.x - r);
    let qy = (p.y - center.y).abs() - (half.y - r);
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    libm_sqrt(ax * ax + ay * ay) + qx.max(qy).min(0.0) - r
}

/// Distance from `p` to the segment `a`-`b` (not the infinite line).
fn sd_segment(p: Point, a: Point, b: Point) -> f32 {
    let pa = p.sub(a);
    let ba = b.sub(a);
    let denom = ba.dot(ba).max(1e-12);
    let h = (pa.dot(ba) / denom).clamp(0.0, 1.0);
    pa.sub(ba.scale(h)).length()
}

/// Overlap length of `[cell0, cell1)` and `[r0, r1)`, clamped to `>= 0`.
fn overlap_1d(cell0: f32, cell1: f32, r0: f32, r1: f32) -> f32 {
    (cell1.min(r1) - cell0.max(r0)).max(0.0)
}

/// Fills an axis-aligned rectangle with exact per-pixel coverage (the overlap area
/// between each pixel's unit square and the rectangle -- no approximation needed
/// since both are axis-aligned).
pub fn fill_rect(canvas: &mut Canvas, x: f32, y: f32, w: f32, h: f32, color: Color) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let premul = color.premul();
    let x0 = x.ffloor() as i32;
    let x1 = (x + w).fceil() as i32;
    let y0 = y.ffloor() as i32;
    let y1 = (y + h).fceil() as i32;
    for py in y0..y1 {
        let cov_y = overlap_1d(py as f32, py as f32 + 1.0, y, y + h);
        if cov_y <= 0.0 {
            continue;
        }
        for px in x0..x1 {
            let cov_x = overlap_1d(px as f32, px as f32 + 1.0, x, x + w);
            let cov = cov_x * cov_y;
            if cov <= 0.0 {
                continue;
            }
            canvas.blend_pixel_coverage(px, py, premul, to_u8(cov));
        }
    }
}

/// Fills a rounded rectangle (corner radius clamped to half the shorter side).
pub fn fill_rounded_rect(canvas: &mut Canvas, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let r = radius.clamp(0.0, w.min(h) * 0.5);
    let center = Point::new(x + w * 0.5, y + h * 0.5);
    let half = Point::new(w * 0.5, h * 0.5);
    let premul = color.premul();
    let x0 = (x - 1.0).ffloor() as i32;
    let x1 = (x + w + 1.0).fceil() as i32;
    let y0 = (y - 1.0).ffloor() as i32;
    let y1 = (y + h + 1.0).fceil() as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            let cov = supersample_coverage(px, py, |sx, sy| sd_round_box(Point::new(sx, sy), center, half, r) <= 0.0);
            if cov == 0 {
                continue;
            }
            canvas.blend_pixel_coverage(px, py, premul, cov);
        }
    }
}

/// Strokes a rounded rectangle's outline, centred on the path (half inside, half
/// outside), with a given line `width`.
#[allow(clippy::too_many_arguments)]
pub fn stroke_rounded_rect(canvas: &mut Canvas, x: f32, y: f32, w: f32, h: f32, radius: f32, width: f32, color: Color) {
    if w <= 0.0 || h <= 0.0 || width <= 0.0 {
        return;
    }
    let r = radius.clamp(0.0, w.min(h) * 0.5);
    let hw = width * 0.5;
    let center = Point::new(x + w * 0.5, y + h * 0.5);
    let half = Point::new(w * 0.5, h * 0.5);
    let premul = color.premul();
    let x0 = (x - hw - 1.0).ffloor() as i32;
    let x1 = (x + w + hw + 1.0).fceil() as i32;
    let y0 = (y - hw - 1.0).ffloor() as i32;
    let y1 = (y + h + hw + 1.0).fceil() as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            let cov = supersample_coverage(px, py, |sx, sy| {
                sd_round_box(Point::new(sx, sy), center, half, r).abs() <= hw
            });
            if cov == 0 {
                continue;
            }
            canvas.blend_pixel_coverage(px, py, premul, cov);
        }
    }
}

/// Fills a circle.
pub fn fill_circle(canvas: &mut Canvas, cx: f32, cy: f32, r: f32, color: Color) {
    if r <= 0.0 {
        return;
    }
    let premul = color.premul();
    let x0 = (cx - r - 1.0).ffloor() as i32;
    let x1 = (cx + r + 1.0).fceil() as i32;
    let y0 = (cy - r - 1.0).ffloor() as i32;
    let y1 = (cy + r + 1.0).fceil() as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            let cov = supersample_coverage(px, py, |sx, sy| {
                let dx = sx - cx;
                let dy = sy - cy;
                dx * dx + dy * dy <= r * r
            });
            if cov == 0 {
                continue;
            }
            canvas.blend_pixel_coverage(px, py, premul, cov);
        }
    }
}

/// Draws an anti-aliased, round-capped line segment of a given `thickness`.
pub fn stroke_line(canvas: &mut Canvas, x0: f32, y0: f32, x1: f32, y1: f32, thickness: f32, color: Color) {
    if thickness <= 0.0 {
        return;
    }
    let hw = thickness * 0.5;
    let a = Point::new(x0, y0);
    let b = Point::new(x1, y1);
    let premul = color.premul();
    let bx0 = (x0.min(x1) - hw - 1.0).ffloor() as i32;
    let bx1 = (x0.max(x1) + hw + 1.0).fceil() as i32;
    let by0 = (y0.min(y1) - hw - 1.0).ffloor() as i32;
    let by1 = (y0.max(y1) + hw + 1.0).fceil() as i32;
    for py in by0..by1 {
        for px in bx0..bx1 {
            let cov = supersample_coverage(px, py, |sx, sy| sd_segment(Point::new(sx, sy), a, b) <= hw);
            if cov == 0 {
                continue;
            }
            canvas.blend_pixel_coverage(px, py, premul, cov);
        }
    }
}

/// Blits `src` onto `dst` at `(dst_x, dst_y)` at 1:1 scale, alpha-blended and
/// additionally scaled by `global_alpha` (255 = opaque, use `src`'s alpha as-is).
pub fn blit(dst: &mut Canvas, src: &Surface, dst_x: i32, dst_y: i32, global_alpha: u8) {
    for y in 0..src.h as i32 {
        for x in 0..src.w as i32 {
            let s = src.get(x, y);
            if s == 0 {
                continue;
            }
            let s = scale_argb(s, global_alpha);
            dst.blend_pixel(dst_x + x, dst_y + y, s);
        }
    }
}

fn scale_argb(argb: u32, alpha: u8) -> u32 {
    if alpha == 255 {
        return argb;
    }
    let ga = alpha as u32;
    let (a, r, g, b) = unpack_argb(argb);
    pack_argb(crate::color::div255(a * ga), crate::color::div255(r * ga), crate::color::div255(g * ga), crate::color::div255(b * ga))
}

/// Blits `src`, resampled with bilinear filtering, into the destination rectangle
/// `(dx, dy, dw, dh)` (which may differ in size from `src`).
pub fn blit_bilinear(dst: &mut Canvas, src: &Surface, dx: f32, dy: f32, dw: f32, dh: f32) {
    if dw <= 0.0 || dh <= 0.0 || src.w == 0 || src.h == 0 {
        return;
    }
    let sx_scale = src.w as f32 / dw;
    let sy_scale = src.h as f32 / dh;
    let x0 = dx.ffloor() as i32;
    let x1 = (dx + dw).fceil() as i32;
    let y0 = dy.ffloor() as i32;
    let y1 = (dy + dh).fceil() as i32;
    for py in y0..y1 {
        let v = ((py as f32 + 0.5) - dy) * sy_scale - 0.5;
        for px in x0..x1 {
            let u = ((px as f32 + 0.5) - dx) * sx_scale - 0.5;
            let color = sample_bilinear(src, u, v);
            if color == 0 {
                continue;
            }
            dst.blend_pixel(px, py, color);
        }
    }
}

fn sample_bilinear(src: &Surface, u: f32, v: f32) -> u32 {
    let u = u.clamp(0.0, src.w as f32 - 1.0);
    let v = v.clamp(0.0, src.h as f32 - 1.0);
    let x0 = u.ffloor() as i32;
    let y0 = v.ffloor() as i32;
    let x1 = (x0 + 1).min(src.w as i32 - 1);
    let y1 = (y0 + 1).min(src.h as i32 - 1);
    let fx = u - x0 as f32;
    let fy = v - y0 as f32;
    let c00 = src.get(x0, y0);
    let c10 = src.get(x1, y0);
    let c01 = src.get(x0, y1);
    let c11 = src.get(x1, y1);
    lerp_argb(lerp_argb(c00, c10, fx), lerp_argb(c01, c11, fx), fy)
}

fn lerp_argb(a: u32, b: u32, t: f32) -> u32 {
    let (aa, ar, ag, ab) = unpack_argb(a);
    let (ba, br, bg, bb) = unpack_argb(b);
    let l = |x: u32, y: u32| -> u32 { (x as f32 + (y as f32 - x as f32) * t).fround() as u32 };
    pack_argb(l(aa, ba), l(ar, br), l(ag, bg), l(ab, bb))
}

/// Re-exported for callers that want to layer their own compositing on `blit`'s
/// result (e.g. drawing a border on top).
pub fn over(dst: u32, src: u32) -> u32 {
    blend_over(dst, src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::Surface;

    fn total_alpha(s: &Surface) -> f64 {
        s.px.iter().map(|&p| (p >> 24) as f64 / 255.0).sum()
    }

    #[test]
    fn fill_rect_area_is_exact() {
        let mut s = Surface::new(20, 20);
        {
            let mut c = s.canvas();
            fill_rect(&mut c, 2.5, 3.5, 4.25, 5.0, Color::WHITE);
        }
        let area = total_alpha(&s);
        let want = 4.25 * 5.0;
        // Exact in continuous coverage; summing quantized 8-bit alpha (see the
        // analogous note in raster.rs) leaves ~1% rounding noise, not exactness.
        assert!((area - want).abs() / want < 0.01, "area={area}, want ~{want}");
    }

    #[test]
    fn fill_rect_pixel_aligned_is_binary() {
        let mut s = Surface::new(10, 10);
        {
            let mut c = s.canvas();
            fill_rect(&mut c, 2.0, 2.0, 3.0, 3.0, Color::WHITE);
        }
        assert_eq!(s.get(2, 2), Color::WHITE.premul());
        assert_eq!(s.get(1, 1), 0);
        assert_eq!(s.get(4, 4), Color::WHITE.premul());
        assert_eq!(s.get(5, 5), 0);
    }

    #[test]
    fn rounded_rect_area_within_one_percent_of_analytic() {
        let mut s = Surface::new(60, 60);
        let (w, h, r) = (40.0f32, 30.0f32, 8.0f32);
        {
            let mut c = s.canvas();
            fill_rounded_rect(&mut c, 5.0, 10.0, w, h, r, Color::WHITE);
        }
        let area = total_alpha(&s);
        let analytic = (w * h) as f64 - (4.0 - core::f32::consts::PI) as f64 * (r * r) as f64;
        assert!((area - analytic).abs() / analytic < 0.01, "area={area}, want ~{analytic}");
    }

    #[test]
    fn circle_area_within_one_percent_of_analytic() {
        let mut s = Surface::new(50, 50);
        let r = 15.0f32;
        {
            let mut c = s.canvas();
            fill_circle(&mut c, 25.0, 25.0, r, Color::WHITE);
        }
        let area = total_alpha(&s);
        let analytic = (core::f32::consts::PI * r * r) as f64;
        assert!((area - analytic).abs() / analytic < 0.01, "area={area}, want ~{analytic}");
    }

    #[test]
    fn rounded_rect_corners_are_symmetric() {
        let mut s = Surface::new(40, 40);
        let (x, y, w, h, r) = (5.0f32, 5.0f32, 30.0f32, 30.0f32, 10.0f32);
        {
            let mut c = s.canvas();
            fill_rounded_rect(&mut c, x, y, w, h, r, Color::WHITE);
        }
        // Symmetric shape (square, centred): every quadrant's coverage pattern
        // should mirror the others exactly.
        let cov = |px: i32, py: i32| -> u32 { s.get(px, py) >> 24 };
        for dy in 0..15i32 {
            for dx in 0..15i32 {
                let tl = cov(5 + dx, 5 + dy);
                let tr = cov(5 + 29 - dx, 5 + dy);
                let bl = cov(5 + dx, 5 + 29 - dy);
                let br = cov(5 + 29 - dx, 5 + 29 - dy);
                assert_eq!(tl, tr, "dx={dx} dy={dy}");
                assert_eq!(tl, bl, "dx={dx} dy={dy}");
                assert_eq!(tl, br, "dx={dx} dy={dy}");
            }
        }
    }

    #[test]
    fn circle_symmetry_across_both_axes() {
        // Centred on a pixel centre (15.5, not 15.0, which is a pixel *boundary*)
        // so that pixel index `15+k` is the exact mirror of pixel index `15-k`.
        let mut s = Surface::new(30, 30);
        {
            let mut c = s.canvas();
            fill_circle(&mut c, 15.5, 15.5, 10.0, Color::WHITE);
        }
        let cov = |px: i32, py: i32| -> u32 { s.get(px, py) >> 24 };
        for dx in 0..10i32 {
            for dy in 0..10i32 {
                assert_eq!(cov(15 + dx, 15 + dy), cov(15 - dx, 15 + dy));
                assert_eq!(cov(15 + dx, 15 + dy), cov(15 + dx, 15 - dy));
            }
        }
    }

    #[test]
    fn stroke_rounded_rect_is_hollow() {
        let mut s = Surface::new(40, 40);
        {
            let mut c = s.canvas();
            stroke_rounded_rect(&mut c, 5.0, 5.0, 30.0, 30.0, 8.0, 3.0, Color::WHITE);
        }
        assert_eq!(s.get(20, 20), 0, "centre of a stroked shape must stay empty");
        assert_eq!(s.get(5, 20), Color::WHITE.premul(), "left edge mid-height is on the stroke");
    }

    #[test]
    fn stroke_line_horizontal_has_expected_thickness() {
        let mut s = Surface::new(20, 10);
        {
            let mut c = s.canvas();
            stroke_line(&mut c, 2.0, 5.0, 18.0, 5.0, 4.0, Color::WHITE);
        }
        // Column through the middle of the line: coverage should sum to ~4.0 (the
        // requested thickness) down that column.
        let sum: f64 = (0..10).map(|y| (s.get(10, y) >> 24) as f64 / 255.0).sum();
        assert!((sum - 4.0).abs() < 0.15, "sum={sum}");
    }

    #[test]
    fn blit_copies_pixels_with_global_alpha() {
        let mut src = Surface::new(2, 2);
        {
            let mut c = src.canvas();
            fill_rect(&mut c, 0.0, 0.0, 2.0, 2.0, Color::WHITE);
        }
        let mut dst = Surface::new(4, 4);
        {
            let mut c = dst.canvas();
            blit(&mut c, &src, 1, 1, 128);
        }
        let (a, r, _g, _b) = unpack_argb(dst.get(1, 1));
        assert_eq!(a, 128);
        assert_eq!(r, 128);
    }

    #[test]
    fn blit_bilinear_upscale_is_smooth_not_nearest() {
        // A 2x1 black/white source, upscaled to 4 wide: the two centre destination
        // pixels should land strictly between black and white (proof it isn't
        // doing nearest-neighbour).
        let mut src = Surface::new(2, 1);
        src.px[0] = Color::BLACK.premul();
        src.px[1] = Color::WHITE.premul();
        let mut dst = Surface::new(4, 1);
        {
            let mut c = dst.canvas();
            blit_bilinear(&mut c, &src, 0.0, 0.0, 4.0, 1.0);
        }
        let (_a, r1, ..) = unpack_argb(dst.get(1, 0));
        let (_a2, r2, ..) = unpack_argb(dst.get(2, 0));
        assert!(r1 > 0 && r1 < 255, "r1={r1}");
        assert!(r2 > 0 && r2 < 255, "r2={r2}");
    }
}
