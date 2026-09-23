//! [`Surface`]: an owned premultiplied-ARGB8888 pixel buffer, and [`Canvas`]: a
//! (sub-)view into one with its own clip-rectangle stack.

use alloc::vec;
use alloc::vec::Vec;

use crate::color::blend_over_coverage;
use crate::geom::Rect;

/// An owned premultiplied ARGB8888 pixel buffer.
///
/// `stride` is in pixels (words), not bytes, and may exceed `w` (row padding);
/// `px.len() == stride * h`.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    /// Width in pixels.
    pub w: usize,
    /// Height in pixels.
    pub h: usize,
    /// Row stride in pixels.
    pub stride: usize,
    /// Premultiplied ARGB8888 pixels, row-major, `stride` words per row.
    pub px: Vec<u32>,
}

impl Surface {
    /// A new surface filled with transparent black.
    pub fn new(w: usize, h: usize) -> Self {
        Surface { w, h, stride: w, px: vec![0u32; w * h] }
    }

    /// The full-surface rectangle, `(0, 0, w, h)`.
    pub fn rect(&self) -> Rect {
        Rect::new(0, 0, self.w as i32, self.h as i32)
    }

    /// A [`Canvas`] view over the whole surface, with a fresh single-entry clip
    /// stack covering it.
    pub fn canvas(&mut self) -> Canvas<'_> {
        let w = self.w;
        let h = self.h;
        let stride = self.stride;
        Canvas { data: &mut self.px, stride, ox: 0, oy: 0, w, h, clip: vec![Rect::new(0, 0, w as i32, h as i32)] }
    }

    /// A [`Canvas`] view over the part of the surface covered by `rect`
    /// (intersected with the surface bounds), addressed with its own local
    /// `(0,0)..(w,h)` coordinate space.
    pub fn sub_surface(&mut self, rect: Rect) -> Canvas<'_> {
        let clipped = rect.intersect(self.rect());
        let stride = self.stride;
        let (ox, oy, w, h) = if clipped.is_empty() {
            (0, 0, 0, 0)
        } else {
            (clipped.x as usize, clipped.y as usize, clipped.w as usize, clipped.h as usize)
        };
        Canvas { data: &mut self.px, stride, ox, oy, w, h, clip: vec![Rect::new(0, 0, w as i32, h as i32)] }
    }

    /// Fetches a pixel (premultiplied ARGB8888), or `0` if out of bounds.
    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return 0;
        }
        self.px[y as usize * self.stride + x as usize]
    }
}

/// A view into a [`Surface`]'s pixels: an origin offset, an extent, and its own
/// clip-rectangle stack (in the view's local coordinates). All the crate's drawing
/// routines are methods on `Canvas`; `Surface::canvas()`/`sub_surface()` produce one.
pub struct Canvas<'a> {
    data: &'a mut [u32],
    stride: usize,
    ox: usize,
    oy: usize,
    /// Width of this view, in pixels.
    pub w: usize,
    /// Height of this view, in pixels.
    pub h: usize,
    clip: Vec<Rect>,
}

impl<'a> Canvas<'a> {
    #[inline]
    fn index(&self, x: usize, y: usize) -> usize {
        (self.oy + y) * self.stride + (self.ox + x)
    }

    /// The view's own bounds, `(0, 0, w, h)` in local coordinates.
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.w as i32, self.h as i32)
    }

    /// The current (innermost) clip rectangle, in local coordinates.
    pub fn current_clip(&self) -> Rect {
        *self.clip.last().expect("clip stack is never empty")
    }

    /// Intersects `rect` with the current clip and pushes the result. Always
    /// matched with a later [`Canvas::pop_clip`].
    pub fn push_clip(&mut self, rect: Rect) {
        let top = self.current_clip();
        self.clip.push(top.intersect(rect));
    }

    /// Pops the innermost clip pushed by [`Canvas::push_clip`]. A no-op once only
    /// the base (whole-view) clip remains, so callers cannot pop past it.
    pub fn pop_clip(&mut self) {
        if self.clip.len() > 1 {
            self.clip.pop();
        }
    }

    /// Reads a pixel (premultiplied ARGB8888); `0` if outside the view.
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return 0;
        }
        self.data[self.index(x as usize, y as usize)]
    }

    /// Writes a pixel directly (no blending, no clip test beyond view bounds).
    /// Prefer [`Canvas::blend_pixel`] for compositing draw calls; this is for
    /// callers (gradients painting a fresh background) that want to replace, not
    /// blend, and have already accounted for clipping.
    #[inline]
    pub fn set_raw(&mut self, x: i32, y: i32, argb: u32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let idx = self.index(x as usize, y as usize);
        self.data[idx] = argb;
    }

    /// Composites `src` (premultiplied) over the pixel at `(x, y)` with the
    /// Porter-Duff "over" operator, honouring the current clip and view bounds.
    #[inline]
    pub fn blend_pixel(&mut self, x: i32, y: i32, src: u32) {
        self.blend_pixel_coverage(x, y, src, 255);
    }

    /// As [`Canvas::blend_pixel`], additionally scaling `src`'s contribution by
    /// `coverage` (0..=255), e.g. anti-aliasing coverage.
    #[inline]
    pub fn blend_pixel_coverage(&mut self, x: i32, y: i32, src: u32, coverage: u32) {
        if coverage == 0 || x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let clip = self.current_clip();
        if !clip.contains(x, y) {
            return;
        }
        let idx = self.index(x as usize, y as usize);
        self.data[idx] = blend_over_coverage(self.data[idx], src, coverage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;

    #[test]
    fn new_surface_is_transparent() {
        let s = Surface::new(4, 3);
        assert_eq!(s.px.len(), 12);
        assert!(s.px.iter().all(|&p| p == 0));
    }

    #[test]
    fn set_and_get_round_trip() {
        let mut s = Surface::new(4, 4);
        let mut c = s.canvas();
        c.set_raw(1, 2, 0xffaabbcc);
        assert_eq!(s.get(1, 2), 0xffaabbcc);
        assert_eq!(s.get(0, 0), 0);
    }

    #[test]
    fn out_of_bounds_get_is_zero_not_panic() {
        let s = Surface::new(2, 2);
        assert_eq!(s.get(-1, 0), 0);
        assert_eq!(s.get(0, -1), 0);
        assert_eq!(s.get(100, 100), 0);
    }

    #[test]
    fn clip_stack_blocks_writes_outside_top_rect() {
        let mut s = Surface::new(10, 10);
        let mut c = s.canvas();
        c.push_clip(Rect::new(2, 2, 3, 3)); // covers [2,5)x[2,5)
        c.blend_pixel(0, 0, Color::WHITE.premul());
        c.blend_pixel(3, 3, Color::WHITE.premul());
        drop(c);
        assert_eq!(s.get(0, 0), 0, "outside clip must be untouched");
        assert_eq!(s.get(3, 3), Color::WHITE.premul());
    }

    #[test]
    fn nested_clip_is_intersection_and_pop_restores() {
        let mut s = Surface::new(10, 10);
        let mut c = s.canvas();
        c.push_clip(Rect::new(0, 0, 5, 5));
        c.push_clip(Rect::new(3, 3, 5, 5)); // intersection -> [3,5)x[3,5)
        assert_eq!(c.current_clip(), Rect::new(3, 3, 2, 2));
        c.blend_pixel(4, 4, Color::WHITE.premul());
        c.blend_pixel(1, 1, Color::WHITE.premul());
        c.pop_clip();
        assert_eq!(c.current_clip(), Rect::new(0, 0, 5, 5));
        c.blend_pixel(1, 1, Color::WHITE.premul());
        drop(c);
        assert_eq!(s.get(4, 4), Color::WHITE.premul());
        assert_eq!(s.get(1, 1), Color::WHITE.premul(), "pop_clip should widen back out");
    }

    #[test]
    fn pop_clip_cannot_go_past_base() {
        let mut s = Surface::new(4, 4);
        let mut c = s.canvas();
        let base = c.current_clip();
        c.pop_clip();
        c.pop_clip();
        assert_eq!(c.current_clip(), base);
    }

    #[test]
    fn sub_surface_is_offset_and_bounded() {
        let mut s = Surface::new(10, 10);
        {
            let mut sub = s.sub_surface(Rect::new(4, 4, 20, 20)); // clipped to 6x6
            assert_eq!((sub.w, sub.h), (6, 6));
            sub.set_raw(0, 0, 0xffffffff);
            sub.set_raw(5, 5, 0xff000001);
        }
        assert_eq!(s.get(4, 4), 0xffffffff);
        assert_eq!(s.get(9, 9), 0xff000001);
        assert_eq!(s.get(0, 0), 0);
    }

    #[test]
    fn blend_pixel_coverage_zero_leaves_pixel_untouched() {
        let mut s = Surface::new(2, 2);
        let mut c = s.canvas();
        c.set_raw(0, 0, 0xff112233);
        c.blend_pixel_coverage(0, 0, Color::WHITE.premul(), 0);
        drop(c);
        assert_eq!(s.get(0, 0), 0xff112233);
    }
}
