//! Renders specimens of the window manager per docs/DESIGN.md: (1) three overlapping windows
//! with taskbar, (2) maximised window, (3) mid-drag state. Window content, icons, cursor and
//! shadows all drawn with otter-gfx.

use otter_gfx::color::Color;
use otter_gfx::font::Font;
use otter_gfx::geom::{Point, Rect};
use otter_gfx::gradient::{fill_radial_gradient, fill_linear_gradient, ColorStop};
use otter_gfx::path::Path;
use otter_gfx::png::encode;
use otter_gfx::raster::fill_path;
use otter_gfx::shadow::{build_shadow_mask, draw_shadow_mask};
use otter_gfx::shapes::{fill_circle, fill_rounded_rect};
use otter_gfx::surface::Surface;
use otter_gfx::text::{draw_text, measure, GlyphCache};
use otter_wm::design;

fn load_font(rel: &str) -> Vec<u8> {
    let path = format!("{}/../../third_party/fonts/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("could not read {path} ({e}); run scripts/fetch-fonts.sh first"))
}

fn draw_procedural_icon(canvas: &mut otter_gfx::surface::Canvas, x: f32, y: f32, icon_type: &str) {
    match icon_type {
        "terminal" => {
            fill_rounded_rect(canvas, x, y, 40.0, 40.0, 6.0, design::SURFACE_DARK);
            fill_rounded_rect(canvas, x + 8.0, y + 16.0, 24.0, 1.5, 0.5, design::ACCENT);
            fill_rounded_rect(canvas, x + 8.0, y + 22.0, 18.0, 1.5, 0.5, design::ACCENT);
        }
        "editor" => {
            fill_rounded_rect(canvas, x, y, 40.0, 40.0, 6.0, design::SURFACE);
            for i in 0..3 {
                let line_y = y + 12.0 + i as f32 * 8.0;
                let line_width = if i == 0 { 24.0 } else { 20.0 };
                fill_rounded_rect(canvas, x + 8.0, line_y, line_width, 1.5, 0.5, design::TEXT);
            }
        }
        "chat" => {
            fill_rounded_rect(canvas, x, y, 40.0, 40.0, 6.0, design::ACCENT_ALT);
            let mut chat_path = Path::new();
            chat_path.move_to(x + 10.0, y + 12.0);
            chat_path.line_to(x + 30.0, y + 12.0);
            chat_path.line_to(x + 30.0, y + 26.0);
            chat_path.line_to(x + 20.0, y + 26.0);
            chat_path.line_to(x + 20.0, y + 32.0);
            chat_path.line_to(x + 10.0, y + 26.0);
            chat_path.close();
            fill_path(canvas, &chat_path, Point::new(0.0, 0.0), design::TEXT_INVERSE);
        }
        "files" => {
            fill_rounded_rect(canvas, x, y, 40.0, 40.0, 6.0, design::TEXT);
            fill_rounded_rect(canvas, x + 8.0, y + 12.0, 24.0, 20.0, 2.0, design::CHROME);
        }
        _ => {
            fill_rounded_rect(canvas, x, y, 40.0, 40.0, 6.0, design::CHROME);
        }
    }
}

fn draw_arrow_cursor(frame: &mut Surface, x: f32, y: f32) {
    let mut cursor_path = Path::new();
    cursor_path
        .move_to(x, y)
        .line_to(x + 6.0, y + 10.0)
        .line_to(x + 3.0, y + 10.0)
        .line_to(x + 2.0, y + 18.0)
        .line_to(x - 1.0, y + 15.0)
        .line_to(x - 4.0, y + 15.0)
        .line_to(x, y + 7.0)
        .line_to(x - 3.0, y + 7.0)
        .close();

    let mut canvas = frame.canvas();
    fill_path(&mut canvas, &cursor_path, Point::new(0.0, 0.0), design::TEXT);

    let outline = otter_gfx::path::stroke_to_fill(&cursor_path, 1.2, 0.2);
    fill_path(&mut canvas, &outline, Point::new(0.0, 0.0), Color::WHITE);
}

#[test]
fn renders_desktop_specimen() {
    let inter_bytes = load_font("inter/Inter-Regular.ttf");
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let jbm_bytes = load_font("jetbrains-mono/JetBrainsMono-Regular.ttf");
    let inter = Font::parse(&inter_bytes).expect("Inter-Regular.ttf parses");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");
    let jbm = Font::parse(&jbm_bytes).expect("JetBrainsMono-Regular.ttf parses");

    const W: usize = 1920;
    const H: usize = 1080;
    let mut frame = Surface::new(W, H);
    let mut cache = GlyphCache::new();

    {
        let mut canvas = frame.canvas();
        fill_linear_gradient(
            &mut canvas,
            Point::new(0.0, 0.0),
            Point::new(0.0, H as f32),
            &[ColorStop::new(0.0, design::BG_TOP), ColorStop::new(1.0, design::BG_BOTTOM)],
        );
        fill_radial_gradient(
            &mut canvas,
            Point::new(160.0, 140.0),
            480.0,
            &[ColorStop::new(0.0, design::GLOW), ColorStop::new(1.0, design::GLOW_FADE)],
        );
    }

    let windows = [
        ("terminal", 80.0, 120.0, 650.0, 450.0, false),
        ("editor", 320.0, 220.0, 700.0, 500.0, false),
        ("chat", 600.0, 320.0, 750.0, 580.0, true),
    ];

    for (_title, x, y, w, h, focused) in &windows {
        let (offset_y, blur, alpha): (f32, f32, f32) =
            if *focused { (12.0, 32.0, 0.45) } else { (6.0, 18.0, 0.30) };

        let mask = build_shadow_mask(*w, *h, design::CORNER_RADIUS, blur, 0.0);
        draw_shadow_mask(
            &mut frame.canvas(),
            &mask,
            *x,
            *y + offset_y,
            Color::rgba(0, 0, 0, f32_round(alpha * 255.0) as u8),
        );
    }

    for (title, x, y, w, h, focused) in &windows {
        let body_color = if *title == "chat" { design::SURFACE_DARK } else { design::SURFACE };
        let text_color = if *title == "chat" { design::TEXT_INVERSE } else { design::TEXT };

        let mut canvas = frame.canvas();
        fill_rounded_rect(&mut canvas, *x, *y, *w, *h, design::CORNER_RADIUS, body_color);

        canvas.push_clip(Rect::new(*x as i32, *y as i32, *w as i32, 34));
        let chrome_color = if *focused { design::CHROME_FOCUS } else { design::CHROME };
        fill_rounded_rect(&mut canvas, *x, *y, *w, *h, design::CORNER_RADIUS, chrome_color);
        canvas.pop_clip();

        fill_circle(&mut canvas, *x + 22.0, *y + 17.0, 6.0, design::DANGER);
        fill_circle(&mut canvas, *x + 42.0, *y + 17.0, 6.0, design::ACCENT);

        let title_w = measure(&inter_sb, title, 14.0, 0.0);
        draw_text(
            &mut canvas,
            &mut cache,
            &inter_sb,
            1,
            title,
            *x + (*w - title_w) / 2.0,
            *y + 22.0,
            14.0,
            0.0,
            design::TEXT_INVERSE,
        );

        if *title == "terminal" {
            let mut canvas = frame.canvas();
            draw_text(&mut canvas, &mut cache, &jbm, 2, "$ otter run", *x + 20.0, *y + 70.0, 13.0, 0.0, text_color);
            draw_text(&mut canvas, &mut cache, &jbm, 2, "booting...", *x + 20.0, *y + 90.0, 13.0, 0.0, design::ACCENT);
            draw_text(&mut canvas, &mut cache, &jbm, 2, "> _", *x + 20.0, *y + 110.0, 13.0, 0.0, text_color);
        } else if *title == "editor" {
            let mut canvas = frame.canvas();
            draw_text(&mut canvas, &mut cache, &inter_sb, 1, "main.rs", *x + 20.0, *y + 70.0, 14.0, 0.0, text_color);
            draw_text(&mut canvas, &mut cache, &jbm, 2, "fn main() {", *x + 20.0, *y + 95.0, 12.0, 0.0, text_color);
            draw_text(&mut canvas, &mut cache, &jbm, 2, "  println!(\"Hello\");", *x + 20.0, *y + 115.0, 12.0, 0.0, text_color);
            draw_text(&mut canvas, &mut cache, &jbm, 2, "}", *x + 20.0, *y + 135.0, 12.0, 0.0, text_color);
        } else if *title == "chat" {
            let mut canvas = frame.canvas();
            fill_rounded_rect(&mut canvas, *x + 20.0, *y + 70.0, 200.0, 40.0, 8.0, design::ACCENT_ALT);
            draw_text(&mut canvas, &mut cache, &inter, 0, "Hello, how can I help?", *x + 28.0, *y + 85.0, 12.0, 0.0, design::TEXT);
            fill_rounded_rect(&mut canvas, *x + 300.0, *y + 130.0, 150.0, 40.0, 8.0, design::ACCENT);
            draw_text(&mut canvas, &mut cache, &inter, 0, "Tell me a fact", *x + 308.0, *y + 145.0, 12.0, 0.0, design::TEXT_INVERSE);
        }
    }

    {
        let mut canvas = frame.canvas();
        let taskbar_y = H as f32 - design::TASKBAR_HEIGHT - design::TASKBAR_PADDING;
        fill_rounded_rect(
            &mut canvas,
            design::TASKBAR_PADDING,
            taskbar_y,
            W as f32 - 2.0 * design::TASKBAR_PADDING,
            design::TASKBAR_HEIGHT,
            design::TASKBAR_CORNER_RADIUS,
            design::CHROME,
        );

        fill_circle(&mut canvas, design::TASKBAR_PADDING + 20.0, taskbar_y + 20.0, 8.0, design::ACCENT);
        draw_procedural_icon(&mut canvas, design::TASKBAR_PADDING + 50.0, taskbar_y + 5.0, "terminal");
        draw_procedural_icon(&mut canvas, design::TASKBAR_PADDING + 100.0, taskbar_y + 5.0, "editor");
        draw_procedural_icon(&mut canvas, design::TASKBAR_PADDING + 150.0, taskbar_y + 5.0, "chat");
        draw_procedural_icon(&mut canvas, design::TASKBAR_PADDING + 200.0, taskbar_y + 5.0, "files");

        let clock_w = measure(&inter_sb, "14:32", 14.0, 0.0);
        draw_text(
            &mut canvas,
            &mut cache,
            &inter_sb,
            1,
            "14:32",
            W as f32 - design::TASKBAR_PADDING - clock_w - 16.0,
            taskbar_y + 26.0,
            14.0,
            0.0,
            design::TEXT_INVERSE,
        );
    }

    draw_arrow_cursor(&mut frame, 960.0, 540.0);

    let png_bytes = encode(&frame);
    let out_dir = format!("{}/../../artifacts/wm-specimens", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&out_dir).unwrap_or_else(|e| panic!("could not create dir {out_dir}: {e}"));

    let out_path = format!("{}/1-desktop.png", out_dir);
    std::fs::write(&out_path, &png_bytes).unwrap_or_else(|e| panic!("could not write {out_path}: {e}"));
    assert!(png_bytes.len() > 10_000, "PNG suspiciously small ({} bytes)", png_bytes.len());

    let decoded = otter_gfx::png::decode(&png_bytes).expect("the PNG we just wrote must decode");
    assert_eq!((decoded.w, decoded.h), (W, H));
}

#[test]
fn renders_maximised_specimen() {
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");

    const W: usize = 1920;
    const H: usize = 1080;
    let mut frame = Surface::new(W, H);
    let mut cache = GlyphCache::new();

    {
        let mut canvas = frame.canvas();
        fill_linear_gradient(
            &mut canvas,
            Point::new(0.0, 0.0),
            Point::new(0.0, H as f32),
            &[ColorStop::new(0.0, design::BG_TOP), ColorStop::new(1.0, design::BG_BOTTOM)],
        );
        fill_radial_gradient(
            &mut canvas,
            Point::new(160.0, 140.0),
            480.0,
            &[ColorStop::new(0.0, design::GLOW), ColorStop::new(1.0, design::GLOW_FADE)],
        );
    }

    let max_h = H as f32 - design::TASKBAR_HEIGHT - design::TASKBAR_PADDING;
    let (offset_y, blur, alpha): (f32, f32, f32) = (12.0, 32.0, 0.45);

    let mask = build_shadow_mask(W as f32, max_h, design::CORNER_RADIUS, blur, 0.0);
    draw_shadow_mask(
        &mut frame.canvas(),
        &mask,
        0.0,
        offset_y,
        Color::rgba(0, 0, 0, f32_round(alpha * 255.0) as u8),
    );

    let mut canvas = frame.canvas();
    fill_rounded_rect(&mut canvas, 0.0, 0.0, W as f32, max_h, design::CORNER_RADIUS, design::SURFACE);

    canvas.push_clip(Rect::new(0, 0, W as i32, 34));
    fill_rounded_rect(
        &mut canvas,
        0.0,
        0.0,
        W as f32,
        max_h,
        design::CORNER_RADIUS,
        design::CHROME_FOCUS,
    );
    canvas.pop_clip();

    fill_circle(&mut canvas, 22.0, 17.0, 6.0, design::DANGER);
    fill_circle(&mut canvas, 42.0, 17.0, 6.0, design::ACCENT);

    let title_w = measure(&inter_sb, "maximised", 14.0, 0.0);
    draw_text(
        &mut canvas,
        &mut cache,
        &inter_sb,
        1,
        "maximised",
        (W as f32 - title_w) / 2.0,
        22.0,
        14.0,
        0.0,
        design::TEXT_INVERSE,
    );

    {
        let mut canvas = frame.canvas();
        let taskbar_y = H as f32 - design::TASKBAR_HEIGHT - design::TASKBAR_PADDING;
        fill_rounded_rect(
            &mut canvas,
            design::TASKBAR_PADDING,
            taskbar_y,
            W as f32 - 2.0 * design::TASKBAR_PADDING,
            design::TASKBAR_HEIGHT,
            design::TASKBAR_CORNER_RADIUS,
            design::CHROME,
        );
    }

    let png_bytes = encode(&frame);
    let out_dir = format!("{}/../../artifacts/wm-specimens", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&out_dir).unwrap_or_else(|e| panic!("could not create dir {out_dir}: {e}"));

    let out_path = format!("{}/2-maximised.png", out_dir);
    std::fs::write(&out_path, &png_bytes).unwrap_or_else(|e| panic!("could not write {out_path}: {e}"));
    assert!(png_bytes.len() > 10_000, "PNG suspiciously small ({} bytes)", png_bytes.len());

    let decoded = otter_gfx::png::decode(&png_bytes).expect("the PNG we just wrote must decode");
    assert_eq!((decoded.w, decoded.h), (W, H));
}

#[test]
fn renders_drag_state_specimen() {
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");

    const W: usize = 1920;
    const H: usize = 1080;
    let mut frame = Surface::new(W, H);
    let mut cache = GlyphCache::new();

    {
        let mut canvas = frame.canvas();
        fill_linear_gradient(
            &mut canvas,
            Point::new(0.0, 0.0),
            Point::new(0.0, H as f32),
            &[ColorStop::new(0.0, design::BG_TOP), ColorStop::new(1.0, design::BG_BOTTOM)],
        );
        fill_radial_gradient(
            &mut canvas,
            Point::new(160.0, 140.0),
            480.0,
            &[ColorStop::new(0.0, design::GLOW), ColorStop::new(1.0, design::GLOW_FADE)],
        );
    }

    let windows = [
        ("terminal", 200.0, 150.0, 650.0, 450.0, false),
        ("moving", 400.0, 320.0, 700.0, 500.0, true),
        ("background", 700.0, 450.0, 600.0, 400.0, false),
    ];

    for (_title, x, y, w, h, focused) in &windows {
        let (offset_y, blur, alpha): (f32, f32, f32) =
            if *focused { (12.0, 32.0, 0.45) } else { (6.0, 18.0, 0.30) };

        let mask = build_shadow_mask(*w, *h, design::CORNER_RADIUS, blur, 0.0);
        draw_shadow_mask(
            &mut frame.canvas(),
            &mask,
            *x,
            *y + offset_y,
            Color::rgba(0, 0, 0, f32_round(alpha * 255.0) as u8),
        );
    }

    for (title, x, y, w, h, focused) in &windows {
        let body_color = if *title == "background" { design::SURFACE_DARK } else { design::SURFACE };
        let _text_color = if *title == "background" { design::TEXT_INVERSE } else { design::TEXT };

        let mut canvas = frame.canvas();
        fill_rounded_rect(&mut canvas, *x, *y, *w, *h, design::CORNER_RADIUS, body_color);

        canvas.push_clip(Rect::new(*x as i32, *y as i32, *w as i32, 34));
        let chrome_color = if *focused { design::CHROME_FOCUS } else { design::CHROME };
        fill_rounded_rect(&mut canvas, *x, *y, *w, *h, design::CORNER_RADIUS, chrome_color);
        canvas.pop_clip();

        fill_circle(&mut canvas, *x + 22.0, *y + 17.0, 6.0, design::DANGER);
        fill_circle(&mut canvas, *x + 42.0, *y + 17.0, 6.0, design::ACCENT);

        let title_w = measure(&inter_sb, title, 14.0, 0.0);
        draw_text(
            &mut canvas,
            &mut cache,
            &inter_sb,
            1,
            title,
            *x + (*w - title_w) / 2.0,
            *y + 22.0,
            14.0,
            0.0,
            design::TEXT_INVERSE,
        );
    }

    {
        let mut canvas = frame.canvas();
        let taskbar_y = H as f32 - design::TASKBAR_HEIGHT - design::TASKBAR_PADDING;
        fill_rounded_rect(
            &mut canvas,
            design::TASKBAR_PADDING,
            taskbar_y,
            W as f32 - 2.0 * design::TASKBAR_PADDING,
            design::TASKBAR_HEIGHT,
            design::TASKBAR_CORNER_RADIUS,
            design::CHROME,
        );
    }

    draw_arrow_cursor(&mut frame, 450.0, 330.0);

    let png_bytes = encode(&frame);
    let out_dir = format!("{}/../../artifacts/wm-specimens", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&out_dir).unwrap_or_else(|e| panic!("could not create dir {out_dir}: {e}"));

    let out_path = format!("{}/3-drag.png", out_dir);
    std::fs::write(&out_path, &png_bytes).unwrap_or_else(|e| panic!("could not write {out_path}: {e}"));
    assert!(png_bytes.len() > 10_000, "PNG suspiciously small ({} bytes)", png_bytes.len());

    let decoded = otter_gfx::png::decode(&png_bytes).expect("the PNG we just wrote must decode");
    assert_eq!((decoded.w, decoded.h), (W, H));
}

fn f32_round(x: f32) -> f32 {
    if x >= 0.0 {
        (x + 0.5) as i32 as f32
    } else {
        (x - 0.5) as i32 as f32
    }
}
