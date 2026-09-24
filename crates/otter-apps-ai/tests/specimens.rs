use otter_gfx::color::Color;
use otter_gfx::font::Font;
use otter_gfx::png::encode;
use otter_gfx::shapes::{fill_rect, fill_rounded_rect};
use otter_gfx::surface::Surface;
use otter_gfx::text::{draw_text, GlyphCache};
use otter_ui::*;

fn load_font(rel: &str) -> Vec<u8> {
    let path = format!("{}/../../third_party/fonts/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("could not read {path} ({e}); run scripts/fetch-fonts.sh first"))
}

fn ensure_output_dir() {
    std::fs::create_dir_all("artifacts/app-specimens").ok();
}

fn write_specimen(name: &str, png_bytes: &[u8]) {
    let path = format!("artifacts/app-specimens/{}.png", name);
    std::fs::write(&path, png_bytes).unwrap_or_else(|e| panic!("could not write {}: {}", path, e));
    println!("Wrote {}", path);
}

#[test]
fn specimen_otter_app() {
    ensure_output_dir();
    let inter_bytes = load_font("inter/Inter-Regular.ttf");
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let mono_bytes = load_font("jetbrains-mono/JetBrainsMono-Regular.ttf");
    let inter = Font::parse(&inter_bytes).expect("Inter-Regular.ttf parses");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");
    let mono = Font::parse(&mono_bytes).expect("JetBrainsMono parses");

    let mut surface = Surface::new(800, 600);
    let mut cache = GlyphCache::new();

    // Background: dark surface
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 800.0, 600.0, theme::SURFACE_DARK);

    // Title bar
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 800.0, 40.0, theme::CHROME_FOCUS);
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter_sb,
        0,
        "Otter",
        20.0,
        28.0,
        14.0,
        0.0,
        theme::TEXT_INVERSE,
    );

    // User message (amber bubble, right side, dark text)
    fill_rounded_rect(
        &mut surface.canvas(),
        420.0,
        60.0,
        360.0,
        60.0,
        theme::CORNER_RADIUS_WIDGET,
        theme::ACCENT,
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Write a file /data/notes/haiku.txt",
        440.0,
        85.0,
        14.0,
        0.0,
        theme::text_on_accent(),
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "with a haiku about OtterOS",
        440.0,
        105.0,
        14.0,
        0.0,
        theme::text_on_accent(),
    );

    // Assistant response bubble (dark surface, left)
    fill_rounded_rect(
        &mut surface.canvas(),
        20.0,
        140.0,
        360.0,
        80.0,
        theme::CORNER_RADIUS_WIDGET,
        Color::rgb(0x1E, 0x2E, 0x35),
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "I'll write a haiku to /data/notes/haiku.txt.",
        40.0,
        165.0,
        14.0,
        0.0,
        theme::TEXT_INVERSE,
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "This requires Allow permission.",
        40.0,
        190.0,
        14.0,
        0.0,
        theme::TEXT_INVERSE,
    );

    // Tool call card (Allow/Deny)
    fill_rounded_rect(
        &mut surface.canvas(),
        20.0,
        240.0,
        760.0,
        120.0,
        theme::CORNER_RADIUS_WIDGET,
        Color::rgb(0x1A, 0x24, 0x2C),
    );

    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter_sb,
        0,
        "Tool Call: write_file",
        40.0,
        265.0,
        14.0,
        0.0,
        theme::TEXT_INVERSE,
    );

    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &mono,
        0,
        "/data/notes/haiku.txt",
        40.0,
        290.0,
        12.0,
        0.0,
        theme::TEXT_MUTED,
    );

    // Allow button (green)
    fill_rounded_rect(
        &mut surface.canvas(),
        40.0,
        310.0,
        100.0,
        35.0,
        theme::CORNER_RADIUS_WIDGET,
        theme::OK,
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Allow",
        60.0,
        335.0,
        12.0,
        0.0,
        theme::TEXT,
    );

    // Deny button (red)
    fill_rounded_rect(
        &mut surface.canvas(),
        160.0,
        310.0,
        100.0,
        35.0,
        theme::CORNER_RADIUS_WIDGET,
        theme::DANGER,
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Deny",
        180.0,
        335.0,
        12.0,
        0.0,
        theme::text_on_accent(),
    );

    // Composer at bottom
    fill_rounded_rect(
        &mut surface.canvas(),
        20.0,
        495.0,
        760.0,
        50.0,
        theme::CORNER_RADIUS_WIDGET,
        theme::SURFACE,
    );

    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Type your message here...",
        40.0,
        523.0,
        14.0,
        0.0,
        theme::TEXT_MUTED,
    );

    let png_bytes = encode(&surface);
    assert!(png_bytes.len() > 5_000);
    write_specimen("otter-app", &png_bytes);
}

#[test]
fn specimen_otter_app_local() {
    ensure_output_dir();
    let inter_bytes = load_font("inter/Inter-Regular.ttf");
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let inter = Font::parse(&inter_bytes).expect("Inter-Regular.ttf parses");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");

    let mut surface = Surface::new(800, 600);
    let mut cache = GlyphCache::new();

    // Background
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 800.0, 600.0, theme::SURFACE_DARK);

    // Title bar with "Local Otter" badge
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 800.0, 40.0, theme::CHROME_FOCUS);
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter_sb,
        0,
        "Otter (Local)",
        20.0,
        28.0,
        14.0,
        0.0,
        theme::TEXT_INVERSE,
    );

    // Streaming indicator with tokens/sec
    fill_rounded_rect(
        &mut surface.canvas(),
        20.0,
        60.0,
        200.0,
        30.0,
        theme::CORNER_RADIUS_WIDGET,
        Color::rgb(0x2C, 0x7A, 0x7B),
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Streaming: 42 tokens/s",
        40.0,
        82.0,
        12.0,
        0.0,
        theme::TEXT_INVERSE,
    );

    // Assistant response with streaming
    fill_rounded_rect(
        &mut surface.canvas(),
        20.0,
        110.0,
        360.0,
        200.0,
        theme::CORNER_RADIUS_WIDGET,
        Color::rgb(0x1E, 0x2E, 0x35),
    );

    let response_lines = vec![
        "The morning dew glistens,",
        "On silicon and waves of light,",
        "Otter swims through code.",
    ];
    let mut y = 135.0;
    for line in response_lines {
        draw_text(
            &mut surface.canvas(),
            &mut cache,
            &inter,
            0,
            line,
            40.0,
            y,
            14.0,
            0.0,
            theme::TEXT_INVERSE,
        );
        y += 20.0;
    }

    // Stop button
    fill_rounded_rect(
        &mut surface.canvas(),
        20.0,
        320.0,
        80.0,
        35.0,
        theme::CORNER_RADIUS_WIDGET,
        theme::DANGER,
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Stop",
        38.0,
        344.0,
        12.0,
        0.0,
        theme::text_on_accent(),
    );

    // Action log
    fill_rounded_rect(
        &mut surface.canvas(),
        20.0,
        370.0,
        760.0,
        90.0,
        theme::CORNER_RADIUS_WIDGET,
        Color::rgb(0x1A, 0x24, 0x2C),
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter_sb,
        0,
        "Action Log",
        40.0,
        390.0,
        12.0,
        0.0,
        theme::TEXT_MUTED,
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "[✓] read_file(/data) - completed",
        40.0,
        410.0,
        11.0,
        0.0,
        theme::TEXT_INVERSE,
    );

    let png_bytes = encode(&surface);
    assert!(png_bytes.len() > 5_000);
    write_specimen("otter-app-local", &png_bytes);
}

#[test]
fn specimen_files_app() {
    ensure_output_dir();
    let inter_bytes = load_font("inter/Inter-Regular.ttf");
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let inter = Font::parse(&inter_bytes).expect("Inter-Regular.ttf parses");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");

    let mut surface = Surface::new(800, 600);
    let mut cache = GlyphCache::new();

    // Background: light surface
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 800.0, 600.0, theme::SURFACE);

    // Title bar
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 800.0, 40.0, theme::CHROME_FOCUS);
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter_sb,
        0,
        "Files - /data",
        20.0,
        28.0,
        14.0,
        0.0,
        theme::TEXT_INVERSE,
    );

    // Path bar
    fill_rounded_rect(
        &mut surface.canvas(),
        20.0,
        60.0,
        760.0,
        30.0,
        theme::CORNER_RADIUS_WIDGET,
        Color::rgb(0xF0, 0xF0, 0xF0),
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "/data",
        40.0,
        80.0,
        12.0,
        0.0,
        theme::TEXT,
    );

    // Column headers (centered and styled)
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Name",
        40.0,
        110.0,
        12.0,
        0.0,
        theme::TEXT_MUTED,
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Size",
        400.0,
        110.0,
        12.0,
        0.0,
        theme::TEXT_MUTED,
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Modified",
        550.0,
        110.0,
        12.0,
        0.0,
        theme::TEXT_MUTED,
    );

    // File list with dark text on amber selection (selected = notes.txt)
    let files = [
        ("README.md", "2048"),
        ("src", "-"),
        ("notes.txt", "512"),
        ("config.toml", "256"),
    ];

    for (i, (name, size)) in files.iter().enumerate() {
        let y = 130.0 + i as f32 * 36.0;
        let is_selected = i == 2;

        let bg_color = if is_selected { theme::ACCENT } else { theme::SURFACE };
        let text_color = if is_selected {
            theme::text_on_accent()
        } else {
            theme::TEXT
        };

        fill_rounded_rect(
            &mut surface.canvas(),
            20.0,
            y,
            760.0,
            32.0,
            theme::CORNER_RADIUS_WIDGET,
            bg_color,
        );

        draw_text(
            &mut surface.canvas(),
            &mut cache,
            &inter,
            0,
            name,
            40.0,
            y + 20.0,
            14.0,
            0.0,
            text_color,
        );

        draw_text(
            &mut surface.canvas(),
            &mut cache,
            &inter,
            0,
            size,
            400.0,
            y + 20.0,
            14.0,
            0.0,
            text_color,
        );
    }

    let png_bytes = encode(&surface);
    assert!(png_bytes.len() > 5_000);
    write_specimen("files-app", &png_bytes);
}
