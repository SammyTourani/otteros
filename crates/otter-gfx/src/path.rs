//! Vector paths: move/line/quad/cubic/close, adaptive flattening to polylines, and
//! affine transforms. Shared by glyph outlines, procedural icons and stroking.

use alloc::vec::Vec;

use crate::geom::{Affine, Point};

/// One path command, in the order they were recorded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathOp {
    /// Starts a new subpath at an absolute point.
    MoveTo(Point),
    /// A straight line from the current point.
    LineTo(Point),
    /// A quadratic Bezier from the current point (control point, end point).
    QuadTo(Point, Point),
    /// A cubic Bezier from the current point (two control points, end point).
    CubicTo(Point, Point, Point),
    /// Closes the current subpath back to its `MoveTo` point.
    Close,
}

/// A sequence of path commands, possibly containing several subpaths.
#[derive(Debug, Clone, Default)]
pub struct Path {
    ops: Vec<PathOp>,
}

impl Path {
    /// An empty path.
    pub fn new() -> Self {
        Path { ops: Vec::new() }
    }

    /// True if the path has no commands.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// The recorded commands, in order.
    pub fn ops(&self) -> &[PathOp] {
        &self.ops
    }

    /// Starts a new subpath.
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.ops.push(PathOp::MoveTo(Point::new(x, y)));
        self
    }

    /// A line from the current point.
    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.ops.push(PathOp::LineTo(Point::new(x, y)));
        self
    }

    /// A quadratic Bezier from the current point.
    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Self {
        self.ops.push(PathOp::QuadTo(Point::new(cx, cy), Point::new(x, y)));
        self
    }

    /// A cubic Bezier from the current point.
    pub fn cubic_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) -> &mut Self {
        self.ops.push(PathOp::CubicTo(Point::new(c1x, c1y), Point::new(c2x, c2y), Point::new(x, y)));
        self
    }

    /// Closes the current subpath.
    pub fn close(&mut self) -> &mut Self {
        self.ops.push(PathOp::Close);
        self
    }

    /// Appends every command of `other` (e.g. one glyph component into its
    /// composite parent). No coordinate transform is applied; transform first if
    /// needed.
    pub fn extend_from(&mut self, other: &Path) {
        self.ops.extend_from_slice(&other.ops);
    }

    /// Appends a rectangle as a closed subpath, wound clockwise in a y-down space.
    pub fn add_rect(&mut self, x: f32, y: f32, w: f32, h: f32) -> &mut Self {
        self.move_to(x, y).line_to(x + w, y).line_to(x + w, y + h).line_to(x, y + h).close()
    }

    /// Appends a circle approximated by four cubic Beziers (closed subpath), wound
    /// clockwise in a y-down space.
    pub fn add_circle(&mut self, cx: f32, cy: f32, r: f32) -> &mut Self {
        // Magic constant for a 4-cubic circle approximation (error < 0.03% of r).
        const K: f32 = 0.552_284_8;
        let k = r * K;
        self.move_to(cx + r, cy)
            .cubic_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r)
            .cubic_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy)
            .cubic_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r)
            .cubic_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy)
            .close()
    }

    /// Returns a new path with every point transformed by `t`.
    pub fn transformed(&self, t: Affine) -> Path {
        let ops = self
            .ops
            .iter()
            .map(|op| match *op {
                PathOp::MoveTo(p) => PathOp::MoveTo(t.apply(p)),
                PathOp::LineTo(p) => PathOp::LineTo(t.apply(p)),
                PathOp::QuadTo(c, p) => PathOp::QuadTo(t.apply(c), t.apply(p)),
                PathOp::CubicTo(c1, c2, p) => PathOp::CubicTo(t.apply(c1), t.apply(c2), t.apply(p)),
                PathOp::Close => PathOp::Close,
            })
            .collect();
        Path { ops }
    }

    /// The axis-aligned bounding box of every point and control point recorded
    /// (control points slightly over-estimate curve extents, which is safe -- it is
    /// only ever used to size a raster buffer). `None` for an empty path.
    pub fn control_bounds(&self) -> Option<(Point, Point)> {
        let mut min = Point::new(f32::MAX, f32::MAX);
        let mut max = Point::new(f32::MIN, f32::MIN);
        let mut any = false;
        let mut visit = |p: Point| {
            any = true;
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        };
        for op in &self.ops {
            match *op {
                PathOp::MoveTo(p) | PathOp::LineTo(p) => visit(p),
                PathOp::QuadTo(c, p) => {
                    visit(c);
                    visit(p);
                }
                PathOp::CubicTo(c1, c2, p) => {
                    visit(c1);
                    visit(c2);
                    visit(p);
                }
                PathOp::Close => {}
            }
        }
        any.then_some((min, max))
    }

    /// Flattens every subpath into a polyline (implicitly closed for filling: the
    /// rasterizer connects the last point back to the first). Curves are
    /// subdivided adaptively so consecutive segments deviate from the true curve by
    /// at most `tolerance` pixels.
    pub fn flatten(&self, tolerance: f32) -> Vec<Vec<Point>> {
        let mut subpaths: Vec<Vec<Point>> = Vec::new();
        let mut current: Vec<Point> = Vec::new();
        let mut start = Point::new(0.0, 0.0);
        let mut pos = Point::new(0.0, 0.0);

        for op in &self.ops {
            match *op {
                PathOp::MoveTo(p) => {
                    if current.len() > 1 {
                        subpaths.push(core::mem::take(&mut current));
                    } else {
                        current.clear();
                    }
                    start = p;
                    pos = p;
                    current.push(p);
                }
                PathOp::LineTo(p) => {
                    current.push(p);
                    pos = p;
                }
                PathOp::QuadTo(c, p) => {
                    flatten_quad(pos, c, p, tolerance, 0, &mut current);
                    pos = p;
                }
                PathOp::CubicTo(c1, c2, p) => {
                    flatten_cubic(pos, c1, c2, p, tolerance, 0, &mut current);
                    pos = p;
                }
                PathOp::Close => {
                    if pos != start {
                        current.push(start);
                    }
                    pos = start;
                }
            }
        }
        if current.len() > 1 {
            subpaths.push(current);
        }
        subpaths
    }
}

/// Converts `path` into a new *filled* path tracing its stroke outline at the
/// given `width`: a filled quad per flattened segment plus a filled circle at
/// every vertex (round joins and round caps "for free"). Not a general
/// miter/bevel stroker, but simple, robust, and antialiases cleanly through the
/// same non-zero-winding fill used for everything else, since every quad and
/// circle is wound the same direction (see `add_segment_quad`) so overlaps
/// reinforce instead of cancelling out.
pub fn stroke_to_fill(path: &Path, width: f32, tolerance: f32) -> Path {
    let half = width * 0.5;
    let mut out = Path::new();
    for poly in path.flatten(tolerance) {
        match poly.len() {
            0 => {}
            1 => {
                out.add_circle(poly[0].x, poly[0].y, half);
            }
            _ => {
                for w in poly.windows(2) {
                    add_segment_quad(&mut out, w[0], w[1], half);
                }
                for p in &poly {
                    out.add_circle(p.x, p.y, half);
                }
            }
        }
    }
    out
}

fn add_segment_quad(path: &mut Path, a: Point, b: Point, half_width: f32) {
    let d = b.sub(a);
    let len = d.length();
    if len < 1e-6 {
        return;
    }
    // The offset normal is `d` rotated so the quad winds clockwise (same sense
    // as `add_rect`/`add_circle` in this y-down space): see raster.rs's doc
    // comment for why consistent winding matters (opposite-wound overlaps
    // cancel under the non-zero rule instead of reinforcing).
    let nx = d.y / len * half_width;
    let ny = -d.x / len * half_width;
    path.move_to(a.x + nx, a.y + ny)
        .line_to(b.x + nx, b.y + ny)
        .line_to(b.x - nx, b.y - ny)
        .line_to(a.x - nx, a.y - ny)
        .close();
}

fn flatten_quad(p0: Point, c: Point, p1: Point, tolerance: f32, depth: u32, out: &mut Vec<Point>) {
    if depth >= 24 || is_quad_flat(p0, c, p1, tolerance) {
        out.push(p1);
        return;
    }
    let p01 = mid(p0, c);
    let p12 = mid(c, p1);
    let p012 = mid(p01, p12);
    flatten_quad(p0, p01, p012, tolerance, depth + 1, out);
    flatten_quad(p012, p12, p1, tolerance, depth + 1, out);
}

fn flatten_cubic(p0: Point, c1: Point, c2: Point, p1: Point, tolerance: f32, depth: u32, out: &mut Vec<Point>) {
    if depth >= 24 || is_cubic_flat(p0, c1, c2, p1, tolerance) {
        out.push(p1);
        return;
    }
    let p01 = mid(p0, c1);
    let p12 = mid(c1, c2);
    let p23 = mid(c2, p1);
    let p012 = mid(p01, p12);
    let p123 = mid(p12, p23);
    let p0123 = mid(p012, p123);
    flatten_cubic(p0, p01, p012, p0123, tolerance, depth + 1, out);
    flatten_cubic(p0123, p123, p23, p1, tolerance, depth + 1, out);
}

fn mid(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

/// Perpendicular distance from `p` to the (infinite) line through `a`-`b`; falls
/// back to distance from `a` if `a == b`.
fn point_line_distance(p: Point, a: Point, b: Point) -> f32 {
    let ab = b.sub(a);
    let len = ab.length();
    if len < 1e-9 {
        return p.sub(a).length();
    }
    // |ab x ap| / |ab|
    let ap = p.sub(a);
    ((ab.x * ap.y - ab.y * ap.x) / len).abs()
}

fn is_quad_flat(p0: Point, c: Point, p1: Point, tolerance: f32) -> bool {
    point_line_distance(c, p0, p1) <= tolerance
}

fn is_cubic_flat(p0: Point, c1: Point, c2: Point, p1: Point, tolerance: f32) -> bool {
    point_line_distance(c1, p0, p1) <= tolerance && point_line_distance(c2, p0, p1) <= tolerance
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::rasterize;

    #[test]
    fn stroke_to_fill_horizontal_line_has_no_gaps() {
        let mut p = Path::new();
        p.move_to(0.0, 0.0).line_to(50.0, 0.0);
        let stroked = stroke_to_fill(&p, 6.0, 0.1);
        let mask = rasterize(&stroked, 0.1).unwrap();
        // Sample straight down the middle of the stroke: every pixel along the
        // 50px run should be solidly covered (no cancellation gaps between the
        // segment quad and the round caps at either end).
        let my = (0 - mask.y) as usize; // absolute y=0, where the line itself runs
        for x in 5..45 {
            let mx = (x - mask.x) as usize;
            let cov = mask.coverage[my * mask.w + mx];
            assert!(cov > 200, "gap in stroke at x={x}, coverage={cov}");
        }
    }

    #[test]
    fn stroke_to_fill_multi_segment_polyline_has_no_gap_at_the_joint() {
        let mut p = Path::new();
        p.move_to(0.0, 0.0).line_to(20.0, 0.0).line_to(20.0, 20.0);
        let stroked = stroke_to_fill(&p, 8.0, 0.1);
        let mask = rasterize(&stroked, 0.1).unwrap();
        // The corner at (20,0) is where two segments join; it must be filled,
        // not left as a hole from opposing winding.
        let mx = (20 - mask.x) as usize;
        let my = (0 - mask.y) as usize;
        assert!(mask.coverage[my * mask.w + mx] > 200);
    }

    #[test]
    fn stroke_to_fill_area_is_plausible_for_a_line() {
        let mut p = Path::new();
        p.move_to(0.0, 0.0).line_to(100.0, 0.0);
        let width = 4.0;
        let stroked = stroke_to_fill(&p, width, 0.1);
        let mask = rasterize(&stroked, 0.1).unwrap();
        // Expect roughly length*width plus the two round-cap semicircles' area;
        // generous bounds since this stroker isn't pixel-exact by design.
        let expected = 100.0 * width + core::f32::consts::PI * (width / 2.0).powi(2);
        let area = mask.total_coverage();
        assert!((area - expected).abs() / expected < 0.1, "area={area}, want ~{expected}");
    }

    #[test]
    fn flatten_single_line_is_two_points() {
        let mut p = Path::new();
        p.move_to(0.0, 0.0).line_to(10.0, 0.0);
        let sub = p.flatten(0.25);
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0], alloc::vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)]);
    }

    #[test]
    fn flatten_closes_implicitly_back_to_start() {
        let mut p = Path::new();
        p.move_to(0.0, 0.0).line_to(10.0, 0.0).line_to(10.0, 10.0).close();
        let sub = p.flatten(0.25);
        assert_eq!(sub.len(), 1);
        assert_eq!(*sub[0].last().unwrap(), Point::new(0.0, 0.0));
    }

    #[test]
    fn flatten_two_subpaths() {
        let mut p = Path::new();
        p.move_to(0.0, 0.0).line_to(1.0, 0.0);
        p.move_to(5.0, 5.0).line_to(6.0, 5.0);
        let sub = p.flatten(0.25);
        assert_eq!(sub.len(), 2);
    }

    #[test]
    fn flatten_quad_is_flat_within_tolerance() {
        let mut p = Path::new();
        p.move_to(0.0, 0.0).quad_to(50.0, 100.0, 100.0, 0.0);
        let tol = 0.1;
        let poly = &p.flatten(tol)[0];
        // Every flattened vertex should lie within `tol` of *some* segment of a much
        // finer reference flattening (a loose but meaningful flatness check): here
        // we instead check the classic property that each successive triple of
        // points has small deviation from co-linearity beyond the tolerance budget.
        for w in poly.windows(3) {
            let d = point_line_distance(w[1], w[0], w[2]);
            assert!(d <= tol * 4.0, "segment deviates too much: {d}");
        }
        assert!(poly.len() > 2, "a curved quad should need multiple segments");
    }

    #[test]
    fn flatten_straight_quad_is_two_points() {
        // A "curve" whose control point is on the chord is already flat.
        let mut p = Path::new();
        p.move_to(0.0, 0.0).quad_to(5.0, 0.0, 10.0, 0.0);
        let poly = &p.flatten(0.01)[0];
        assert_eq!(poly.len(), 2);
    }

    #[test]
    fn flatten_cubic_circle_stays_round() {
        let mut p = Path::new();
        p.add_circle(0.0, 0.0, 10.0);
        let poly = &p.flatten(0.05)[0];
        for pt in poly {
            let r = pt.length();
            assert!((r - 10.0).abs() < 0.1, "point at radius {r}, want ~10");
        }
    }

    #[test]
    fn control_bounds_of_rect() {
        let mut p = Path::new();
        p.add_rect(1.0, 2.0, 3.0, 4.0);
        let (min, max) = p.control_bounds().unwrap();
        assert_eq!(min, Point::new(1.0, 2.0));
        assert_eq!(max, Point::new(4.0, 6.0));
    }

    #[test]
    fn control_bounds_of_empty_is_none() {
        assert!(Path::new().control_bounds().is_none());
    }

    #[test]
    fn transformed_translates_points() {
        let mut p = Path::new();
        p.move_to(0.0, 0.0).line_to(1.0, 1.0);
        let t = p.transformed(Affine::translate(10.0, 20.0));
        match t.ops()[0] {
            PathOp::MoveTo(pt) => assert_eq!(pt, Point::new(10.0, 20.0)),
            _ => panic!("expected MoveTo"),
        }
    }
}
