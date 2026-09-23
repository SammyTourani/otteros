//! Integration tests against the real Inter and JetBrains Mono font files
//! (brief M4-T2). These need `scripts/fetch-fonts.sh` to have been run first;
//! they fail loudly (not silently skip) if the files aren't there, since that's
//! almost certainly what a developer wants to know.

use otter_gfx::font::Font;
use otter_gfx::geom::Affine;
use otter_gfx::raster;

fn load(rel: &str) -> Vec<u8> {
    let path = format!("{}/../../third_party/fonts/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("could not read {path} ({e}); run scripts/fetch-fonts.sh first");
    })
}

fn inter_regular() -> Font<'static> {
    let bytes: &'static [u8] = Box::leak(load("inter/Inter-Regular.ttf").into_boxed_slice());
    Font::parse(bytes).expect("Inter-Regular.ttf should parse")
}

fn jbm_regular() -> Font<'static> {
    let bytes: &'static [u8] = Box::leak(load("jetbrains-mono/JetBrainsMono-Regular.ttf").into_boxed_slice());
    Font::parse(bytes).expect("JetBrainsMono-Regular.ttf should parse")
}

#[test]
fn inter_and_jbm_parse_and_report_sane_metrics() {
    let inter = inter_regular();
    assert!(inter.units_per_em() >= 1000);
    assert!(inter.num_glyphs() > 100);

    let jbm = jbm_regular();
    assert!(jbm.units_per_em() >= 1000);
    assert!(jbm.num_glyphs() > 100);
}

#[test]
fn cmap_finds_ascii_and_e_acute() {
    let inter = inter_regular();
    let a = inter.glyph_id_for_char('A');
    let e_acute = inter.glyph_id_for_char('\u{e9}'); // é
    assert_ne!(a, 0, "'A' must have a real glyph");
    assert_ne!(e_acute, 0, "'é' must have a real glyph");
    assert_ne!(a, e_acute);
}

#[test]
fn jetbrains_mono_ascii_advances_are_all_equal() {
    let jbm = jbm_regular();
    let mut advances = (0x20u32..=0x7E).map(|cp| {
        let c = char::from_u32(cp).unwrap();
        let gid = jbm.glyph_id_for_char(c);
        assert_ne!(gid, 0, "JetBrains Mono should map ASCII {c:?}");
        jbm.advance_width(gid)
    });
    let first = advances.next().unwrap();
    for (i, w) in advances.enumerate() {
        assert_eq!(w, first, "glyph at ascii offset {i} has a different advance (not monospace)");
    }
}

#[test]
fn glyph_bboxes_fit_inside_the_head_bbox() {
    let inter = inter_regular();
    let (fx0, fy0, fx1, fy1) = inter.bbox();
    // Sample a good spread of glyphs (every 37th, a prime step, avoids any
    // periodic artifact) rather than all several thousand for test speed.
    let mut checked = 0;
    for gid in (0..inter.num_glyphs()).step_by(37) {
        let Ok(path) = inter.outline(gid) else { continue };
        let Some((min, max)) = path.control_bounds() else { continue };
        // Bezier control points can slightly overshoot the true on-curve bbox, so
        // allow a small tolerance (a handful of font units) rather than demanding
        // bit-exact containment.
        let tol = 8.0;
        assert!(min.x >= fx0 as f32 - tol, "glyph {gid} min.x={} < head xMin={}", min.x, fx0);
        assert!(min.y >= fy0 as f32 - tol, "glyph {gid} min.y={} < head yMin={}", min.y, fy0);
        assert!(max.x <= fx1 as f32 + tol, "glyph {gid} max.x={} > head xMax={}", max.x, fx1);
        assert!(max.y <= fy1 as f32 + tol, "glyph {gid} max.y={} > head yMax={}", max.y, fy1);
        checked += 1;
    }
    assert!(checked > 10, "expected to have actually checked a good number of glyphs");
}

#[test]
fn e_acute_is_a_composite_of_two_components() {
    let inter = inter_regular();
    let gid = inter.glyph_id_for_char('\u{e9}');
    assert_ne!(gid, 0);
    let n = inter.component_count(gid).unwrap();
    assert_eq!(n, 2, "é should be composed of a base 'e' and an acute accent");
}

#[test]
fn kerning_pair_av_in_inter_is_negative() {
    let inter = inter_regular();
    let a = inter.glyph_id_for_char('A');
    let v = inter.glyph_id_for_char('V');
    let k = inter.kerning(a, v);
    assert!(k < 0, "kerning(A,V) = {k}, want negative (A and V should tuck together)");
}

/// Rasterizes one glyph at `size_px`, returning its coverage mask.
fn rasterize_char(font: &Font, c: char, size_px: f32) -> raster::Mask {
    let gid = font.glyph_id_for_char(c);
    assert_ne!(gid, 0, "{c:?} should have a glyph");
    let outline = font.outline(gid).unwrap();
    let scale = size_px / font.units_per_em() as f32;
    let placed = outline.transformed(Affine { a: scale, b: 0.0, c: 0.0, d: -scale, e: 0.0, f: 0.0 });
    raster::rasterize(&placed, 0.2).expect("a visible glyph should rasterize to something")
}

#[test]
fn lowercase_l_has_a_plausible_stem_width_at_32px() {
    let inter = inter_regular();
    let mask = rasterize_char(&inter, 'l', 32.0);
    // Sum coverage across the middle row: for a single vertical stroke this
    // approximates the stem's width in pixels, antialiasing included.
    let mid_row = mask.h / 2;
    let row = &mask.coverage[mid_row * mask.w..(mid_row + 1) * mask.w];
    let width: f32 = row.iter().map(|&c| c as f32 / 255.0).sum();
    assert!((1.0..=6.0).contains(&width), "measured stem width {width}px at 32px is not plausible");
}

#[test]
fn lowercase_o_has_an_empty_centre() {
    let inter = inter_regular();
    let mask = rasterize_char(&inter, 'o', 48.0);
    let cx = mask.w / 2;
    let cy = mask.h / 2;
    let centre = mask.coverage[cy * mask.w + cx];
    assert_eq!(centre, 0, "the centre of 'o' should be hollow, got coverage {centre}");
    // But it should be a real, mostly-solid glyph, not degenerate/empty overall.
    let total: u32 = mask.coverage.iter().map(|&c| c as u32).sum();
    assert!(total > 1000, "total coverage {total} looks too small for a real 'o'");
}

#[test]
fn malformed_font_fuzz_truncation_never_panics() {
    let bytes = load("inter/Inter-Regular.ttf");
    for i in 0..200usize {
        let len = (i * 37 + 4).min(bytes.len());
        let truncated = &bytes[..len];
        if let Ok(font) = Font::parse(truncated) {
            let _ = font.outline(0);
            let _ = font.outline(1);
            let _ = font.glyph_id_for_char('A');
            let _ = font.advance_width(0);
            let _ = font.kerning(0, 1);
        }
    }
}

#[test]
fn malformed_font_fuzz_bit_flips_never_panics() {
    let original = load("inter/Inter-Regular.ttf");
    // Deterministic pseudo-random offsets (no external RNG crate needed): a
    // simple multiplicative congruential sequence covering the whole file.
    let mut state: u64 = 0x2545F4914F6CDD1D;
    for _ in 0..200 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let offset = (state >> 33) as usize % original.len();
        let mut mutated = original.clone();
        mutated[offset] ^= 0xFF;
        if let Ok(font) = Font::parse(&mutated) {
            for gid in [0u16, 1, 2, 36, 100] {
                let _ = font.outline(gid);
                let _ = font.advance_width(gid);
            }
            let _ = font.glyph_id_for_char('A');
            let _ = font.glyph_id_for_char('\u{e9}');
        }
    }
}
