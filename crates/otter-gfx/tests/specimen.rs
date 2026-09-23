//! Renders `artifacts/gfx-specimen.png` (brief M4-T2): the Kelp wallpaper
//! gradient, three overlapping rounded windows with shadows and title bars,
//! Inter text at every size in DESIGN.md's type scale plus JetBrains Mono,
//! both light-on-dark and dark-on-light, and a stroked+filled vector shape --
//! using nothing but this crate's own PNG encoder, rasterizer and text engine.

use otter_gfx::color::Color;
use otter_gfx::font::Font;
use otter_gfx::geom::{Point, Rect};
use otter_gfx::gradient::{fill_radial_gradient, fill_linear_gradient, ColorStop};
use otter_gfx::path::{stroke_to_fill, Path};
use otter_gfx::png::encode;
use otter_gfx::raster::fill_path;
use otter_gfx::shadow::{build_shadow_mask, draw_shadow_mask};
use otter_gfx::shapes::{fill_circle, fill_rounded_rect};
use otter_gfx::surface::Surface;
use otter_gfx::text::{draw_text, measure, word_wrap, GlyphCache};

fn load_font(rel: &str) -> Vec<u8> {
    let path = format!("{}/../../third_party/fonts/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("could not read {path} ({e}); run scripts/fetch-fonts.sh first"))
}

// docs/DESIGN.md "Kelp" colour tokens (sRGB).
const BG_TOP: Color = Color::rgb(0x10, 0x30, 0x3B);
const BG_BOTTOM: Color = Color::rgb(0x08, 0x14, 0x19);
const GLOW: Color = Color::rgba(0x2C, 0x7A, 0x7B, 110);
const GLOW_FADE: Color = Color::rgba(0x2C, 0x7A, 0x7B, 0);
const CHROME: Color = Color::rgba(0x1B, 0x24, 0x29, 235); // ~92%
const CHROME_FOCUS: Color = Color::rgba(0x22, 0x2E, 0x34, 245); // ~96%
const SURFACE: Color = Color::rgb(0xF6, 0xF4, 0xF0);
const SURFACE_DARK: Color = Color::rgb(0x12, 0x1A, 0x1E);
const TEXT: Color = Color::rgb(0x1E, 0x22, 0x26);
const TEXT_INVERSE: Color = Color::rgb(0xE8, 0xEC, 0xEE);
const ACCENT: Color = Color::rgb(0xF2, 0xA5, 0x41);
const ACCENT_ALT: Color = Color::rgb(0x3F, 0xB8, 0xAF);
const DANGER: Color = Color::rgb(0xE5, 0x53, 0x4B);

struct Win<'a> {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    title: &'a str,
    focused: bool,
    body: Color,
    text: Color,
}

#[test]
fn renders_the_design_specimen() {
    let inter_bytes = load_font("inter/Inter-Regular.ttf");
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let jbm_bytes = load_font("jetbrains-mono/JetBrainsMono-Regular.ttf");
    let inter = Font::parse(&inter_bytes).expect("Inter-Regular.ttf parses");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");
    let jbm = Font::parse(&jbm_bytes).expect("JetBrainsMono-Regular.ttf parses");

    const W: usize = 1040;
    const H: usize = 680;
    let mut surface = Surface::new(W, H);
    let mut cache = GlyphCache::new();

    // 1. Wallpaper: top-to-bottom gradient plus a soft radial glow, upper-left.
    {
        let mut canvas = surface.canvas();
        fill_linear_gradient(
            &mut canvas,
            Point::new(0.0, 0.0),
            Point::new(0.0, H as f32),
            &[ColorStop::new(0.0, BG_TOP), ColorStop::new(1.0, BG_BOTTOM)],
        );
        fill_radial_gradient(&mut canvas, Point::new(160.0, 140.0), 480.0, &[ColorStop::new(0.0, GLOW), ColorStop::new(1.0, GLOW_FADE)]);
    }

    let windows = [
        Win { x: 60.0, y: 90.0, w: 380.0, h: 300.0, title: "terminal", focused: false, body: SURFACE_DARK, text: TEXT_INVERSE },
        Win { x: 300.0, y: 190.0, w: 380.0, h: 300.0, title: "notes", focused: false, body: SURFACE, text: TEXT },
        Win { x: 560.0, y: 250.0, w: 420.0, h: 370.0, title: "type scale", focused: true, body: SURFACE, text: TEXT },
    ];

    // 2. Shadows first (DESIGN.md: focused 0/12/32/45%, unfocused 0/6/18/30%),
    //    all behind every window body.
    for win in &windows {
        let (offset_y, blur, alpha): (f32, f32, f32) = if win.focused { (12.0, 32.0, 0.45) } else { (6.0, 18.0, 0.30) };
        let mask = build_shadow_mask(win.w, win.h, 10.0, blur, 0.0);
        draw_shadow_mask(&mut surface.canvas(), &mask, win.x, win.y + offset_y, Color::rgba(0, 0, 0, (alpha * 255.0).round() as u8));
    }

    // 3. Window chrome: rounded body, rounded-top title bar (via a rect clip --
    //    see Canvas::push_clip), close/minimise dots, centred title.
    for win in &windows {
        let mut canvas = surface.canvas();
        fill_rounded_rect(&mut canvas, win.x, win.y, win.w, win.h, 10.0, win.body);
        canvas.push_clip(Rect::new(win.x as i32, win.y as i32, win.w as i32, 34));
        fill_rounded_rect(&mut canvas, win.x, win.y, win.w, win.h, 10.0, if win.focused { CHROME_FOCUS } else { CHROME });
        canvas.pop_clip();

        fill_circle(&mut canvas, win.x + 22.0, win.y + 17.0, 6.0, DANGER);
        fill_circle(&mut canvas, win.x + 42.0, win.y + 17.0, 6.0, ACCENT);

        let title_w = measure(&inter_sb, win.title, 14.0, 0.0);
        draw_text(&mut canvas, &mut cache, &inter_sb, 1, win.title, win.x + (win.w - title_w) / 2.0, win.y + 22.0, 14.0, 0.0, TEXT_INVERSE);
    }

    // 4. Content: JetBrains Mono 14px light-on-dark in the terminal window...
    {
        let mut canvas = surface.canvas();
        let w = &windows[0];
        draw_text(&mut canvas, &mut cache, &jbm, 2, "$ otter run --release", w.x + 20.0, w.y + 34.0 + 28.0, 14.0, 0.0, w.text);
        draw_text(&mut canvas, &mut cache, &jbm, 2, "booting kelp desktop...", w.x + 20.0, w.y + 34.0 + 52.0, 14.0, 0.0, ACCENT_ALT);
        draw_text(&mut canvas, &mut cache, &jbm, 2, "otterOS> _", w.x + 20.0, w.y + 34.0 + 76.0, 14.0, 0.0, w.text);
    }
    // ...dark-on-light body text (with word wrap) in the notes window...
    {
        let mut canvas = surface.canvas();
        let w = &windows[1];
        draw_text(&mut canvas, &mut cache, &inter_sb, 1, "Design notes", w.x + 20.0, w.y + 34.0 + 28.0, 16.0, 0.0, w.text);
        let body = "Own type, gradients\nand soft shadows.\nNo graphics crate.";
        for (i, line) in word_wrap(&inter, body, 13.0, w.w - 60.0).iter().enumerate() {
            draw_text(&mut canvas, &mut cache, &inter, 0, line, w.x + 20.0, w.y + 34.0 + 55.0 + i as f32 * 18.0, 13.0, 0.0, w.text);
        }
    }
    // ...and the full Inter type scale, dark-on-light, in the focused window.
    {
        let mut canvas = surface.canvas();
        let w = &windows[2];
        let mut y = w.y + 34.0 + 34.0;
        for size in [12.0f32, 14.0, 18.0, 24.0, 48.0] {
            draw_text(&mut canvas, &mut cache, &inter, 0, &format!("Aa {size:.0}px Otter"), w.x + 24.0, y, size, 0.0, w.text);
            y += size * 1.4 + 12.0;
        }
    }

    // 5. A stroked and filled vector shape, exercising the general path engine
    //    (not glyphs): a rounded blob, filled in accent.alt and outlined white.
    {
        let mut canvas = surface.canvas();
        let mut blob = Path::new();
        blob.move_to(110.0, 560.0)
            .cubic_to(170.0, 495.0, 270.0, 500.0, 305.0, 555.0)
            .cubic_to(345.0, 618.0, 265.0, 645.0, 195.0, 635.0)
            .cubic_to(135.0, 627.0, 70.0, 615.0, 110.0, 560.0)
            .close();
        fill_path(&mut canvas, &blob, Point::new(0.0, 0.0), ACCENT_ALT);
        let outline = stroke_to_fill(&blob, 4.0, 0.2);
        fill_path(&mut canvas, &outline, Point::new(0.0, 0.0), Color::WHITE);
    }

    let png_bytes = encode(&surface);
    let out_path = format!("{}/../../artifacts/gfx-specimen.png", env!("CARGO_MANIFEST_DIR"));
    std::fs::write(&out_path, &png_bytes).unwrap_or_else(|e| panic!("could not write {out_path}: {e}"));
    assert!(png_bytes.len() > 5_000, "specimen PNG suspiciously small ({} bytes)", png_bytes.len());

    // Round-trip it through our own decoder too, as a cheap end-to-end sanity
    // check that the file we just wrote is actually valid.
    let decoded = otter_gfx::png::decode(&png_bytes).expect("the specimen we just wrote must decode");
    assert_eq!((decoded.w, decoded.h), (W, H));
}
