//! Non-zero winding scanline rasterizer using signed-area accumulation, and glue to
//! fill a [`Path`] onto a [`Canvas`].
//!
//! # Algorithm
//! For each edge, split it at every integer scanline (row) it crosses, and within
//! each row, walk the pixel columns it crosses. Each per-cell fragment contributes
//! two deltas to a `width * height` accumulator: the exact trapezoidal area to the
//! *right* of the edge fragment within its own cell (an exact result, since the
//! edge is a straight line inside one cell, so the area is the average of the two
//! boundary depths times the fragment's `dy`), and the remaining `dy` "carried" to
//! the next cell so that a left-to-right prefix sum over a row reconstructs the
//! winding-weighted coverage at every pixel. Horizontal edges (`dy == 0`) never
//! contribute (as in any edge-crossing scan conversion) -- only the edges that
//! actually change winding as `y` increases matter. Summing this over every edge of
//! every subpath and clamping `|prefix sum|` to `[0, 1]` per pixel gives exactly the
//! non-zero fill rule's antialiased coverage.
//!
//! Buffers are always sized to fully contain the path being rasterized (see
//! [`rasterize`]); [`Rasterizer::add_line`] clamps x to `[0, w]`, which is only
//! exact under that invariant (an edge genuinely outside the buffer on the right
//! would undercount coverage -- irrelevant here since nothing calls it that way).

use alloc::vec;
use alloc::vec::Vec;

use crate::color::Color;
use crate::geom::{Affine, FloatExt, Point};
use crate::path::Path;
use crate::surface::Canvas;

/// Accumulates signed edge coverage over a fixed-size pixel grid.
pub struct Rasterizer {
    w: usize,
    h: usize,
    acc: Vec<f32>,
}

impl Rasterizer {
    /// A rasterizer for a `w` x `h` pixel grid, initially empty (zero coverage).
    pub fn new(w: usize, h: usize) -> Self {
        Rasterizer { w, h, acc: vec![0.0f32; w * h] }
    }

    /// Adds one edge, in the rasterizer's own pixel coordinates.
    pub fn add_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let (mut x0, mut y0, mut x1, mut y1) = (x0, y0, x1, y1);
        if y0 == y1 || self.h == 0 || self.w == 0 {
            return;
        }
        let dir: f32 = if y0 < y1 { 1.0 } else { -1.0 };
        if y0 > y1 {
            core::mem::swap(&mut x0, &mut x1);
            core::mem::swap(&mut y0, &mut y1);
        }
        let h = self.h as f32;
        if y1 <= 0.0 || y0 >= h {
            return;
        }
        let dxdy = (x1 - x0) / (y1 - y0);
        if y0 < 0.0 {
            x0 += (0.0 - y0) * dxdy;
            y0 = 0.0;
        }
        if y1 > h {
            y1 = h;
        }
        if y0 >= y1 {
            return;
        }

        let row_start = (y0.ffloor() as isize).max(0);
        let row_end = (y1.fceil() as isize).min(self.h as isize);
        let mut row = row_start;
        while row < row_end {
            let row_top = row as f32;
            let row_bot = row_top + 1.0;
            let ya = y0.max(row_top);
            let yb = y1.min(row_bot);
            if ya < yb {
                let xa = x0 + (ya - y0) * dxdy;
                let xb = x0 + (yb - y0) * dxdy;
                let dy = (yb - ya) * dir;
                self.add_row_span(row as usize, xa, xb, dy);
            }
            row += 1;
        }
    }

    /// Adds every edge of a (conceptually closed) polyline, closing it back to its
    /// first point if the caller has not already done so.
    pub fn add_polyline(&mut self, pts: &[Point]) {
        if pts.len() < 2 {
            return;
        }
        for w in pts.windows(2) {
            self.add_line(w[0].x, w[0].y, w[1].x, w[1].y);
        }
        let first = pts[0];
        let last = pts[pts.len() - 1];
        if first != last {
            self.add_line(last.x, last.y, first.x, first.y);
        }
    }

    fn add_row_span(&mut self, row: usize, xa: f32, xb: f32, dy: f32) {
        let w = self.w as f32;
        let xa = xa.clamp(0.0, w);
        let xb = xb.clamp(0.0, w);
        let (x0, x1) = if xa <= xb { (xa, xb) } else { (xb, xa) };
        let base = row * self.w;

        if x1 - x0 < 1e-6 {
            let cell = (x0 as usize).min(self.w - 1);
            let frac = x0 - cell as f32;
            let area_local = dy * (1.0 - frac);
            self.acc[base + cell] += area_local;
            if cell + 1 < self.w {
                self.acc[base + cell + 1] += dy - area_local;
            }
            return;
        }

        let cell0 = (x0.ffloor() as usize).min(self.w - 1);
        let cell1 = ((x1 - 1e-6).ffloor() as usize).min(self.w - 1);
        let dydx = dy / (x1 - x0);
        let mut prev_x = x0;
        for cell in cell0..=cell1 {
            let cell_left = cell as f32;
            let cell_right = cell_left + 1.0;
            let seg_x0 = prev_x.max(cell_left);
            let seg_x1 = x1.min(cell_right);
            if seg_x1 <= seg_x0 {
                continue;
            }
            let seg_dy = (seg_x1 - seg_x0) * dydx;
            let xa_frac = seg_x0 - cell_left;
            let xb_frac = seg_x1 - cell_left;
            let area_local = seg_dy * (1.0 - (xa_frac + xb_frac) * 0.5);
            self.acc[base + cell] += area_local;
            let carry = seg_dy - area_local;
            if cell + 1 < self.w {
                self.acc[base + cell + 1] += carry;
            }
            prev_x = seg_x1;
        }
    }

    /// Resolves the accumulated signed area into 8-bit non-zero-rule coverage
    /// (`w * h` bytes, row-major): `min(|prefix sum|, 1) * 255`, rounded.
    pub fn accumulate_coverage(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.w * self.h];
        for row in 0..self.h {
            let base = row * self.w;
            let mut acc = 0f32;
            for x in 0..self.w {
                acc += self.acc[base + x];
                let coverage = acc.abs().min(1.0);
                out[base + x] = (coverage * 255.0 + 0.5) as u8;
            }
        }
        out
    }
}

/// An anti-aliased coverage mask, positioned at integer offset `(x, y)` in whatever
/// space the source path was defined in.
pub struct Mask {
    /// X offset of the mask's top-left corner.
    pub x: i32,
    /// Y offset of the mask's top-left corner.
    pub y: i32,
    /// Mask width in pixels.
    pub w: usize,
    /// Mask height in pixels.
    pub h: usize,
    /// Row-major 8-bit coverage, `w * h` bytes.
    pub coverage: Vec<u8>,
}

impl Mask {
    /// Total coverage summed over every pixel, in fractional pixels (e.g. `pi *
    /// r^2` for a circle of radius `r`). Used by area-based tests.
    pub fn total_coverage(&self) -> f32 {
        self.coverage.iter().map(|&c| c as f32 / 255.0).sum()
    }
}

/// Rasterizes `path` (non-zero winding rule) into a [`Mask`] sized to just contain
/// it (plus a 1px margin for antialiasing), or `None` for an empty/degenerate path.
pub fn rasterize(path: &Path, tolerance: f32) -> Option<Mask> {
    let (min, max) = path.control_bounds()?;
    let x0 = min.x.ffloor() as i32 - 1;
    let y0 = min.y.ffloor() as i32 - 1;
    let x1 = max.x.fceil() as i32 + 1;
    let y1 = max.y.fceil() as i32 + 1;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let w = (x1 - x0) as usize;
    let h = (y1 - y0) as usize;
    let offset = Point::new(x0 as f32, y0 as f32);
    let mut r = Rasterizer::new(w, h);
    for poly in path.flatten(tolerance) {
        if poly.len() < 2 {
            continue;
        }
        let shifted: Vec<Point> = poly.iter().map(|p| p.sub(offset)).collect();
        r.add_polyline(&shifted);
    }
    Some(Mask { x: x0, y: y0, w, h, coverage: r.accumulate_coverage() })
}

/// Fills `path` (non-zero winding) with a solid `color`, translated by `origin`,
/// composited onto `canvas`.
pub fn fill_path(canvas: &mut Canvas, path: &Path, origin: Point, color: Color) {
    let placed = path.transformed(Affine::translate(origin.x, origin.y));
    let Some(mask) = rasterize(&placed, 0.2) else { return };
    let premul = color.premul();
    for y in 0..mask.h {
        let row = y * mask.w;
        for x in 0..mask.w {
            let cov = mask.coverage[row + x] as u32;
            if cov == 0 {
                continue;
            }
            canvas.blend_pixel_coverage(mask.x + x as i32, mask.y + y as i32, premul, cov);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::Surface;

    #[test]
    fn axis_aligned_rect_is_fully_covered_inside() {
        let mut p = Path::new();
        p.add_rect(0.0, 0.0, 4.0, 4.0);
        let mask = rasterize(&p, 0.1).unwrap();
        // The rect occupies pixels (0,0)..(3,3) inclusive within the mask (offset by
        // the 1px margin baked into `rasterize`).
        for y in 0..4usize {
            for x in 0..4usize {
                let mx = (x as i32 - mask.x) as usize;
                let my = (y as i32 - mask.y) as usize;
                assert_eq!(mask.coverage[my * mask.w + mx], 255, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn rect_area_matches_analytic_exactly() {
        let mut p = Path::new();
        p.add_rect(0.5, 0.5, 3.0, 2.0);
        let mask = rasterize(&p, 0.1).unwrap();
        let area = mask.total_coverage();
        // 8-bit coverage quantizes each partially-covered pixel to the nearest of
        // 256 levels (e.g. true 0.5 -> 128/255 = 0.50196), so summed area is exact
        // only up to that rounding noise, not bit-for-bit -- bounded well within 1%
        // of the analytic area for a shape this size.
        assert!((area - 6.0).abs() / 6.0 < 0.01, "area={area}, want ~6.0");
    }

    #[test]
    fn circle_area_within_one_percent_of_analytic() {
        let mut p = Path::new();
        p.add_circle(20.0, 20.0, 10.0);
        let mask = rasterize(&p, 0.05).unwrap();
        let area = mask.total_coverage();
        let analytic = core::f32::consts::PI * 100.0;
        assert!((area - analytic).abs() / analytic < 0.01, "area={area}, want ~{analytic}");
    }

    #[test]
    fn hole_from_opposite_winding_is_empty() {
        // Outer square CW, inner square CCW (reverse point order): nonzero winding
        // should punch a hole.
        let mut p = Path::new();
        p.move_to(0.0, 0.0).line_to(10.0, 0.0).line_to(10.0, 10.0).line_to(0.0, 10.0).close();
        p.move_to(3.0, 3.0).line_to(3.0, 7.0).line_to(7.0, 7.0).line_to(7.0, 3.0).close();
        let mask = rasterize(&p, 0.1).unwrap();
        let get = |x: i32, y: i32| -> u8 {
            let mx = (x - mask.x) as usize;
            let my = (y - mask.y) as usize;
            mask.coverage[my * mask.w + mx]
        };
        assert_eq!(get(5, 5), 0, "centre of the hole must be uncovered");
        assert_eq!(get(1, 1), 255, "outside the hole must be fully covered");
    }

    #[test]
    fn same_winding_squares_union_without_double_darkening() {
        let mut p = Path::new();
        p.add_rect(0.0, 0.0, 10.0, 10.0);
        p.add_rect(5.0, 5.0, 10.0, 10.0);
        let mask = rasterize(&p, 0.1).unwrap();
        let get = |x: i32, y: i32| -> u8 {
            let mx = (x - mask.x) as usize;
            let my = (y - mask.y) as usize;
            mask.coverage[my * mask.w + mx]
        };
        assert_eq!(get(7, 7), 255, "overlap region should saturate at full coverage");
    }

    #[test]
    fn empty_path_rasterizes_to_none() {
        assert!(rasterize(&Path::new(), 0.1).is_none());
    }

    #[test]
    fn fill_path_blends_onto_canvas() {
        let mut s = Surface::new(20, 20);
        let mut p = Path::new();
        p.add_rect(0.0, 0.0, 10.0, 10.0);
        {
            let mut c = s.canvas();
            fill_path(&mut c, &p, Point::new(2.0, 2.0), Color::WHITE);
        }
        assert_eq!(s.get(5, 5), Color::WHITE.premul());
        assert_eq!(s.get(0, 0), 0);
    }

    #[test]
    fn diagonal_edge_conserves_total_area_across_many_columns() {
        // A long, shallow diagonal edge spans many pixel columns within one row;
        // exercise the multi-cell walk in `add_row_span` and check the triangle's
        // area comes out exact.
        let mut p = Path::new();
        p.move_to(0.0, 0.0).line_to(40.0, 1.0).line_to(0.0, 1.0).close();
        let mask = rasterize(&p, 0.1).unwrap();
        let area = mask.total_coverage();
        assert!((area - 20.0).abs() < 0.05, "area={area}, want 20.0");
    }
}
