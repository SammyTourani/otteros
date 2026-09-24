// Renders five specimen PNGs showcasing otter-ui widgets:
// 1. A form with labels, fields, checkboxes and buttons
// 2. A list of files with one selected row
// 3. A chat window mock with message bubbles
// 4. A TerminalView with shell output
// 5. A TextArea editing Rust code with line numbers

use otter_gfx::color::Color;
use otter_gfx::font::Font;
use otter_gfx::png::encode;
use otter_gfx::shapes::{fill_rect, fill_rounded_rect};
use otter_gfx::surface::Surface;
use otter_gfx::text::{draw_text, GlyphCache};
use otter_ui::*;
use otter_term::Color as TermColor;

fn load_font(rel: &str) -> Vec<u8> {
    let path = format!("{}/../../third_party/fonts/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("could not read {path} ({e}); run scripts/fetch-fonts.sh first"))
}

fn ensure_output_dir() {
    std::fs::create_dir_all("../../artifacts/ui-specimens").ok();
}

fn write_specimen(name: &str, png_bytes: &[u8]) {
    let path = format!("../../artifacts/ui-specimens/{}.png", name);
    std::fs::write(&path, png_bytes).unwrap_or_else(|e| panic!("could not write {}: {}", path, e));
    println!("Wrote {}", path);
}

#[test]
fn specimen_1_form() {
    ensure_output_dir();
    let inter_bytes = load_font("inter/Inter-Regular.ttf");
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let inter = Font::parse(&inter_bytes).expect("Inter-Regular.ttf parses");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");

    let mut surface = Surface::new(600, 500);
    let mut cache = GlyphCache::new();

    // Background: light surface
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 600.0, 500.0, theme::SURFACE);

    // Title
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter_sb,
        0,
        "User Settings",
        24.0,
        40.0,
        18.0,
        0.0,
        theme::TEXT,
    );

    // Name field with label
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Full Name",
        24.0,
        80.0,
        12.0,
        0.0,
        theme::TEXT_MUTED,
    );
    fill_rounded_rect(
        &mut surface.canvas(),
        24.0,
        95.0,
        280.0,
        40.0,
        theme::CORNER_RADIUS_WIDGET,
        Color::rgb(0xF0, 0xF0, 0xF0),
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Alice Johnson",
        36.0,
        124.0,
        14.0,
        0.0,
        theme::TEXT,
    );

    // Email field with focus ring (focused)
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Email Address",
        296.0,
        80.0,
        12.0,
        0.0,
        theme::TEXT_MUTED,
    );
    fill_rounded_rect(
        &mut surface.canvas(),
        296.0,
        92.0,
        280.0,
        44.0,
        theme::CORNER_RADIUS_WIDGET,
        theme::ACCENT,
    );
    fill_rounded_rect(
        &mut surface.canvas(),
        298.0,
        94.0,
        276.0,
        40.0,
        theme::CORNER_RADIUS_WIDGET,
        Color::rgb(0xF0, 0xF0, 0xF0),
    );

    // Draw selection highlight on "alice"
    fill_rect(&mut surface.canvas(), 308.0, 105.0, 38.0, 18.0, Color::rgba(0x3F, 0xB8, 0xAF, 90));

    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "alice@example.com",
        308.0,
        124.0,
        14.0,
        0.0,
        theme::TEXT,
    );
    // Draw cursor after selection
    fill_rect(&mut surface.canvas(), 346.0, 105.0, 2.0, 18.0, theme::ACCENT);

    // Checkbox group
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Preferences",
        24.0,
        160.0,
        12.0,
        0.0,
        theme::TEXT_MUTED,
    );

    // Checkbox 1 (unchecked)
    fill_rounded_rect(&mut surface.canvas(), 24.0, 180.0, 24.0, 24.0, 3.0, Color::rgb(0xE0, 0xE0, 0xE0));
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Receive emails",
        56.0, 199.0, 14.0, 0.0, theme::TEXT,
    );

    // Checkbox 2 (checked)
    fill_rounded_rect(&mut surface.canvas(), 24.0, 220.0, 24.0, 24.0, 3.0, Color::rgb(0xE0, 0xE0, 0xE0));
    fill_rounded_rect(&mut surface.canvas(), 28.0, 224.0, 16.0, 16.0, 2.0, theme::ACCENT);
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Marketing",
        56.0, 239.0, 14.0, 0.0, theme::TEXT,
    );

    // Checkbox 3 (unchecked)
    fill_rounded_rect(&mut surface.canvas(), 24.0, 260.0, 24.0, 24.0, 3.0, Color::rgb(0xE0, 0xE0, 0xE0));
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Notifications",
        56.0, 279.0, 14.0, 0.0, theme::TEXT,
    );

    // Calculate button text centering using font metrics
    let upm_btn = inter.units_per_em().max(1) as f32;
    let ascender_btn = inter.ascender() as f32 * 14.0 / upm_btn;
    let descender_btn = inter.descender() as f32 * 14.0 / upm_btn;
    let text_height_btn = ascender_btn - descender_btn;
    let button_y = 340.0 + (40.0 - text_height_btn) / 2.0 + ascender_btn;

    // Button row: Primary, Secondary, Danger
    fill_rounded_rect(&mut surface.canvas(), 24.0, 340.0, 100.0, 40.0, theme::CORNER_RADIUS_WIDGET, theme::ACCENT);
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Save",
        54.0, button_y, 14.0, 0.0, theme::text_on_accent(),
    );

    fill_rounded_rect(&mut surface.canvas(), 140.0, 340.0, 100.0, 40.0, theme::CORNER_RADIUS_WIDGET, Color::rgb(0xD0, 0xD0, 0xD0));
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Cancel",
        163.0, button_y, 14.0, 0.0, theme::TEXT,
    );

    fill_rounded_rect(&mut surface.canvas(), 256.0, 340.0, 100.0, 40.0, theme::CORNER_RADIUS_WIDGET, theme::DANGER);
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Delete",
        278.0, button_y, 14.0, 0.0, theme::text_on_accent(),
    );

    let png_bytes = encode(&surface);
    assert!(png_bytes.len() > 5_000);
    write_specimen("01-form", &png_bytes);
}

#[test]
fn specimen_2_list() {
    ensure_output_dir();
    let inter_bytes = load_font("inter/Inter-Regular.ttf");
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let inter = Font::parse(&inter_bytes).expect("Inter-Regular.ttf parses");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");

    let mut surface = Surface::new(400, 350);
    let mut cache = GlyphCache::new();

    // Background
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 400.0, 350.0, theme::SURFACE);

    // Title
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter_sb,
        0,
        "Files",
        24.0,
        40.0,
        16.0,
        0.0,
        theme::TEXT,
    );

    // List rows
    let items = ["README.md", "src/main.rs", "Cargo.toml", "LICENSE", "notes.txt"];
    for (i, item) in items.iter().enumerate() {
        let y = 80.0 + i as f32 * 40.0;
        let is_selected = i == 1; // Second item selected

        let bg_color = if is_selected { theme::ACCENT } else { theme::SURFACE };
        let text_color = if is_selected { theme::TEXT_INVERSE } else { theme::TEXT };

        fill_rounded_rect(&mut surface.canvas(), 24.0, y, 352.0, 36.0, theme::CORNER_RADIUS_WIDGET, bg_color);

        draw_text(
            &mut surface.canvas(),
            &mut cache,
            &inter,
            0,
            item,
            36.0,
            y + 24.0,
            14.0,
            0.0,
            text_color,
        );
    }

    let png_bytes = encode(&surface);
    assert!(png_bytes.len() > 5_000);
    write_specimen("02-list", &png_bytes);
}

#[test]
fn specimen_3_chat() {
    ensure_output_dir();
    let inter_bytes = load_font("inter/Inter-Regular.ttf");
    let inter = Font::parse(&inter_bytes).expect("Inter-Regular.ttf parses");

    let mut surface = Surface::new(500, 550);
    let mut cache = GlyphCache::new();

    // Background: dark
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 500.0, 550.0, theme::SURFACE_DARK);

    // User message bubble (right, accent bg)
    fill_rounded_rect(&mut surface.canvas(), 240.0, 40.0, 240.0, 50.0, theme::CORNER_RADIUS_WIDGET, theme::ACCENT);
    let upm = inter.units_per_em().max(1) as f32;
    let ascender = inter.ascender() as f32 * 14.0 / upm;
    let descender = inter.descender() as f32 * 14.0 / upm;
    let text_height = ascender - descender;
    let msg_y = 40.0 + (50.0 - text_height) / 2.0 + ascender;
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "What's the weather like?",
        260.0,
        msg_y,
        14.0,
        0.0,
        theme::text_on_accent(),
    );

    // Otter response bubble (left, surface.dark bg with border)
    fill_rounded_rect(&mut surface.canvas(), 20.0, 110.0, 240.0, 80.0, theme::CORNER_RADIUS_WIDGET, Color::rgb(0x1E, 0x2E, 0x35));
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "The weather is partly cloudy",
        40.0,
        142.0,
        14.0,
        0.0,
        theme::TEXT_INVERSE,
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "with a temperature of 72°F.",
        40.0,
        168.0,
        14.0,
        0.0,
        theme::TEXT_INVERSE,
    );

    // User message
    fill_rounded_rect(&mut surface.canvas(), 200.0, 220.0, 280.0, 50.0, theme::CORNER_RADIUS_WIDGET, theme::ACCENT);
    let msg2_y = 220.0 + (50.0 - text_height) / 2.0 + ascender;
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Thanks! Anything else?",
        220.0,
        msg2_y,
        14.0,
        0.0,
        theme::text_on_accent(),
    );

    // Otter response
    fill_rounded_rect(&mut surface.canvas(), 20.0, 290.0, 240.0, 70.0, theme::CORNER_RADIUS_WIDGET, Color::rgb(0x1E, 0x2E, 0x35));
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "I can help you with various",
        40.0,
        315.0,
        14.0,
        0.0,
        theme::TEXT_INVERSE,
    );
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "tasks. What would you like?",
        40.0,
        341.0,
        14.0,
        0.0,
        theme::TEXT_INVERSE,
    );

    // Input field at bottom
    fill_rounded_rect(&mut surface.canvas(), 20.0, 410.0, 380.0, 45.0, theme::CORNER_RADIUS_WIDGET, theme::SURFACE);
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Type a message...",
        40.0,
        442.0,
        14.0,
        0.0,
        theme::TEXT_MUTED,
    );

    // Send button
    fill_rounded_rect(&mut surface.canvas(), 410.0, 410.0, 70.0, 45.0, theme::CORNER_RADIUS_WIDGET, theme::ACCENT);
    let send_y = 410.0 + (45.0 - text_height) / 2.0 + ascender;
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Send",
        430.0,
        send_y,
        14.0,
        0.0,
        theme::text_on_accent(),
    );

    let png_bytes = encode(&surface);
    assert!(png_bytes.len() > 5_000);
    write_specimen("03-chat", &png_bytes);
}

#[test]
fn specimen_4_terminal() {
    ensure_output_dir();
    let jbm_bytes = load_font("jetbrains-mono/JetBrainsMono-Regular.ttf");
    let jbm = Font::parse(&jbm_bytes).expect("JetBrainsMono-Regular.ttf parses");

    // Load and feed the shell session
    let session_path = format!("{}/../../crates/otter-term/tests/fixtures/shell-session.bin", env!("CARGO_MANIFEST_DIR"));
    let session_bytes = std::fs::read(&session_path).expect("could not read shell session");

    let mut view = otter_ui::TerminalView::new(80, 24);
    view.terminal_mut().feed_bytes(&session_bytes);

    let mut surface = Surface::new(640, 400);
    let mut cache = GlyphCache::new();

    // Dark background
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 640.0, 400.0, theme::SURFACE_DARK);

    // Render terminal grid (visible portion)
    let (cols, rows) = view.terminal().dims();
    let char_width = 8.4;
    let line_height = 14.0 * 1.4;

    let visible_rows = 12.min(rows);
    let visible_cols = 76.min(cols);

    for row_idx in 0..visible_rows {
        for col_idx in 0..visible_cols {
            if let Some(cell) = view.terminal().cell_at(row_idx, col_idx) {
                let char_str = cell.ch.to_string();
                let x = 40.0 + col_idx as f32 * char_width;
                let y = 50.0 + row_idx as f32 * line_height;

                let fg_color = match cell.fg {
                    TermColor::Green | TermColor::BrightGreen => theme::OK,
                    TermColor::Cyan | TermColor::BrightCyan => theme::ACCENT_ALT,
                    _ => theme::TEXT_INVERSE,
                };

                draw_text(
                    &mut surface.canvas(),
                    &mut cache,
                    &jbm,
                    0,
                    &char_str,
                    x,
                    y,
                    14.0,
                    0.0,
                    fg_color,
                );
            }
        }
    }

    let png_bytes = encode(&surface);
    assert!(png_bytes.len() > 5_000);
    write_specimen("04-terminal", &png_bytes);
}

#[test]
fn specimen_5_textarea() {
    ensure_output_dir();
    let jbm_bytes = load_font("jetbrains-mono/JetBrainsMono-Regular.ttf");
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let jbm = Font::parse(&jbm_bytes).expect("JetBrainsMono-Regular.ttf parses");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");

    let mut surface = Surface::new(600, 450);
    let mut cache = GlyphCache::new();

    // Background
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 600.0, 450.0, theme::SURFACE);

    // Title
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter_sb,
        0,
        "Editor",
        24.0,
        30.0,
        16.0,
        0.0,
        theme::TEXT,
    );

    // Text area with focus ring
    fill_rounded_rect(&mut surface.canvas(), 20.0, 62.0, 560.0, 360.0, theme::CORNER_RADIUS_WIDGET, theme::ACCENT);
    fill_rounded_rect(&mut surface.canvas(), 22.0, 64.0, 556.0, 356.0, theme::CORNER_RADIUS_WIDGET, theme::SURFACE);

    // Line numbers and code
    let code_lines = [
        "fn main() {",
        "    println!(\"Hello\");",
        "    let x = 42;",
        "    let y = x * 2;",
        "    println!(\"{}\", y);",
        "}",
    ];

    for (i, line) in code_lines.iter().enumerate() {
        let y = 90.0 + i as f32 * 24.0;

        // Line number
        let line_num = (i + 1).to_string();
        draw_text(
            &mut surface.canvas(),
            &mut cache,
            &jbm,
            0,
            &line_num,
            32.0,
            y,
            14.0,
            0.0,
            theme::TEXT_MUTED,
        );

        // Code
        draw_text(
            &mut surface.canvas(),
            &mut cache,
            &jbm,
            0,
            line,
            70.0,
            y,
            14.0,
            0.0,
            theme::TEXT,
        );
    }

    let png_bytes = encode(&surface);
    assert!(png_bytes.len() > 5_000);
    write_specimen("05-textarea", &png_bytes);
}
