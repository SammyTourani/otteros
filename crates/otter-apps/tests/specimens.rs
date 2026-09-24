// Renders specimen PNGs showcasing otter-apps:
// 1. terminal-app.png: a terminal emulator showing shell output
// 2. editor-app.png: a text editor with Rust code, selection, and find bar

use otter_gfx::color::Color;
use otter_gfx::font::Font;
use otter_gfx::png::encode;
use otter_gfx::shapes::{fill_rect, fill_rounded_rect};
use otter_gfx::surface::Surface;
use otter_gfx::text::{draw_text, GlyphCache};
use otter_term::Color as TermColor;
use std::format;

fn load_font(rel: &str) -> Vec<u8> {
    let path = format!("{}/../../third_party/fonts/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("could not read {path} ({e}); run scripts/fetch-fonts.sh first"))
}

fn ensure_output_dir() {
    std::fs::create_dir_all("../../artifacts/app-specimens").ok();
}

fn write_specimen(name: &str, png_bytes: &[u8]) {
    let path = format!("../../artifacts/app-specimens/{}.png", name);
    std::fs::write(&path, png_bytes).unwrap_or_else(|e| panic!("could not write {}: {}", path, e));
    println!("Wrote {}", path);
}

/// Color palette for terminal output
fn term_color_to_gfx(term_color: TermColor) -> Color {
    match term_color {
        TermColor::Black => Color::rgb(0x00, 0x00, 0x00),
        TermColor::Red => Color::rgb(0xCC, 0x00, 0x00),
        TermColor::Green => Color::rgb(0x00, 0xCC, 0x00),
        TermColor::Yellow => Color::rgb(0xCC, 0xCC, 0x00),
        TermColor::Blue => Color::rgb(0x00, 0x00, 0xEE),
        TermColor::Magenta => Color::rgb(0xCC, 0x00, 0xCC),
        TermColor::Cyan => Color::rgb(0x00, 0xCC, 0xCC),
        TermColor::White => Color::rgb(0xE5, 0xE5, 0xE5),
        TermColor::BrightBlack => Color::rgb(0x66, 0x66, 0x66),
        TermColor::BrightRed => Color::rgb(0xFF, 0x33, 0x33),
        TermColor::BrightGreen => Color::rgb(0x33, 0xFF, 0x33),
        TermColor::BrightYellow => Color::rgb(0xFF, 0xFF, 0x33),
        TermColor::BrightBlue => Color::rgb(0x33, 0x33, 0xFF),
        TermColor::BrightMagenta => Color::rgb(0xFF, 0x33, 0xFF),
        TermColor::BrightCyan => Color::rgb(0x33, 0xFF, 0xFF),
        TermColor::BrightWhite => Color::rgb(0xFF, 0xFF, 0xFF),
        TermColor::Rgb(r, g, b) => Color::rgb(r, g, b),
    }
}

#[test]
fn specimen_terminal_app() {
    ensure_output_dir();

    let jetbrains_bytes = load_font("jetbrains-mono/JetBrainsMono-Regular.ttf");
    let jetbrains = Font::parse(&jetbrains_bytes).expect("JetBrainsMono-Regular.ttf parses");

    // Create terminal and feed some test data
    let mut terminal = otter_term::Terminal::new(80, 24);
    terminal.set_newline_mode(true);

    // Simulate shell session output
    let shell_output = b"$ echo 'OtterOS Terminal Demo'\nOtterOS Terminal Demo\n$ ls -la\n\
        total 42\ndrwxr-xr-x  5 user  staff  160 Sep 24 12:00 .\n\
        drwxr-xr-x  8 user  staff  256 Sep 24 12:00 ..\n\
        -rw-r--r--  1 user  staff 1234 Sep 24 10:00 main.rs\n\
        -rw-r--r--  1 user  staff  567 Sep 24 10:00 lib.rs\n\
        $ cargo build\n   Compiling otter-apps v0.1.0\n    Finished release [optimized]";

    terminal.feed_bytes(shell_output);

    // Render to a 640x480 surface
    let mut surface = Surface::new(640, 480);
    let mut canvas = surface.canvas();

    // Background: dark terminal color from DESIGN.md
    fill_rect(&mut canvas, 0.0, 0.0, 640.0, 480.0, Color::rgb(0x12, 0x1A, 0x1E));

    let char_width = 8.0;
    let char_height = 16.0;
    let mut glyph_cache = GlyphCache::new();

    // Render terminal grid
    let (cols, rows) = terminal.dims();
    for row in 0..rows {
        for col in 0..cols {
            if let Some(cell) = terminal.cell_at(row, col) {
                let x = col as f32 * char_width;
                let y = row as f32 * char_height;

                // Draw background for cell
                let bg = term_color_to_gfx(cell.bg);
                fill_rect(&mut canvas, x, y, char_width, char_height, bg);

                // Draw text
                let fg = term_color_to_gfx(cell.fg);
                let ch = cell.ch.to_string();
                draw_text(
                    &mut canvas,
                    &mut glyph_cache,
                    &jetbrains,
                    0,
                    &ch,
                    x + 1.0,
                    y + 12.0,
                    14.0,
                    0.0,
                    fg,
                );
            }
        }
    }

    // Add title bar text
    fill_rect(&mut canvas, 0.0, 0.0, 640.0, 34.0, Color::rgba(0x22, 0x2E, 0x34, 245));
    draw_text(
        &mut canvas,
        &mut glyph_cache,
        &jetbrains,
        0,
        "Terminal",
        320.0,
        22.0,
        14.0,
        0.0,
        Color::rgb(0xE8, 0xEC, 0xEE),
    );

    let png = encode(&surface);
    write_specimen("terminal-app", &png);
}

#[test]
fn specimen_editor_app() {
    ensure_output_dir();

    let inter_bytes = load_font("inter/Inter-Regular.ttf");
    let inter_sb_bytes = load_font("inter/Inter-SemiBold.ttf");
    let jetbrains_bytes = load_font("jetbrains-mono/JetBrainsMono-Regular.ttf");

    let inter = Font::parse(&inter_bytes).expect("Inter-Regular.ttf parses");
    let inter_sb = Font::parse(&inter_sb_bytes).expect("Inter-SemiBold.ttf parses");
    let jetbrains = Font::parse(&jetbrains_bytes).expect("JetBrainsMono-Regular.ttf parses");

    let mut surface = Surface::new(800, 600);
    let mut cache = GlyphCache::new();

    // Background: light surface color
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 800.0, 600.0, Color::rgb(0xF6, 0xF4, 0xF0));

    // Title bar: 34px at top
    fill_rect(&mut surface.canvas(), 0.0, 0.0, 800.0, 34.0, Color::rgba(0x22, 0x2E, 0x34, 245));
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter_sb,
        0,
        "example.rs ●",
        20.0,
        22.0,
        14.0,
        0.0,
        Color::rgb(0xE8, 0xEC, 0xEE),
    );

    // Status bar: 20px at very bottom (chrome background)
    let status_bar_y = 580.0;
    fill_rect(&mut surface.canvas(), 0.0, status_bar_y, 800.0, 20.0, Color::rgba(0x1B, 0x24, 0x29, 235));
    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Ln 2, Col 12 | UTF-8 | LF",
        20.0,
        591.0,
        12.0,
        0.0,
        Color::rgb(0xE8, 0xEC, 0xEE),
    );

    // Find bar: 50px directly above status bar (optional, when open)
    let find_bar_y = 530.0;
    fill_rect(&mut surface.canvas(), 0.0, find_bar_y, 800.0, 50.0, Color::rgb(0xFF, 0xFF, 0xFF));

    fill_rounded_rect(
        &mut surface.canvas(),
        20.0,
        find_bar_y + 8.0,
        300.0,
        34.0,
        6.0,
        Color::rgb(0xF0, 0xF0, 0xF0),
    );

    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "Find:",
        30.0,
        find_bar_y + 24.0,
        14.0,
        0.0,
        Color::rgb(0x1E, 0x22, 0x26),
    );

    draw_text(
        &mut surface.canvas(),
        &mut cache,
        &inter,
        0,
        "println",
        100.0,
        find_bar_y + 24.0,
        14.0,
        0.0,
        Color::rgb(0x1E, 0x22, 0x26),
    );

    // Editor area with line numbers and code (shrinks to fit find bar + status bar)
    let line_height = 20.0;
    let left_margin = 40.0;
    let top_margin = 50.0;

    let code_lines = [
        "fn main() {",
        "    let x = 42;",
        "    println!(\"Hello {}\", x);",
        "}",
    ];

    for (i, line) in code_lines.iter().enumerate() {
        let y = top_margin + i as f32 * line_height;
        if y + line_height > find_bar_y {
            break; // Stop rendering if we hit the find bar
        }

        // Line number
        let line_num = format!("{}", i + 1);
        draw_text(
            &mut surface.canvas(),
            &mut cache,
            &jetbrains,
            0,
            &line_num,
            10.0,
            y + 14.0,
            14.0,
            0.0,
            Color::rgb(0x8A, 0x96, 0x9C),
        );

        // Code
        draw_text(
            &mut surface.canvas(),
            &mut cache,
            &jetbrains,
            0,
            line,
            left_margin,
            y + 14.0,
            14.0,
            0.0,
            Color::rgb(0x1E, 0x22, 0x26),
        );
    }

    // Selection highlight (on second line, around "let x")
    fill_rounded_rect(
        &mut surface.canvas(),
        left_margin,
        top_margin + 1.0 * line_height,
        50.0,
        line_height,
        2.0,
        Color::rgba(0x3F, 0xB8, 0xAF, 60),
    );

    let png = encode(&surface);
    write_specimen("editor-app", &png);
}
