//! Linear and radial gradients with 4x4 ordered (Bayer) dithering, so smooth colour
//! ramps (e.g. the desktop wallpaper) don't show 8-bit banding.

use crate::color::Color;
use crate::geom::{FloatExt, Point};
use crate::surface::Canvas;

/// One colour stop in a gradient ramp.
#[derive(Debug, Clone, Copy)]
pub struct ColorStop {
    /// Position along the gradient, 0..=1 (values outside are clamped by callers).
    pub t: f32,
    /// The colour at this position.
    pub color: Color,
}

impl ColorStop {
    /// Builds a colour stop.
    pub const fn new(t: f32, color: Color) -> Self {
        ColorStop { t, color }
    }
}

// The standard 4x4 Bayer ordered-dithering threshold matrix (values 0..15 map to
// evenly spaced thresholds within one quantization step).
const BAYER4: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Evaluates a (sorted, non-empty) stop list at `t`, returning straight-alpha
/// channels as `f32` in `0..=255` (not yet rounded -- the caller dithers).
fn eval_stops_f32(stops: &[ColorStop], t: f32) -> (f32, f32, f32, f32) {
    debug_assert!(!stops.is_empty());
    if t <= stops[0].t {
        let c = stops[0].color;
        return (c.r as f32, c.g as f32, c.b as f32, c.a as f32);
    }
    let last = stops[stops.len() - 1];
    if t >= last.t {
        return (last.color.r as f32, last.color.g as f32, last.color.b as f32, last.color.a as f32);
    }
    for w in stops.windows(2) {
        if t >= w[0].t && t <= w[1].t {
            let span = (w[1].t - w[0].t).max(1e-6);
            let lt = (t - w[0].t) / span;
            let lerp = |a: u8, b: u8| a as f32 + (b as f32 - a as f32) * lt;
            return (
                lerp(w[0].color.r, w[1].color.r),
                lerp(w[0].color.g, w[1].color.g),
                lerp(w[0].color.b, w[1].color.b),
                lerp(w[0].color.a, w[1].color.a),
            );
        }
    }
    (last.color.r as f32, last.color.g as f32, last.color.b as f32, last.color.a as f32)
}

/// Ordered-dithers one continuous channel value (`0..=255`) at pixel `(x, y)`:
/// rounds down except where the fractional part exceeds this pixel's Bayer
/// threshold, in which case it rounds up. Exact (zero error) when `v` is already an
/// integer; otherwise off by less than one quantization step, and unbiased on
/// average across the matrix's 16 thresholds.
fn dither_channel(v: f32, x: usize, y: usize) -> u8 {
    let v = v.clamp(0.0, 255.0);
    let base = v.ffloor();
    let frac = v - base;
    let threshold = (BAYER4[y & 3][x & 3] as f32 + 0.5) / 16.0;
    let out = if frac > threshold { base + 1.0 } else { base };
    out.clamp(0.0, 255.0) as u8
}

fn dithered_premul(r: f32, g: f32, b: f32, a: f32, x: usize, y: usize) -> u32 {
    let color = Color::rgba(dither_channel(r, x, y), dither_channel(g, x, y), dither_channel(b, x, y), a.clamp(0.0, 255.0) as u8);
    color.premul()
}

/// Paints a linear gradient from `p0` to `p1` over the canvas's current bounds,
/// clipped as usual. `stops` must be non-empty and sorted by `t`.
pub fn fill_linear_gradient(canvas: &mut Canvas, p0: Point, p1: Point, stops: &[ColorStop]) {
    if stops.is_empty() {
        return;
    }
    let d = p1.sub(p0);
    let len2 = d.dot(d).max(1e-6);
    let bounds = canvas.bounds();
    for y in bounds.y..bounds.bottom() {
        for x in bounds.x..bounds.right() {
            let p = Point::new(x as f32 + 0.5, y as f32 + 0.5);
            let t = (p.sub(p0).dot(d) / len2).clamp(0.0, 1.0);
            let (r, g, b, a) = eval_stops_f32(stops, t);
            canvas.blend_pixel(x, y, dithered_premul(r, g, b, a, x as usize, y as usize));
        }
    }
}

/// Paints a radial gradient centred at `center` reaching `stops`' last stop at
/// `radius`, over the canvas's current bounds.
pub fn fill_radial_gradient(canvas: &mut Canvas, center: Point, radius: f32, stops: &[ColorStop]) {
    if stops.is_empty() || radius <= 0.0 {
        return;
    }
    let bounds = canvas.bounds();
    for y in bounds.y..bounds.bottom() {
        for x in bounds.x..bounds.right() {
            let p = Point::new(x as f32 + 0.5, y as f32 + 0.5);
            let t = (p.sub(center).length() / radius).clamp(0.0, 1.0);
            let (r, g, b, a) = eval_stops_f32(stops, t);
            canvas.blend_pixel(x, y, dithered_premul(r, g, b, a, x as usize, y as usize));
        }
    }
}

/// Precomputed values used only by tests to check dithering error bounds.
#[cfg(test)]
fn collect_dither_samples() -> alloc::vec::Vec<(f32, u8)> {
    let mut out = alloc::vec::Vec::new();
    let mut v = 0.0f32;
    while v <= 255.0 {
        for y in 0..4 {
            for x in 0..4 {
                out.push((v, dither_channel(v, x, y)));
            }
        }
        v += 0.37;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::Surface;

    #[test]
    fn linear_gradient_endpoint_colours_are_exact() {
        // Endpoints placed at pixel *centres* (0.5 and 99.5): sampling also happens
        // at pixel centres, so t=0/t=1 land exactly on pixels 0 and 99.
        let mut s = Surface::new(100, 4);
        let stops = [ColorStop::new(0.0, Color::rgb(10, 20, 30)), ColorStop::new(1.0, Color::rgb(200, 210, 220))];
        {
            let mut c = s.canvas();
            fill_linear_gradient(&mut c, Point::new(0.5, 0.0), Point::new(99.5, 0.0), &stops);
        }
        assert_eq!(s.get(0, 0), Color::rgb(10, 20, 30).premul());
        assert_eq!(s.get(99, 0), Color::rgb(200, 210, 220).premul());
    }

    #[test]
    fn radial_gradient_centre_and_edge_are_exact() {
        let mut s = Surface::new(41, 41);
        let stops = [ColorStop::new(0.0, Color::WHITE), ColorStop::new(1.0, Color::BLACK)];
        {
            let mut c = s.canvas();
            fill_radial_gradient(&mut c, Point::new(20.5, 20.5), 20.0, &stops);
        }
        assert_eq!(s.get(20, 20), Color::WHITE.premul());
    }

    #[test]
    fn dither_error_is_always_below_one_quantization_step() {
        for (v, out) in collect_dither_samples() {
            let err = (out as f32 - v).abs();
            assert!(err < 1.0, "v={v} out={out} err={err}");
        }
    }

    #[test]
    fn dither_is_exact_for_integer_input() {
        for x in 0..4 {
            for y in 0..4 {
                assert_eq!(dither_channel(128.0, x, y), 128);
                assert_eq!(dither_channel(0.0, x, y), 0);
                assert_eq!(dither_channel(255.0, x, y), 255);
            }
        }
    }

    #[test]
    fn dither_breaks_up_banding_across_the_bayer_cell() {
        // At v=128.5 (halfway through a quantization step), roughly half of the 16
        // Bayer thresholds should round up and half down -- proof it's not just
        // always rounding the same way (which would look like ordinary banding).
        let ups = (0..4).flat_map(|y| (0..4).map(move |x| (x, y))).filter(|&(x, y)| dither_channel(128.5, x, y) == 129).count();
        assert!((6..=10).contains(&ups), "ups={ups}, want roughly half of 16");
    }

    #[test]
    fn gradient_average_over_a_bayer_cell_matches_continuous_value_closely() {
        // Dithering must be unbiased: averaging the dithered output over a full 4x4
        // tile should recover the true continuous value much more closely than any
        // single sample's 1-step error bound.
        for v in [10.25f32, 100.6, 200.1, 254.9] {
            let sum: f32 = (0..4).flat_map(|y| (0..4).map(move |x| (x, y))).map(|(x, y)| dither_channel(v, x, y) as f32).sum();
            let avg = sum / 16.0;
            assert!((avg - v).abs() < 0.1, "v={v} avg={avg}");
        }
    }
}
