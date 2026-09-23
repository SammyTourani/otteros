//! `loca`/`glyf`: simple and composite glyph outlines.

use alloc::vec::Vec;

use super::tables::Reader;
use super::FontError;
use crate::geom::{Affine, Point};
use crate::path::Path;

/// Composite glyphs referencing composite glyphs are limited to this many levels
/// (real fonts never nest more than 2-3 deep; this only guards against malformed
/// or adversarial fonts looping forever).
pub const MAX_COMPOSITE_DEPTH: u32 = 8;
/// A single composite glyph is limited to this many components, for the same
/// reason.
pub const MAX_COMPONENTS: u32 = 128;

fn loca_entry(loca: &Reader, long: bool, index: usize) -> Result<u32, FontError> {
    if long {
        loca.u32(index * 4)
    } else {
        Ok(loca.u16(index * 2)? as u32 * 2)
    }
}

fn glyph_range(loca: &Reader, long: bool, glyph_id: u16, num_glyphs: u16) -> Result<(u32, u32), FontError> {
    if glyph_id >= num_glyphs {
        return Err(FontError::InvalidGlyphId);
    }
    let start = loca_entry(loca, long, glyph_id as usize)?;
    let end = loca_entry(loca, long, glyph_id as usize + 1)?;
    if end < start {
        return Err(FontError::MalformedGlyf);
    }
    Ok((start, end))
}

/// Parses one glyph's outline (font units, y-up), following composite references.
pub fn parse_glyph(
    glyf: &Reader,
    loca: &Reader,
    long: bool,
    num_glyphs: u16,
    glyph_id: u16,
    depth: u32,
) -> Result<Path, FontError> {
    if depth > MAX_COMPOSITE_DEPTH {
        return Err(FontError::CompositeRecursionLimit);
    }
    let (start, end) = glyph_range(loca, long, glyph_id, num_glyphs)?;
    if start == end {
        return Ok(Path::new()); // no outline (e.g. space)
    }
    let g = glyf.sub(start as usize, (end - start) as usize)?;
    let num_contours = g.i16(0)?;
    if num_contours >= 0 {
        parse_simple_glyph(&g, num_contours as usize)
    } else {
        parse_composite_glyph(&g, glyf, loca, long, num_glyphs, depth)
    }
}

fn parse_simple_glyph(g: &Reader, num_contours: usize) -> Result<Path, FontError> {
    let mut off = 10usize; // numberOfContours(2) + bbox(8)
    let mut end_pts = Vec::with_capacity(num_contours);
    for _ in 0..num_contours {
        end_pts.push(g.u16(off)?);
        off += 2;
    }
    let num_points = match end_pts.last() {
        Some(&last) => last as usize + 1,
        None => 0,
    };

    let instr_len = g.u16(off)? as usize;
    off += 2 + instr_len;

    let mut flags: Vec<u8> = Vec::with_capacity(num_points);
    while flags.len() < num_points {
        let f = g.u8(off)?;
        off += 1;
        flags.push(f);
        if f & 0x08 != 0 {
            let repeat = g.u8(off)?;
            off += 1;
            for _ in 0..repeat {
                if flags.len() >= num_points {
                    break;
                }
                flags.push(f);
            }
        }
    }
    if flags.len() != num_points {
        return Err(FontError::MalformedGlyf);
    }

    let mut xs = Vec::with_capacity(num_points);
    let mut x = 0i32;
    for &f in &flags {
        if f & 0x02 != 0 {
            let dx = g.u8(off)? as i32;
            off += 1;
            x += if f & 0x10 != 0 { dx } else { -dx };
        } else if f & 0x10 == 0 {
            x += g.i16(off)? as i32;
            off += 2;
        }
        xs.push(x);
    }
    let mut ys = Vec::with_capacity(num_points);
    let mut y = 0i32;
    for &f in &flags {
        if f & 0x04 != 0 {
            let dy = g.u8(off)? as i32;
            off += 1;
            y += if f & 0x20 != 0 { dy } else { -dy };
        } else if f & 0x20 == 0 {
            y += g.i16(off)? as i32;
            off += 2;
        }
        ys.push(y);
    }

    let mut path = Path::new();
    let mut start_idx = 0usize;
    for &end in &end_pts {
        let end = end as usize;
        if end < start_idx || end >= num_points {
            return Err(FontError::MalformedGlyf);
        }
        build_contour(&mut path, &flags[start_idx..=end], &xs[start_idx..=end], &ys[start_idx..=end]);
        start_idx = end + 1;
    }
    Ok(path)
}

fn mid(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

/// Turns one contour's on/off-curve point list into path commands, handling
/// implied on-curve points between consecutive off-curve (conic control) points
/// and contours that start off-curve (including fully off-curve contours).
fn build_contour(path: &mut Path, flags: &[u8], xs: &[i32], ys: &[i32]) {
    let n = flags.len();
    if n == 0 {
        return;
    }
    let on_curve = |i: usize| flags[i] & 0x01 != 0;
    let pt = |i: usize| Point::new(xs[i] as f32, ys[i] as f32);

    // Normalize into a list starting at an on-curve point (synthesizing one via
    // the standard midpoint rule if the contour has none at all).
    let mut pts: Vec<(bool, Point)> = Vec::with_capacity(n + 1);
    if let Some(first_on) = (0..n).find(|&i| on_curve(i)) {
        for k in 0..n {
            let i = (first_on + k) % n;
            pts.push((on_curve(i), pt(i)));
        }
    } else {
        pts.push((true, mid(pt(n - 1), pt(0))));
        for i in 0..n {
            pts.push((false, pt(i)));
        }
    }

    let m = pts.len();
    let start = pts[0].1;
    path.move_to(start.x, start.y);
    let mut pending: Option<Point> = None;
    for k in 1..=m {
        let (on, p) = pts[k % m];
        if on {
            match pending.take() {
                Some(ctrl) => {
                    path.quad_to(ctrl.x, ctrl.y, p.x, p.y);
                }
                None => {
                    path.line_to(p.x, p.y);
                }
            }
        } else {
            match pending.take() {
                Some(ctrl) => {
                    let implied = mid(ctrl, p);
                    path.quad_to(ctrl.x, ctrl.y, implied.x, implied.y);
                    pending = Some(p);
                }
                None => pending = Some(p),
            }
        }
    }
    path.close();
}

fn parse_composite_glyph(
    g: &Reader,
    glyf: &Reader,
    loca: &Reader,
    long: bool,
    num_glyphs: u16,
    depth: u32,
) -> Result<Path, FontError> {
    const ARG_WORDS: u16 = 0x0001;
    const ARGS_ARE_XY: u16 = 0x0002;
    const WE_HAVE_SCALE: u16 = 0x0008;
    const MORE_COMPONENTS: u16 = 0x0020;
    const WE_HAVE_XY_SCALE: u16 = 0x0040;
    const WE_HAVE_2X2: u16 = 0x0080;

    let mut path = Path::new();
    let mut off = 10usize;
    let mut components = 0u32;
    loop {
        components += 1;
        if components > MAX_COMPONENTS {
            return Err(FontError::CompositeRecursionLimit);
        }
        let flags = g.u16(off)?;
        let glyph_index = g.u16(off + 2)?;
        off += 4;

        let (dx, dy) = if flags & ARG_WORDS != 0 {
            let a1 = g.i16(off)? as f32;
            let a2 = g.i16(off + 2)? as f32;
            off += 4;
            (a1, a2)
        } else {
            let a1 = g.i8(off)? as f32;
            let a2 = g.i8(off + 1)? as f32;
            off += 2;
            (a1, a2)
        };
        let (dx, dy) = if flags & ARGS_ARE_XY != 0 { (dx, dy) } else { (0.0, 0.0) };

        let (a, b, c, d) = if flags & WE_HAVE_SCALE != 0 {
            let s = g.f2dot14(off)?;
            off += 2;
            (s, 0.0, 0.0, s)
        } else if flags & WE_HAVE_XY_SCALE != 0 {
            let sx = g.f2dot14(off)?;
            let sy = g.f2dot14(off + 2)?;
            off += 4;
            (sx, 0.0, 0.0, sy)
        } else if flags & WE_HAVE_2X2 != 0 {
            let a = g.f2dot14(off)?;
            let b = g.f2dot14(off + 2)?;
            let c = g.f2dot14(off + 4)?;
            let d = g.f2dot14(off + 6)?;
            off += 8;
            (a, b, c, d)
        } else {
            (1.0, 0.0, 0.0, 1.0)
        };

        let transform = Affine { a, b, c, d, e: dx, f: dy };
        let component = parse_glyph(glyf, loca, long, num_glyphs, glyph_index, depth + 1)?;
        path.extend_from(&component.transformed(transform));

        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    Ok(path)
}

/// Number of components in glyph `glyph_id`: `0` for an empty glyph (e.g. space),
/// `1` for a simple glyph, `N` for a composite of `N` components. Used by tests
/// (e.g. checking that an accented letter is built from two components) and
/// potentially by future hinting/caching code that treats composites specially.
pub fn count_components(glyf: &Reader, loca: &Reader, long: bool, num_glyphs: u16, glyph_id: u16) -> Result<usize, FontError> {
    const ARG_WORDS: u16 = 0x0001;
    const WE_HAVE_SCALE: u16 = 0x0008;
    const MORE_COMPONENTS: u16 = 0x0020;
    const WE_HAVE_XY_SCALE: u16 = 0x0040;
    const WE_HAVE_2X2: u16 = 0x0080;

    let (start, end) = glyph_range(loca, long, glyph_id, num_glyphs)?;
    if start == end {
        return Ok(0);
    }
    let g = glyf.sub(start as usize, (end - start) as usize)?;
    if g.i16(0)? >= 0 {
        return Ok(1);
    }
    let mut off = 10usize;
    let mut count = 0usize;
    loop {
        count += 1;
        if count as u32 > MAX_COMPONENTS {
            return Err(FontError::CompositeRecursionLimit);
        }
        let flags = g.u16(off)?;
        off += 4; // flags + glyphIndex
        off += if flags & ARG_WORDS != 0 { 4 } else { 2 };
        off += if flags & WE_HAVE_SCALE != 0 {
            2
        } else if flags & WE_HAVE_XY_SCALE != 0 {
            4
        } else if flags & WE_HAVE_2X2 != 0 {
            8
        } else {
            0
        };
        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }
    Ok(count)
}
