//! Box shadows for rounded rectangles: an alpha mask rasterized once, blurred with
//! a separable (horizontal-then-vertical) box blur, and cached by size so many
//! same-sized window/button shadows don't re-blur every frame.

use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;

use crate::color::Color;
use crate::geom::{FloatExt, Point};
use crate::shapes::{sd_round_box, supersample_coverage};
use crate::surface::Canvas;

/// A blurred alpha mask ready to be tinted and composited under a shape.
pub struct ShadowMask {
    /// Mask width in pixels.
    pub w: usize,
    /// Mask height in pixels.
    pub h: usize,
    /// Margin baked into `w`/`h` on every side beyond the shadow's nominal
    /// (spread-expanded) rectangle, to hold the blur's falloff.
    pub margin: i32,
    /// Row-major alpha (0..=255), `w * h` bytes.
    pub data: Vec<u8>,
}

/// Chooses a single box-blur radius approximating a Gaussian of standard deviation
/// `sigma` well enough for a UI drop shadow (three passes of this radius, in each
/// direction, is a standard cheap approximation to a true Gaussian blur).
fn box_radius_for_sigma(sigma: f32) -> i32 {
    if sigma <= 0.0 {
        return 0;
    }
    (sigma * 1.5).fround().max(1.0) as i32
}

fn box_blur_horizontal(buf: &mut [u8], w: usize, h: usize, r: i32) {
    if r <= 0 || w == 0 {
        return;
    }
    let norm = 2 * r + 1;
    let mut out = vec![0u8; w];
    for row in 0..h {
        let base = row * w;
        let mut sum: i32 = 0;
        for k in 0..=r {
            if (k as usize) < w {
                sum += buf[base + k as usize] as i32;
            }
        }
        for x in 0..w as i32 {
            out[x as usize] = ((sum + norm / 2) / norm).clamp(0, 255) as u8;
            let remove = x - r;
            let add = x + r + 1;
            if remove >= 0 && (remove as usize) < w {
                sum -= buf[base + remove as usize] as i32;
            }
            if add >= 0 && (add as usize) < w {
                sum += buf[base + add as usize] as i32;
            }
        }
        buf[base..base + w].copy_from_slice(&out);
    }
}

fn box_blur_vertical(buf: &mut [u8], w: usize, h: usize, r: i32) {
    if r <= 0 || h == 0 {
        return;
    }
    let norm = 2 * r + 1;
    let mut out = vec![0u8; h];
    for col in 0..w {
        let mut sum: i32 = 0;
        for k in 0..=r {
            if (k as usize) < h {
                sum += buf[k as usize * w + col] as i32;
            }
        }
        for y in 0..h as i32 {
            out[y as usize] = ((sum + norm / 2) / norm).clamp(0, 255) as u8;
            let remove = y - r;
            let add = y + r + 1;
            if remove >= 0 && (remove as usize) < h {
                sum -= buf[remove as usize * w + col] as i32;
            }
            if add >= 0 && (add as usize) < h {
                sum += buf[add as usize * w + col] as i32;
            }
        }
        for y in 0..h {
            buf[y * w + col] = out[y];
        }
    }
}

/// Builds a blurred alpha mask for a rounded rect `w`x`h` with corner `radius`,
/// grown by `spread` on every side before blurring by `blur` (a CSS-style blur
/// radius: standard deviation is `blur / 2`).
pub fn build_shadow_mask(w: f32, h: f32, radius: f32, blur: f32, spread: f32) -> ShadowMask {
    let sw = (w + 2.0 * spread).max(0.0);
    let sh = (h + 2.0 * spread).max(0.0);
    let sr = (radius + spread).clamp(0.0, sw.min(sh) * 0.5);
    let sigma = (blur * 0.5).max(0.0);
    let box_r = box_radius_for_sigma(sigma);
    let margin = 3 * box_r + 2;

    let width = sw.fceil() as i32 + 2 * margin;
    let height = sh.fceil() as i32 + 2 * margin;
    let width = width.max(1) as usize;
    let height = height.max(1) as usize;

    let mut data = vec![0u8; width * height];
    let center = Point::new(margin as f32 + sw * 0.5, margin as f32 + sh * 0.5);
    let half = Point::new(sw * 0.5, sh * 0.5);
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let cov = supersample_coverage(x, y, |sx, sy| sd_round_box(Point::new(sx, sy), center, half, sr) <= 0.0);
            data[y as usize * width + x as usize] = cov as u8;
        }
    }

    box_blur_horizontal(&mut data, width, height, box_r);
    box_blur_horizontal(&mut data, width, height, box_r);
    box_blur_horizontal(&mut data, width, height, box_r);
    box_blur_vertical(&mut data, width, height, box_r);
    box_blur_vertical(&mut data, width, height, box_r);
    box_blur_vertical(&mut data, width, height, box_r);

    ShadowMask { w: width, h: height, margin, data }
}

/// Composites a shadow mask, tinted by `color`, so its un-blurred rounded-rect
/// footprint's top-left corner lands at `(rect_x, rect_y)`.
pub fn draw_shadow_mask(canvas: &mut Canvas, mask: &ShadowMask, rect_x: f32, rect_y: f32, color: Color) {
    let ox = (rect_x - mask.margin as f32).fround() as i32;
    let oy = (rect_y - mask.margin as f32).fround() as i32;
    for y in 0..mask.h {
        for x in 0..mask.w {
            let a = mask.data[y * mask.w + x];
            if a == 0 {
                continue;
            }
            let scaled_a = crate::color::div255(color.a as u32 * a as u32) as u8;
            let c = Color::rgba(color.r, color.g, color.b, scaled_a);
            canvas.blend_pixel(ox + x as i32, oy + y as i32, c.premul());
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ShadowKey(u32, u32, u32, u32, u32);

impl ShadowKey {
    fn new(w: f32, h: f32, radius: f32, blur: f32, spread: f32) -> Self {
        ShadowKey(w.to_bits(), h.to_bits(), radius.to_bits(), blur.to_bits(), spread.to_bits())
    }
}

/// Caches blurred shadow masks by their `(w, h, radius, blur, spread)` parameters,
/// so repeatedly-drawn same-sized shadows (typical of UI chrome) reuse the blur
/// work instead of recomputing it every draw.
pub struct ShadowCache {
    entries: Vec<(ShadowKey, Rc<ShadowMask>)>,
}

impl ShadowCache {
    /// An empty cache.
    pub fn new() -> Self {
        ShadowCache { entries: Vec::new() }
    }

    /// Returns the mask for these parameters, building (and caching) it if this is
    /// the first time they've been seen.
    pub fn get(&mut self, w: f32, h: f32, radius: f32, blur: f32, spread: f32) -> Rc<ShadowMask> {
        let key = ShadowKey::new(w, h, radius, blur, spread);
        if let Some((_, mask)) = self.entries.iter().find(|(k, _)| *k == key) {
            return mask.clone();
        }
        let mask = Rc::new(build_shadow_mask(w, h, radius, blur, spread));
        const MAX_ENTRIES: usize = 16;
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push((key, mask.clone()));
        mask
    }

    /// Number of masks currently cached (used by tests).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if the cache holds nothing yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for ShadowCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(mask: &ShadowMask) -> f64 {
        mask.data.iter().map(|&v| v as f64).sum()
    }

    #[test]
    fn blur_preserves_total_alpha_within_rounding_noise() {
        let sharp = build_shadow_mask(30.0, 20.0, 6.0, 0.0, 0.0);
        let blurred = build_shadow_mask(30.0, 20.0, 6.0, 16.0, 0.0);
        let before = total(&sharp);
        let after = total(&blurred);
        assert!((before - after).abs() / before < 0.02, "before={before} after={after}");
    }

    #[test]
    fn blur_softens_the_edge() {
        let sharp = build_shadow_mask(30.0, 20.0, 6.0, 0.0, 0.0);
        let blurred = build_shadow_mask(30.0, 20.0, 6.0, 16.0, 0.0);
        // Count the *fraction* of fully-saturated (255) or fully-zero pixels (not
        // the raw count: blurring grows the buffer's margin, so raw counts aren't
        // comparable). Blurring should shrink that fraction in favour of a soft ramp.
        let hard_fraction = |m: &ShadowMask| {
            m.data.iter().filter(|&&v| v == 0 || v == 255).count() as f64 / m.data.len() as f64
        };
        assert!(hard_fraction(&blurred) < hard_fraction(&sharp), "blurred={} sharp={}", hard_fraction(&blurred), hard_fraction(&sharp));
    }

    #[test]
    fn zero_blur_and_spread_matches_a_plain_rounded_rect_mask() {
        let mask = build_shadow_mask(10.0, 10.0, 3.0, 0.0, 0.0);
        assert_eq!(mask.margin, 2); // 3*box_r(0)+2
        // Centre should be fully covered, a corner-of-the-corner fully empty.
        let get = |x: i32, y: i32| mask.data[y as usize * mask.w + x as usize];
        assert_eq!(get(mask.margin + 5, mask.margin + 5), 255);
        assert_eq!(get(mask.margin, mask.margin), 0);
    }

    #[test]
    fn spread_grows_the_shape() {
        let no_spread = build_shadow_mask(10.0, 10.0, 2.0, 0.0, 0.0);
        let spread = build_shadow_mask(10.0, 10.0, 2.0, 0.0, 5.0);
        assert!(total(&spread) > total(&no_spread) * 1.5);
    }

    #[test]
    fn cache_reuses_the_same_mask_for_repeated_params() {
        let mut cache = ShadowCache::new();
        let a = cache.get(40.0, 30.0, 10.0, 24.0, 0.0);
        assert_eq!(cache.len(), 1);
        let b = cache.get(40.0, 30.0, 10.0, 24.0, 0.0);
        assert_eq!(cache.len(), 1, "second call with the same params must not rebuild");
        assert!(Rc::ptr_eq(&a, &b));
        cache.get(41.0, 30.0, 10.0, 24.0, 0.0);
        assert_eq!(cache.len(), 2);
    }
}
