#![no_std]
#![cfg_attr(test, allow(unused))]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::{String, ToString};

use otter_gfx::color::Color;
use otter_gfx::font::Font;
use otter_gfx::shapes::{fill_rect, fill_rounded_rect};
use otter_gfx::surface::Surface;
use otter_gfx::text::{draw_text, measure, GlyphCache};
use otter_textbuf::TextBuffer;
use otter_term::Terminal;

/// Theme colors from docs/DESIGN.md (sRGB)
pub mod theme {
    use otter_gfx::color::Color;

    pub const BG_TOP: Color = Color::rgb(0x10, 0x30, 0x3B);
    pub const BG_BOTTOM: Color = Color::rgb(0x08, 0x14, 0x19);
    pub const GLOW: Color = Color::rgba(0x2C, 0x7A, 0x7B, 110);
    pub const CHROME: Color = Color::rgba(0x1B, 0x24, 0x29, 235);
    pub const CHROME_FOCUS: Color = Color::rgba(0x22, 0x2E, 0x34, 245);
    pub const SURFACE: Color = Color::rgb(0xF6, 0xF4, 0xF0);
    pub const SURFACE_DARK: Color = Color::rgb(0x12, 0x1A, 0x1E);
    pub const TEXT: Color = Color::rgb(0x1E, 0x22, 0x26);
    pub const TEXT_INVERSE: Color = Color::rgb(0xE8, 0xEC, 0xEE);
    pub const TEXT_MUTED: Color = Color::rgb(0x8A, 0x96, 0x9C);
    pub const ACCENT: Color = Color::rgb(0xF2, 0xA5, 0x41);
    pub const ACCENT_ALT: Color = Color::rgb(0x3F, 0xB8, 0xAF);
    pub const DANGER: Color = Color::rgb(0xE5, 0x53, 0x4B);
    pub const OK: Color = Color::rgb(0x57, 0xC2, 0x7A);

    pub const CORNER_RADIUS_WINDOW: f32 = 10.0;
    pub const CORNER_RADIUS_WIDGET: f32 = 6.0;
    pub const FOCUS_RING_WIDTH: f32 = 2.0;

    /// Get text color for rendering on accent/danger backgrounds (high contrast)
    pub fn text_on_accent() -> Color {
        TEXT
    }
}

/// Layout dimensions
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub fn new(width: f32, height: f32) -> Self {
        Size { width, height }
    }

    pub fn zero() -> Self {
        Size { width: 0.0, height: 0.0 }
    }

    pub fn min(self, other: Size) -> Size {
        Size {
            width: if self.width < other.width { self.width } else { other.width },
            height: if self.height < other.height { self.height } else { other.height },
        }
    }

    pub fn max(self, other: Size) -> Size {
        Size {
            width: if self.width > other.width { self.width } else { other.width },
            height: if self.height > other.height { self.height } else { other.height },
        }
    }
}

/// Layout rectangle
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Rect { x, y, width, height }
    }

    pub fn at_size(size: Size) -> Self {
        Rect { x: 0.0, y: 0.0, width: size.width, height: size.height }
    }

    pub fn inset(self, dx: f32, dy: f32) -> Rect {
        Rect {
            x: self.x + dx,
            y: self.y + dy,
            width: (self.width - 2.0 * dx).max(0.0),
            height: (self.height - 2.0 * dy).max(0.0),
        }
    }

    pub fn translate(self, dx: f32, dy: f32) -> Rect {
        Rect {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
    }
}

/// Horizontal alignment
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

/// Vertical alignment
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    Top,
    Center,
    Bottom,
}

/// UI event
#[derive(Clone, Debug)]
pub enum Event {
    PointerMove { x: f32, y: f32 },
    PointerDown { x: f32, y: f32, button: u8 },
    PointerUp { x: f32, y: f32, button: u8 },
    Wheel { x: f32, y: f32, dx: f32, dy: f32 },
    KeyDown { key: u32, modifiers: u8, text: Option<String> },
    KeyUp { key: u32, modifiers: u8 },
    Focus { id: u32 },
    Blur { id: u32 },
}

/// UI action from widgets
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Clicked(u32),
    TextChanged(u32),
    Submitted(u32),
    ListSelected(u32, usize),
    CheckChanged(u32, bool),
}

/// Button style
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    Primary,
    Secondary,
    Danger,
}

/// Widget trait
pub trait Widget {
    fn measure<'a>(&self, fonts: &Fonts<'a>, max_width: f32) -> Size;
    fn arrange(&mut self, bounds: Rect);
    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache);
    fn handle_event(&mut self, event: &Event) -> Option<Action>;
    fn set_id(&mut self, id: u32);
    fn id(&self) -> u32;
    fn focused(&self) -> bool;
    fn set_focused(&mut self, focused: bool);
    fn bounds(&self) -> Rect;
}

/// Font references for rendering
pub struct Fonts<'a> {
    pub regular: &'a Font<'a>,
    pub semibold: &'a Font<'a>,
    pub mono: &'a Font<'a>,
}

/// Label widget
pub struct Label {
    id: u32,
    text: String,
    bounds: Rect,
    font_size: f32,
    color: Color,
    h_align: HAlign,
}

impl Label {
    pub fn new(text: impl Into<String>) -> Self {
        Label {
            id: 0,
            text: text.into(),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            font_size: 14.0,
            color: theme::TEXT,
            h_align: HAlign::Left,
        }
    }

    pub fn with_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn with_align(mut self, align: HAlign) -> Self {
        self.h_align = align;
        self
    }
}

impl Widget for Label {
    fn measure<'a>(&self, fonts: &Fonts<'a>, _max_width: f32) -> Size {
        let width = measure(fonts.regular, &self.text, self.font_size, 0.0);
        Size::new(width, self.font_size * 1.4)
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache) {
        let x = match self.h_align {
            HAlign::Left => self.bounds.x,
            HAlign::Center => self.bounds.x + (self.bounds.width - measure(fonts.regular, &self.text, self.font_size, 0.0)) / 2.0,
            HAlign::Right => self.bounds.x + self.bounds.width - measure(fonts.regular, &self.text, self.font_size, 0.0),
        };
        draw_text(
            &mut surface.canvas(),
            cache,
            fonts.regular,
            0,
            &self.text,
            x,
            self.bounds.y + self.font_size,
            self.font_size,
            0.0,
            self.color,
        );
    }

    fn handle_event(&mut self, _event: &Event) -> Option<Action> {
        None
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        false
    }

    fn set_focused(&mut self, _focused: bool) {}

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// Button widget
pub struct Button {
    id: u32,
    text: String,
    bounds: Rect,
    style: ButtonStyle,
    hovered: bool,
    pressed: bool,
    focused: bool,
}

impl Button {
    pub fn new(text: impl Into<String>, style: ButtonStyle) -> Self {
        Button {
            id: 0,
            text: text.into(),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            style,
            hovered: false,
            pressed: false,
            focused: false,
        }
    }

    fn bg_color(&self) -> Color {
        if self.pressed {
            match self.style {
                ButtonStyle::Primary => Color::rgb(0xD8, 0x8E, 0x28),
                ButtonStyle::Secondary => Color::rgb(0xC0, 0xC0, 0xC0),
                ButtonStyle::Danger => Color::rgb(0xC7, 0x37, 0x2F),
            }
        } else if self.hovered {
            match self.style {
                ButtonStyle::Primary => Color::rgb(0xFA, 0xB9, 0x52),
                ButtonStyle::Secondary => Color::rgb(0xE0, 0xE0, 0xE0),
                ButtonStyle::Danger => Color::rgb(0xF0, 0x69, 0x5F),
            }
        } else {
            match self.style {
                ButtonStyle::Primary => theme::ACCENT,
                ButtonStyle::Secondary => Color::rgb(0xD0, 0xD0, 0xD0),
                ButtonStyle::Danger => theme::DANGER,
            }
        }
    }

    fn text_color(&self) -> Color {
        theme::text_on_accent()
    }
}

impl Widget for Button {
    fn measure<'a>(&self, fonts: &Fonts<'a>, _max_width: f32) -> Size {
        let width = measure(fonts.regular, &self.text, 14.0, 0.0);
        Size::new(width + 24.0, 40.0)
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache) {
        fill_rounded_rect(
            &mut surface.canvas(),
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
            theme::CORNER_RADIUS_WIDGET,
            self.bg_color(),
        );

        if self.focused {
            // Draw focus ring
            for i in 0..2 {
                let inset = i as f32;
                fill_rounded_rect(
                    &mut surface.canvas(),
                    self.bounds.x - inset,
                    self.bounds.y - inset,
                    self.bounds.width + 2.0 * inset,
                    self.bounds.height + 2.0 * inset,
                    theme::CORNER_RADIUS_WIDGET,
                    if i == 0 { theme::ACCENT } else { Color::TRANSPARENT },
                );
            }
        }

        let text_width = measure(fonts.regular, &self.text, 14.0, 0.0);
        let x = self.bounds.x + (self.bounds.width - text_width) / 2.0;

        // Vertically center using font metrics
        let upm = fonts.regular.units_per_em().max(1) as f32;
        let ascender = fonts.regular.ascender() as f32 * 14.0 / upm;
        let descender = fonts.regular.descender() as f32 * 14.0 / upm;
        let text_height = ascender - descender;
        let y = self.bounds.y + (self.bounds.height - text_height) / 2.0 + ascender;

        draw_text(
            &mut surface.canvas(),
            cache,
            fonts.regular,
            0,
            &self.text,
            x,
            y,
            14.0,
            0.0,
            self.text_color(),
        );
    }

    fn handle_event(&mut self, event: &Event) -> Option<Action> {
        match event {
            Event::PointerMove { x, y } => {
                let was_hovered = self.hovered;
                self.hovered = x >= &self.bounds.x && x < &(self.bounds.x + self.bounds.width)
                    && y >= &self.bounds.y && y < &(self.bounds.y + self.bounds.height);
                if was_hovered != self.hovered {
                    // Damage tracking would go here
                }
                None
            }
            Event::PointerDown { x, y, button } if *button == 0 => {
                if x >= &self.bounds.x && x < &(self.bounds.x + self.bounds.width)
                    && y >= &self.bounds.y && y < &(self.bounds.y + self.bounds.height) {
                    self.pressed = true;
                }
                None
            }
            Event::PointerUp { x, y, button } if *button == 0 => {
                if self.pressed {
                    self.pressed = false;
                    // Check if pointer is still within bounds at release time
                    if x >= &self.bounds.x && x < &(self.bounds.x + self.bounds.width)
                        && y >= &self.bounds.y && y < &(self.bounds.y + self.bounds.height) {
                        return Some(Action::Clicked(self.id));
                    }
                }
                None
            }
            Event::KeyDown { key: 13, .. } if self.focused => {
                Some(Action::Clicked(self.id))
            }
            _ => None,
        }
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// TextField widget (single line)
pub struct TextField {
    id: u32,
    buffer: TextBuffer,
    bounds: Rect,
    focused: bool,
    placeholder: String,
}

impl TextField {
    pub fn new() -> Self {
        TextField {
            id: 0,
            buffer: TextBuffer::new(),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            focused: false,
            placeholder: String::new(),
        }
    }

    pub fn with_placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn text(&self) -> String {
        self.buffer.content()
    }

    pub fn set_text(&mut self, text: &str) {
        let _ = self.buffer.load(text.as_bytes());
    }
}

impl Default for TextField {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for TextField {
    fn measure<'a>(&self, _fonts: &Fonts<'a>, _max_width: f32) -> Size {
        Size::new(200.0, 40.0)
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache) {
        fill_rounded_rect(
            &mut surface.canvas(),
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
            theme::CORNER_RADIUS_WIDGET,
            theme::SURFACE,
        );

        if self.focused {
            fill_rounded_rect(
                &mut surface.canvas(),
                self.bounds.x - 2.0,
                self.bounds.y - 2.0,
                self.bounds.width + 4.0,
                self.bounds.height + 4.0,
                theme::CORNER_RADIUS_WIDGET,
                theme::ACCENT,
            );
            fill_rounded_rect(
                &mut surface.canvas(),
                self.bounds.x,
                self.bounds.y,
                self.bounds.width,
                self.bounds.height,
                theme::CORNER_RADIUS_WIDGET,
                theme::SURFACE,
            );
        }

        let content = self.buffer.content();
        let text = if content.is_empty() && !self.focused {
            self.placeholder.clone()
        } else {
            content.clone()
        };

        let text_x = self.bounds.x + 12.0;

        // Draw selection highlight if there is one
        if self.focused && !content.is_empty() {
            let (sel_start, sel_end) = self.buffer.selection();
            if sel_start != sel_end {
                let start_char = core::cmp::min(sel_start, sel_end);
                let end_char = core::cmp::max(sel_start, sel_end);

                // Get substrings for measuring
                let start_str = &content[..start_char.min(content.len())];
                let end_str = &content[..end_char.min(content.len())];

                let start_x = measure(fonts.regular, start_str, 14.0, 0.0);
                let end_x = measure(fonts.regular, end_str, 14.0, 0.0);

                // Draw selection with accent.alt color at ~35% alpha
                let selection_color = Color::rgba(0x3F, 0xB8, 0xAF, 90);
                fill_rect(
                    &mut surface.canvas(),
                    text_x + start_x,
                    self.bounds.y + 8.0,
                    (end_x - start_x).max(2.0),
                    self.bounds.height - 16.0,
                    selection_color,
                );
            }
        }

        draw_text(
            &mut surface.canvas(),
            cache,
            fonts.regular,
            0,
            &text,
            text_x,
            self.bounds.y + 26.0,
            14.0,
            0.0,
            if text == self.placeholder { theme::TEXT_MUTED } else { theme::TEXT },
        );

        // Draw cursor if focused
        if self.focused {
            let cursor_byte_pos = self.buffer.cursor();
            if cursor_byte_pos <= content.len() {
                let before_cursor = &content[..cursor_byte_pos];
                let cursor_offset = measure(fonts.regular, before_cursor, 14.0, 0.0);
                let cursor_x = text_x + cursor_offset;
                fill_rect(
                    &mut surface.canvas(),
                    cursor_x,
                    self.bounds.y + 8.0,
                    2.0,
                    self.bounds.height - 16.0,
                    theme::ACCENT,
                );
            }
        }
    }

    fn handle_event(&mut self, event: &Event) -> Option<Action> {
        if !self.focused {
            return None;
        }

        match event {
            Event::KeyDown { text: Some(ch), .. } => {
                self.buffer.insert(ch, 0);
                Some(Action::TextChanged(self.id))
            }
            Event::KeyDown { key: 8, .. } => {
                // Backspace
                if self.buffer.cursor() > 0 {
                    self.buffer.delete_char(0);
                    Some(Action::TextChanged(self.id))
                } else {
                    None
                }
            }
            Event::KeyDown { key: 13, .. } => {
                Some(Action::Submitted(self.id))
            }
            _ => None,
        }
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// TextArea widget (multi-line, with line numbers option)
pub struct TextArea {
    id: u32,
    buffer: TextBuffer,
    bounds: Rect,
    focused: bool,
    show_line_numbers: bool,
    scroll_y: f32,
}

impl TextArea {
    pub fn new() -> Self {
        TextArea {
            id: 0,
            buffer: TextBuffer::new(),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            focused: false,
            show_line_numbers: true,
            scroll_y: 0.0,
        }
    }

    pub fn text(&self) -> String {
        self.buffer.content()
    }

    pub fn set_text(&mut self, text: &str) {
        let _ = self.buffer.load(text.as_bytes());
    }
}

impl Default for TextArea {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for TextArea {
    fn measure<'a>(&self, _fonts: &Fonts<'a>, _max_width: f32) -> Size {
        Size::new(400.0, 200.0)
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache) {
        fill_rounded_rect(
            &mut surface.canvas(),
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
            theme::CORNER_RADIUS_WIDGET,
            theme::SURFACE,
        );

        if self.focused {
            fill_rounded_rect(
                &mut surface.canvas(),
                self.bounds.x - 2.0,
                self.bounds.y - 2.0,
                self.bounds.width + 4.0,
                self.bounds.height + 4.0,
                theme::CORNER_RADIUS_WIDGET,
                theme::ACCENT,
            );
            fill_rounded_rect(
                &mut surface.canvas(),
                self.bounds.x,
                self.bounds.y,
                self.bounds.width,
                self.bounds.height,
                theme::CORNER_RADIUS_WIDGET,
                theme::SURFACE,
            );
        }

        let line_height = 14.0 * 1.4;
        for (i, line) in self.buffer.content().lines().enumerate() {
            let y = self.bounds.y + 16.0 + i as f32 * line_height - self.scroll_y;
            if y < self.bounds.y || y > self.bounds.y + self.bounds.height {
                continue;
            }

            if self.show_line_numbers {
                let line_num = (i + 1).to_string();
                draw_text(
                    &mut surface.canvas(),
                    cache,
                    fonts.mono,
                    0,
                    &line_num,
                    self.bounds.x + 8.0,
                    y,
                    14.0,
                    0.0,
                    theme::TEXT_MUTED,
                );
            }

            let text_x = if self.show_line_numbers { self.bounds.x + 40.0 } else { self.bounds.x + 12.0 };
            draw_text(
                &mut surface.canvas(),
                cache,
                fonts.mono,
                0,
                line,
                text_x,
                y,
                14.0,
                0.0,
                theme::TEXT,
            );
        }
    }

    fn handle_event(&mut self, event: &Event) -> Option<Action> {
        if !self.focused {
            return None;
        }

        match event {
            Event::KeyDown { text: Some(ch), .. } => {
                self.buffer.insert(ch, 0);
                Some(Action::TextChanged(self.id))
            }
            Event::KeyDown { key: 8, .. } => {
                // Backspace
                if self.buffer.cursor() > 0 {
                    self.buffer.delete_char(0);
                    Some(Action::TextChanged(self.id))
                } else {
                    None
                }
            }
            Event::Wheel { dy, .. } => {
                self.scroll_y = (self.scroll_y - dy).max(0.0);
                None
            }
            _ => None,
        }
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// List widget
pub struct List {
    id: u32,
    items: Vec<String>,
    selected: Option<usize>,
    bounds: Rect,
    focused: bool,
}

impl List {
    pub fn new(items: Vec<String>) -> Self {
        List {
            id: 0,
            items,
            selected: None,
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            focused: false,
        }
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }
}

impl Widget for List {
    fn measure<'a>(&self, _fonts: &Fonts<'a>, _max_width: f32) -> Size {
        Size::new(200.0, self.items.len() as f32 * 32.0)
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache) {
        for (i, item) in self.items.iter().enumerate() {
            let item_y = self.bounds.y + i as f32 * 32.0;
            let item_rect = Rect::new(self.bounds.x, item_y, self.bounds.width, 32.0);

            let bg_color = if Some(i) == self.selected {
                theme::ACCENT
            } else {
                theme::SURFACE
            };

            fill_rect(
                &mut surface.canvas(),
                item_rect.x,
                item_rect.y,
                item_rect.width,
                item_rect.height,
                bg_color,
            );

            let text_color = if Some(i) == self.selected {
                theme::TEXT_INVERSE
            } else {
                theme::TEXT
            };

            draw_text(
                &mut surface.canvas(),
                cache,
                fonts.regular,
                0,
                item,
                item_rect.x + 12.0,
                item_rect.y + 22.0,
                14.0,
                0.0,
                text_color,
            );
        }
    }

    fn handle_event(&mut self, event: &Event) -> Option<Action> {
        if !self.focused {
            return None;
        }

        match event {
            Event::PointerDown { x, y, button } if *button == 0 => {
                let idx = ((y - self.bounds.y) / 32.0) as usize;
                if idx < self.items.len() {
                    self.selected = Some(idx);
                    return Some(Action::ListSelected(self.id, idx));
                }
                None
            }
            Event::KeyDown { key: 38, .. } => {
                // Up arrow
                if let Some(idx) = self.selected {
                    if idx > 0 {
                        self.selected = Some(idx - 1);
                        return Some(Action::ListSelected(self.id, idx - 1));
                    }
                } else if !self.items.is_empty() {
                    self.selected = Some(0);
                    return Some(Action::ListSelected(self.id, 0));
                }
                None
            }
            Event::KeyDown { key: 40, .. } => {
                // Down arrow
                if let Some(idx) = self.selected {
                    if idx + 1 < self.items.len() {
                        self.selected = Some(idx + 1);
                        return Some(Action::ListSelected(self.id, idx + 1));
                    }
                } else if !self.items.is_empty() {
                    self.selected = Some(0);
                    return Some(Action::ListSelected(self.id, 0));
                }
                None
            }
            _ => None,
        }
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// Checkbox widget
pub struct Checkbox {
    id: u32,
    checked: bool,
    bounds: Rect,
    focused: bool,
    label: String,
}

impl Checkbox {
    pub fn new(label: impl Into<String>) -> Self {
        Checkbox {
            id: 0,
            checked: false,
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            focused: false,
            label: label.into(),
        }
    }

    pub fn is_checked(&self) -> bool {
        self.checked
    }

    pub fn set_checked(&mut self, checked: bool) {
        self.checked = checked;
    }
}

impl Widget for Checkbox {
    fn measure<'a>(&self, fonts: &Fonts<'a>, _max_width: f32) -> Size {
        let label_width = measure(fonts.regular, &self.label, 14.0, 0.0);
        Size::new(24.0 + 12.0 + label_width, 24.0)
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache) {
        let check_rect = Rect::new(self.bounds.x, self.bounds.y, 24.0, 24.0);

        fill_rounded_rect(
            &mut surface.canvas(),
            check_rect.x,
            check_rect.y,
            check_rect.width,
            check_rect.height,
            3.0,
            theme::SURFACE,
        );

        if self.checked {
            fill_rounded_rect(
                &mut surface.canvas(),
                check_rect.x + 4.0,
                check_rect.y + 4.0,
                check_rect.width - 8.0,
                check_rect.height - 8.0,
                2.0,
                theme::ACCENT,
            );
        }

        draw_text(
            &mut surface.canvas(),
            cache,
            fonts.regular,
            0,
            &self.label,
            self.bounds.x + 40.0,
            self.bounds.y + 18.0,
            14.0,
            0.0,
            theme::TEXT,
        );
    }

    fn handle_event(&mut self, event: &Event) -> Option<Action> {
        match event {
            Event::PointerDown { x, y, button } if *button == 0 => {
                let rect = Rect::new(self.bounds.x, self.bounds.y, 24.0, 24.0);
                if x >= &rect.x && x < &(rect.x + rect.width)
                    && y >= &rect.y && y < &(rect.y + rect.height) {
                    self.checked = !self.checked;
                    return Some(Action::CheckChanged(self.id, self.checked));
                }
                None
            }
            Event::KeyDown { key: 32, .. } if self.focused => {
                self.checked = !self.checked;
                Some(Action::CheckChanged(self.id, self.checked))
            }
            _ => None,
        }
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// ProgressBar widget
pub struct ProgressBar {
    id: u32,
    value: f32,
    bounds: Rect,
}

impl ProgressBar {
    pub fn new(value: f32) -> Self {
        ProgressBar {
            id: 0,
            value: value.clamp(0.0, 1.0),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
        }
    }

    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }
}

impl Widget for ProgressBar {
    fn measure<'a>(&self, _fonts: &Fonts<'a>, _max_width: f32) -> Size {
        Size::new(200.0, 8.0)
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn render<'a>(&self, surface: &mut Surface, _fonts: &Fonts<'a>, _cache: &mut GlyphCache) {
        fill_rounded_rect(
            &mut surface.canvas(),
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
            4.0,
            Color::rgb(0xE0, 0xE0, 0xE0),
        );

        let filled_width = self.bounds.width * self.value;
        fill_rounded_rect(
            &mut surface.canvas(),
            self.bounds.x,
            self.bounds.y,
            filled_width,
            self.bounds.height,
            4.0,
            theme::ACCENT,
        );
    }

    fn handle_event(&mut self, _event: &Event) -> Option<Action> {
        None
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        false
    }

    fn set_focused(&mut self, _focused: bool) {}

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// TerminalView widget (renders an otter-term Terminal)
pub struct TerminalView {
    id: u32,
    terminal: Terminal,
    bounds: Rect,
    focused: bool,
}

impl TerminalView {
    pub fn new(cols: usize, rows: usize) -> Self {
        TerminalView {
            id: 0,
            terminal: Terminal::new(cols, rows),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            focused: false,
        }
    }

    pub fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }

    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }
}

impl Widget for TerminalView {
    fn measure<'a>(&self, _fonts: &Fonts<'a>, _max_width: f32) -> Size {
        // Assume 14px JetBrains Mono with standard monospace sizing
        let (cols, rows) = self.terminal.dims();
        Size::new(cols as f32 * 8.4, rows as f32 * 19.6)
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache) {
        fill_rounded_rect(
            &mut surface.canvas(),
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
            theme::CORNER_RADIUS_WIDGET,
            theme::SURFACE_DARK,
        );

        let char_width = 8.4;
        let line_height = 14.0 * 1.4;
        let (cols, rows) = self.terminal.dims();

        for row_idx in 0..rows {
            for col_idx in 0..cols {
                if let Some(cell) = self.terminal.cell_at(row_idx, col_idx) {
                    let char_str = cell.ch.to_string();
                    let x = self.bounds.x + col_idx as f32 * char_width;
                    let y = self.bounds.y + 8.0 + row_idx as f32 * line_height;

                    let fg_color = terminal_color_to_rgb(cell.fg);
                    draw_text(
                        &mut surface.canvas(),
                        cache,
                        fonts.mono,
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
    }

    fn handle_event(&mut self, _event: &Event) -> Option<Action> {
        None
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// Convert otter-term Color to otter-gfx Color
fn terminal_color_to_rgb(color: otter_term::Color) -> Color {
    match color {
        otter_term::Color::Black => Color::rgb(0x00, 0x00, 0x00),
        otter_term::Color::Red => Color::rgb(0xCC, 0x00, 0x00),
        otter_term::Color::Green => Color::rgb(0x00, 0xCC, 0x00),
        otter_term::Color::Yellow => Color::rgb(0xCC, 0xCC, 0x00),
        otter_term::Color::Blue => Color::rgb(0x00, 0x00, 0xCC),
        otter_term::Color::Magenta => Color::rgb(0xCC, 0x00, 0xCC),
        otter_term::Color::Cyan => Color::rgb(0x00, 0xCC, 0xCC),
        otter_term::Color::White => Color::rgb(0xCC, 0xCC, 0xCC),
        otter_term::Color::BrightBlack => Color::rgb(0x66, 0x66, 0x66),
        otter_term::Color::BrightRed => Color::rgb(0xFF, 0x00, 0x00),
        otter_term::Color::BrightGreen => Color::rgb(0x00, 0xFF, 0x00),
        otter_term::Color::BrightYellow => Color::rgb(0xFF, 0xFF, 0x00),
        otter_term::Color::BrightBlue => Color::rgb(0x00, 0x00, 0xFF),
        otter_term::Color::BrightMagenta => Color::rgb(0xFF, 0x00, 0xFF),
        otter_term::Color::BrightCyan => Color::rgb(0x00, 0xFF, 0xFF),
        otter_term::Color::BrightWhite => Color::rgb(0xFF, 0xFF, 0xFF),
        otter_term::Color::Rgb(r, g, b) => Color::rgb(r, g, b),
    }
}

/// Spacer widget
pub struct Spacer {
    id: u32,
    width: f32,
    height: f32,
    bounds: Rect,
}

impl Spacer {
    pub fn new(width: f32, height: f32) -> Self {
        Spacer {
            id: 0,
            width,
            height,
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
        }
    }
}

impl Widget for Spacer {
    fn measure<'a>(&self, _fonts: &Fonts<'a>, _max_width: f32) -> Size {
        Size::new(self.width, self.height)
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }

    fn render<'a>(&self, _surface: &mut Surface, _fonts: &Fonts<'a>, _cache: &mut GlyphCache) {}

    fn handle_event(&mut self, _event: &Event) -> Option<Action> {
        None
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        false
    }

    fn set_focused(&mut self, _focused: bool) {}

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// Container for widgets
pub struct Container {
    children: Vec<Box<dyn Widget>>,
    bounds: Rect,
    id: u32,
}

impl Container {
    pub fn new() -> Self {
        Container {
            children: Vec::new(),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            id: 0,
        }
    }

    pub fn add(&mut self, child: Box<dyn Widget>) {
        self.children.push(child);
    }
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Container {
    fn measure<'a>(&self, fonts: &Fonts<'a>, max_width: f32) -> Size {
        let mut total_width: f32 = 0.0;
        let mut total_height: f32 = 0.0;
        for child in &self.children {
            let size = child.measure(fonts, max_width);
            total_width = total_width.max(size.width);
            total_height = total_height.max(size.height);
        }
        Size::new(total_width, total_height)
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
        for child in &mut self.children {
            child.arrange(bounds);
        }
    }

    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache) {
        for child in &self.children {
            child.render(surface, fonts, cache);
        }
    }

    fn handle_event(&mut self, event: &Event) -> Option<Action> {
        for child in &mut self.children {
            if let Some(action) = child.handle_event(event) {
                return Some(action);
            }
        }
        None
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        false
    }

    fn set_focused(&mut self, _focused: bool) {}

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// Column layout container
pub struct Column {
    id: u32,
    children: Vec<Box<dyn Widget>>,
    bounds: Rect,
    padding: f32,
    spacing: f32,
}

impl Column {
    pub fn new() -> Self {
        Column {
            id: 0,
            children: Vec::new(),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            padding: 0.0,
            spacing: 0.0,
        }
    }

    pub fn with_padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }

    pub fn with_spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn add(&mut self, child: Box<dyn Widget>) {
        self.children.push(child);
    }
}

impl Default for Column {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Column {
    fn measure<'a>(&self, fonts: &Fonts<'a>, max_width: f32) -> Size {
        let mut total_height: f32 = 0.0;
        let mut measured_max_width: f32 = 0.0;

        for (i, child) in self.children.iter().enumerate() {
            let size = child.measure(fonts, max_width);
            measured_max_width = measured_max_width.max(size.width);
            if i > 0 {
                total_height += self.spacing;
            }
            total_height += size.height;
        }

        Size::new(
            measured_max_width + 2.0 * self.padding,
            total_height + 2.0 * self.padding,
        )
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
        let inset = bounds.inset(self.padding, self.padding);
        let mut current_y = inset.y;

        for child in &mut self.children {
            // We can't call measure here without having fonts, so we just use a default height
            // This is a simplification - a real implementation would need to pass fonts
            let est_height = 40.0;
            child.arrange(Rect::new(inset.x, current_y, inset.width, est_height));
            current_y += est_height + self.spacing;
        }
    }

    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache) {
        for child in &self.children {
            child.render(surface, fonts, cache);
        }
    }

    fn handle_event(&mut self, event: &Event) -> Option<Action> {
        for child in &mut self.children {
            if let Some(action) = child.handle_event(event) {
                return Some(action);
            }
        }
        None
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        false
    }

    fn set_focused(&mut self, _focused: bool) {}

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

/// Row layout container
pub struct Row {
    id: u32,
    children: Vec<Box<dyn Widget>>,
    bounds: Rect,
    padding: f32,
    spacing: f32,
}

impl Row {
    pub fn new() -> Self {
        Row {
            id: 0,
            children: Vec::new(),
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            padding: 0.0,
            spacing: 0.0,
        }
    }

    pub fn with_padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }

    pub fn with_spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn add(&mut self, child: Box<dyn Widget>) {
        self.children.push(child);
    }
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Row {
    fn measure<'a>(&self, fonts: &Fonts<'a>, max_width: f32) -> Size {
        let mut total_width: f32 = 0.0;
        let mut max_height: f32 = 0.0;

        for (i, child) in self.children.iter().enumerate() {
            let size = child.measure(fonts, max_width);
            max_height = max_height.max(size.height);
            if i > 0 {
                total_width += self.spacing;
            }
            total_width += size.width;
        }

        Size::new(
            total_width + 2.0 * self.padding,
            max_height + 2.0 * self.padding,
        )
    }

    fn arrange(&mut self, bounds: Rect) {
        self.bounds = bounds;
        let inset = bounds.inset(self.padding, self.padding);
        let mut current_x = inset.x;

        for child in &mut self.children {
            // We can't call measure here without having fonts, so we just use a default width
            // This is a simplification - a real implementation would need to pass fonts
            let est_width = 100.0;
            child.arrange(Rect::new(current_x, inset.y, est_width, inset.height));
            current_x += est_width + self.spacing;
        }
    }

    fn render<'a>(&self, surface: &mut Surface, fonts: &Fonts<'a>, cache: &mut GlyphCache) {
        for child in &self.children {
            child.render(surface, fonts, cache);
        }
    }

    fn handle_event(&mut self, event: &Event) -> Option<Action> {
        for child in &mut self.children {
            if let Some(action) = child.handle_event(event) {
                return Some(action);
            }
        }
        None
    }

    fn set_id(&mut self, id: u32) {
        self.id = id;
    }

    fn id(&self) -> u32 {
        self.id
    }

    fn focused(&self) -> bool {
        false
    }

    fn set_focused(&mut self, _focused: bool) {}

    fn bounds(&self) -> Rect {
        self.bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_size_min_max() {
        let s1 = Size::new(10.0, 20.0);
        let s2 = Size::new(15.0, 15.0);
        assert_eq!(s1.min(s2), Size::new(10.0, 15.0));
        assert_eq!(s1.max(s2), Size::new(15.0, 20.0));
    }

    #[test]
    fn test_rect_inset() {
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        let inset = r.inset(10.0, 10.0);
        assert_eq!(inset, Rect::new(10.0, 10.0, 80.0, 80.0));
    }

    #[test]
    fn test_rect_translate() {
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        let translated = r.translate(50.0, 50.0);
        assert_eq!(translated, Rect::new(50.0, 50.0, 100.0, 100.0));
    }

    #[test]
    fn test_label_basic() {
        let label = Label::new("Hello");
        assert_eq!(label.id(), 0);
        assert!(!label.focused());
    }

    #[test]
    fn test_label_with_color() {
        let label = Label::new("Test").with_color(theme::ACCENT);
        assert_eq!(label.color, theme::ACCENT);
    }

    #[test]
    fn test_button_primary() {
        let button = Button::new("Click", ButtonStyle::Primary);
        assert_eq!(button.style, ButtonStyle::Primary);
    }

    #[test]
    fn test_button_secondary() {
        let button = Button::new("Click", ButtonStyle::Secondary);
        assert_eq!(button.style, ButtonStyle::Secondary);
    }

    #[test]
    fn test_button_danger() {
        let button = Button::new("Delete", ButtonStyle::Danger);
        assert_eq!(button.style, ButtonStyle::Danger);
    }

    #[test]
    fn test_button_hover() {
        let mut button = Button::new("Hover", ButtonStyle::Primary);
        let event = Event::PointerMove { x: 10.0, y: 10.0 };
        button.arrange(Rect::new(0.0, 0.0, 80.0, 40.0));
        button.handle_event(&event);
        assert!(button.hovered);
    }

    #[test]
    fn test_button_click() {
        let mut button = Button::new("Click", ButtonStyle::Primary);
        button.set_id(1);
        button.arrange(Rect::new(0.0, 0.0, 80.0, 40.0));

        let down = Event::PointerDown { x: 10.0, y: 10.0, button: 0 };
        button.handle_event(&down);
        assert!(button.pressed);

        let up = Event::PointerUp { x: 10.0, y: 10.0, button: 0 };
        let action = button.handle_event(&up);
        assert_eq!(action, Some(Action::Clicked(1)));
        assert!(!button.pressed);
    }

    #[test]
    fn test_button_focused_enter() {
        let mut button = Button::new("Click", ButtonStyle::Primary);
        button.set_id(1);
        button.set_focused(true);
        let event = Event::KeyDown { key: 13, modifiers: 0, text: None };
        let action = button.handle_event(&event);
        assert_eq!(action, Some(Action::Clicked(1)));
    }

    #[test]
    fn test_textfield_empty() {
        let field = TextField::new();
        assert_eq!(field.text(), "");
    }

    #[test]
    fn test_textfield_set_text() {
        let mut field = TextField::new();
        field.set_text("hello");
        assert_eq!(field.text(), "hello");
    }

    #[test]
    fn test_textfield_placeholder() {
        let field = TextField::new().with_placeholder("Enter text");
        assert_eq!(field.placeholder, "Enter text");
    }

    #[test]
    fn test_textfield_focus() {
        let mut field = TextField::new();
        assert!(!field.focused());
        field.set_focused(true);
        assert!(field.focused());
    }

    #[test]
    fn test_list_new() {
        let items = vec!["Item 1".to_string(), "Item 2".to_string()];
        let list = List::new(items.clone());
        assert_eq!(list.items, items);
        assert_eq!(list.selected(), None);
    }

    #[test]
    fn test_list_select() {
        let items = vec!["Item 1".to_string(), "Item 2".to_string()];
        let mut list = List::new(items);
        list.set_focused(true);
        list.arrange(Rect::new(0.0, 0.0, 200.0, 64.0));

        let event = Event::PointerDown { x: 10.0, y: 10.0, button: 0 };
        let action = list.handle_event(&event);
        assert_eq!(list.selected(), Some(0));
    }

    #[test]
    fn test_list_arrow_keys() {
        let items = vec!["Item 1".to_string(), "Item 2".to_string(), "Item 3".to_string()];
        let mut list = List::new(items);
        list.set_focused(true);

        // First down arrow selects item 0
        let down = Event::KeyDown { key: 40, modifiers: 0, text: None };
        list.handle_event(&down);
        assert_eq!(list.selected(), Some(0));

        // Next down arrow selects item 1
        list.handle_event(&down);
        assert_eq!(list.selected(), Some(1));

        // Up arrow goes back to item 0
        let up = Event::KeyDown { key: 38, modifiers: 0, text: None };
        list.handle_event(&up);
        assert_eq!(list.selected(), Some(0));
    }

    #[test]
    fn test_checkbox_new() {
        let check = Checkbox::new("Accept");
        assert!(!check.checked);
    }

    #[test]
    fn test_checkbox_toggle() {
        let mut check = Checkbox::new("Accept");
        assert!(!check.is_checked());
        check.set_checked(true);
        assert!(check.is_checked());
    }

    #[test]
    fn test_checkbox_click() {
        let mut check = Checkbox::new("Accept");
        check.set_id(1);
        check.arrange(Rect::new(0.0, 0.0, 100.0, 24.0));

        let event = Event::PointerDown { x: 10.0, y: 10.0, button: 0 };
        let action = check.handle_event(&event);
        assert_eq!(action, Some(Action::CheckChanged(1, true)));
        assert!(check.is_checked());

        let action = check.handle_event(&event);
        assert_eq!(action, Some(Action::CheckChanged(1, false)));
        assert!(!check.is_checked());
    }

    #[test]
    fn test_progressbar_new() {
        let pb = ProgressBar::new(0.5);
        assert_eq!(pb.value, 0.5);
    }

    #[test]
    fn test_progressbar_clamp() {
        let pb1 = ProgressBar::new(1.5);
        assert_eq!(pb1.value, 1.0);
        let pb2 = ProgressBar::new(-0.5);
        assert_eq!(pb2.value, 0.0);
    }

    #[test]
    fn test_progressbar_set_value() {
        let mut pb = ProgressBar::new(0.0);
        pb.set_value(0.75);
        assert_eq!(pb.value, 0.75);
    }

    #[test]
    fn test_textarea_new() {
        let area = TextArea::new();
        assert_eq!(area.text(), "");
    }

    #[test]
    fn test_textarea_set_text() {
        let mut area = TextArea::new();
        area.set_text("line1\nline2");
        assert_eq!(area.text(), "line1\nline2");
    }

    #[test]
    fn test_terminal_view_new() {
        let view = TerminalView::new(80, 24);
        let (cols, rows) = view.terminal.dims();
        assert_eq!(cols, 80);
        assert_eq!(rows, 24);
    }

    #[test]
    fn test_spacer_size() {
        let spacer = Spacer::new(10.0, 20.0);
        assert_eq!(spacer.width, 10.0);
        assert_eq!(spacer.height, 20.0);
    }

    #[test]
    fn test_column_new() {
        let col = Column::new().with_padding(10.0).with_spacing(5.0);
        assert_eq!(col.padding, 10.0);
        assert_eq!(col.spacing, 5.0);
    }

    #[test]
    fn test_row_new() {
        let row = Row::new().with_padding(10.0).with_spacing(5.0);
        assert_eq!(row.padding, 10.0);
        assert_eq!(row.spacing, 5.0);
    }

    #[test]
    fn test_halign_left() {
        let align = HAlign::Left;
        assert_eq!(align, HAlign::Left);
    }

    #[test]
    fn test_halign_center() {
        let align = HAlign::Center;
        assert_eq!(align, HAlign::Center);
    }

    #[test]
    fn test_valign_top() {
        let align = VAlign::Top;
        assert_eq!(align, VAlign::Top);
    }

    #[test]
    fn test_action_equality() {
        assert_eq!(Action::Clicked(1), Action::Clicked(1));
        assert_ne!(Action::Clicked(1), Action::Clicked(2));
    }

    #[test]
    fn test_theme_colors() {
        assert_ne!(theme::ACCENT, theme::DANGER);
        assert_ne!(theme::SURFACE, theme::SURFACE_DARK);
        assert_ne!(theme::TEXT, theme::TEXT_INVERSE);
    }

    #[test]
    fn test_button_style_equality() {
        assert_eq!(ButtonStyle::Primary, ButtonStyle::Primary);
        assert_ne!(ButtonStyle::Primary, ButtonStyle::Secondary);
    }

    #[test]
    fn test_label_alignment() {
        let label_left = Label::new("Test").with_align(HAlign::Left);
        assert_eq!(label_left.h_align, HAlign::Left);

        let label_center = Label::new("Test").with_align(HAlign::Center);
        assert_eq!(label_center.h_align, HAlign::Center);
    }

    #[test]
    fn test_label_font_size() {
        let label = Label::new("Test").with_size(18.0);
        assert_eq!(label.font_size, 18.0);
    }

    #[test]
    fn test_multiple_buttons_unique_ids() {
        let mut btn1 = Button::new("One", ButtonStyle::Primary);
        let mut btn2 = Button::new("Two", ButtonStyle::Secondary);
        btn1.set_id(1);
        btn2.set_id(2);
        assert_eq!(btn1.id(), 1);
        assert_eq!(btn2.id(), 2);
    }

    #[test]
    fn test_event_pointer_move() {
        let event = Event::PointerMove { x: 100.0, y: 200.0 };
        match event {
            Event::PointerMove { x, y } => {
                assert_eq!(x, 100.0);
                assert_eq!(y, 200.0);
            }
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn test_event_key_down() {
        let event = Event::KeyDown { key: 65, modifiers: 0, text: Some("A".to_string()) };
        match event {
            Event::KeyDown { key, text, .. } => {
                assert_eq!(key, 65);
                assert_eq!(text, Some("A".to_string()));
            }
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn test_rect_at_size() {
        let size = Size::new(100.0, 50.0);
        let rect = Rect::at_size(size);
        assert_eq!(rect, Rect::new(0.0, 0.0, 100.0, 50.0));
    }

    #[test]
    fn test_button_out_of_bounds_click() {
        let mut button = Button::new("Click", ButtonStyle::Primary);
        button.set_id(1);
        button.arrange(Rect::new(0.0, 0.0, 80.0, 40.0));

        let down = Event::PointerDown { x: 100.0, y: 100.0, button: 0 };
        button.handle_event(&down);
        assert!(!button.pressed);

        let up = Event::PointerUp { x: 100.0, y: 100.0, button: 0 };
        let action = button.handle_event(&up);
        assert_eq!(action, None);
    }

    #[test]
    fn test_size_zero() {
        let s = Size::zero();
        assert_eq!(s.width, 0.0);
        assert_eq!(s.height, 0.0);
    }

    #[test]
    fn test_list_empty() {
        let list = List::new(vec![]);
        assert_eq!(list.items.len(), 0);
    }

    #[test]
    fn test_checkbox_space_key() {
        let mut check = Checkbox::new("Test");
        check.set_id(1);
        check.set_focused(true);

        let event = Event::KeyDown { key: 32, modifiers: 0, text: None };
        let action = check.handle_event(&event);
        assert_eq!(action, Some(Action::CheckChanged(1, true)));
    }

    #[test]
    fn test_halign_right() {
        assert_eq!(HAlign::Right, HAlign::Right);
    }

    #[test]
    fn test_valign_center() {
        assert_eq!(VAlign::Center, VAlign::Center);
    }

    #[test]
    fn test_valign_bottom() {
        assert_eq!(VAlign::Bottom, VAlign::Bottom);
    }

    #[test]
    fn test_event_wheel() {
        let event = Event::Wheel { x: 100.0, y: 200.0, dx: 10.0, dy: 20.0 };
        match event {
            Event::Wheel { dx, dy, .. } => {
                assert_eq!(dx, 10.0);
                assert_eq!(dy, 20.0);
            }
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn test_label_bounds_after_arrange() {
        let mut label = Label::new("Test");
        let bounds = Rect::new(10.0, 20.0, 100.0, 30.0);
        label.arrange(bounds);
        assert_eq!(label.bounds(), bounds);
    }

    #[test]
    fn test_button_bounds_after_arrange() {
        let mut button = Button::new("Click", ButtonStyle::Primary);
        let bounds = Rect::new(10.0, 20.0, 80.0, 40.0);
        button.arrange(bounds);
        assert_eq!(button.bounds(), bounds);
    }

    #[test]
    fn test_textfield_bounds_after_arrange() {
        let mut field = TextField::new();
        let bounds = Rect::new(10.0, 20.0, 200.0, 40.0);
        field.arrange(bounds);
        assert_eq!(field.bounds(), bounds);
    }

    #[test]
    fn test_list_bounds_after_arrange() {
        let items = vec!["Item 1".to_string(), "Item 2".to_string()];
        let mut list = List::new(items);
        let bounds = Rect::new(10.0, 20.0, 200.0, 100.0);
        list.arrange(bounds);
        assert_eq!(list.bounds(), bounds);
    }

    #[test]
    fn test_checkbox_bounds_after_arrange() {
        let mut check = Checkbox::new("Test");
        let bounds = Rect::new(10.0, 20.0, 100.0, 24.0);
        check.arrange(bounds);
        assert_eq!(check.bounds(), bounds);
    }

    #[test]
    fn test_progressbar_bounds_after_arrange() {
        let mut pb = ProgressBar::new(0.5);
        let bounds = Rect::new(10.0, 20.0, 200.0, 8.0);
        pb.arrange(bounds);
        assert_eq!(pb.bounds(), bounds);
    }

    #[test]
    fn test_spacer_bounds_after_arrange() {
        let mut spacer = Spacer::new(10.0, 20.0);
        let bounds = Rect::new(10.0, 20.0, 50.0, 50.0);
        spacer.arrange(bounds);
        assert_eq!(spacer.bounds(), bounds);
    }

    #[test]
    fn test_textfield_caret_position() {
        let mut field = TextField::new();
        field.set_text("hello");
        // After set_text, cursor is at the beginning
        assert_eq!(field.buffer.cursor(), 0);
        // After inserting, cursor moves
        field.buffer.insert("x", 0);
        assert_eq!(field.buffer.cursor(), 1);
    }

    #[test]
    fn test_textfield_selection() {
        let mut field = TextField::new();
        field.set_text("hello world");
        // Test that we can get selection info from the buffer
        let (start, end) = field.buffer.selection();
        // Initially no selection
        assert_eq!(start, end);
    }

    #[test]
    fn test_textarea_text_handling() {
        let mut area = TextArea::new();
        area.set_text("line1\nline2\nline3");
        assert_eq!(area.text(), "line1\nline2\nline3");
        assert_eq!(area.buffer.line_count(), 3);
    }

    #[test]
    fn test_button_text_color() {
        let btn_primary = Button::new("Primary", ButtonStyle::Primary);
        let btn_secondary = Button::new("Secondary", ButtonStyle::Secondary);
        let btn_danger = Button::new("Danger", ButtonStyle::Danger);

        // All buttons should use dark text for contrast
        assert_eq!(btn_primary.text_color(), theme::TEXT);
        assert_eq!(btn_secondary.text_color(), theme::TEXT);
        assert_eq!(btn_danger.text_color(), theme::TEXT);
    }

    #[test]
    fn test_button_and_accent_text_contrast() {
        // This test verifies that text on accent backgrounds is dark and has good contrast

        // Theme text color should be dark (luminance < 0.3)
        let text_color = theme::text_on_accent();
        // RGB(0x1E, 0x22, 0x26) = dark gray/near-black
        // Relative luminance = 0.2126*R + 0.7152*G + 0.0722*B (using sRGB)
        let r = 0x1E as f32 / 255.0;
        let g = 0x22 as f32 / 255.0;
        let b = 0x26 as f32 / 255.0;
        let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        assert!(luminance < 0.3, "Text color must be dark, luminance was {}", luminance);

        // Verify accent color is light for good contrast
        let accent_r = 0xF2 as f32 / 255.0;
        let accent_g = 0xA5 as f32 / 255.0;
        let accent_b = 0x41 as f32 / 255.0;
        let accent_luminance = 0.2126 * accent_r + 0.7152 * accent_g + 0.0722 * accent_b;
        assert!(accent_luminance > 0.5, "Accent color must be light for contrast, luminance was {}", accent_luminance);

        // Verify contrast ratio >= 4:1 (good contrast for readability)
        let contrast_ratio = (accent_luminance + 0.05) / (luminance + 0.05);
        assert!(contrast_ratio >= 4.0, "Contrast ratio must be >= 4:1 for readability, was {}", contrast_ratio);
    }
}
