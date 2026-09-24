#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::vec::Vec;
use alloc::string::String;

use otter_gfx::color::Color;
use otter_gfx::geom::{Point, Rect};
use otter_gfx::surface::Surface;

// Helper functions for no_std math
fn f32_round(x: f32) -> f32 {
    if x >= 0.0 {
        (x + 0.5) as i32 as f32
    } else {
        (x - 0.5) as i32 as f32
    }
}

// Design tokens from docs/DESIGN.md
pub mod design {
    use otter_gfx::color::Color;

    pub const BG_TOP: Color = Color::rgb(0x10, 0x30, 0x3B);
    pub const BG_BOTTOM: Color = Color::rgb(0x08, 0x14, 0x19);
    pub const GLOW: Color = Color::rgba(0x2C, 0x7A, 0x7B, 110);
    pub const GLOW_FADE: Color = Color::rgba(0x2C, 0x7A, 0x7B, 0);
    pub const CHROME: Color = Color::rgba(0x1B, 0x24, 0x29, 235);
    pub const CHROME_FOCUS: Color = Color::rgba(0x22, 0x2E, 0x34, 245);
    pub const SURFACE: Color = Color::rgb(0xF6, 0xF4, 0xF0);
    pub const SURFACE_DARK: Color = Color::rgb(0x12, 0x1A, 0x1E);
    pub const TEXT: Color = Color::rgb(0x1E, 0x22, 0x26);
    pub const TEXT_INVERSE: Color = Color::rgb(0xE8, 0xEC, 0xEE);
    pub const ACCENT: Color = Color::rgb(0xF2, 0xA5, 0x41);
    pub const ACCENT_ALT: Color = Color::rgb(0x3F, 0xB8, 0xAF);
    pub const DANGER: Color = Color::rgb(0xE5, 0x53, 0x4B);
    pub const OK: Color = Color::rgb(0x57, 0xC2, 0x7A);

    pub const TITLE_BAR_HEIGHT: f32 = 34.0;
    pub const CORNER_RADIUS: f32 = 10.0;
    pub const TASKBAR_HEIGHT: f32 = 40.0;
    pub const TASKBAR_CORNER_RADIUS: f32 = 14.0;
    pub const TASKBAR_PADDING: f32 = 10.0;
    pub const WINDOW_MIN_WIDTH: f32 = 200.0;
    pub const WINDOW_MIN_HEIGHT: f32 = 120.0;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Minimised,
    Maximised,
}

#[derive(Clone, Debug)]
pub struct Window {
    pub id: u32,
    pub title: String,
    pub rect: Rect,
    pub state: WindowState,
    pub focused: bool,
    pub surface: Option<WindowSurface>,
}

#[derive(Clone, Debug)]
pub struct WindowSurface {
    pub width: usize,
    pub height: usize,
    pub pixels: alloc::vec::Vec<u32>,
}

impl WindowSurface {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: alloc::vec![0; width * height],
        }
    }

    pub fn get_pixel(&self, x: usize, y: usize) -> u32 {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x]
        } else {
            0
        }
    }

    pub fn set_pixel(&mut self, x: usize, y: usize, pixel: u32) {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x] = pixel;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Arrow,
    Move,
    ResizeH,
    ResizeV,
    ResizeNWSE,
    ResizeNESW,
}

#[derive(Clone)]
pub struct Cursor {
    pub x: f32,
    pub y: f32,
    pub shape: CursorShape,
    pub visible: bool,
}

impl Cursor {
    pub fn new() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            shape: CursorShape::Arrow,
            visible: true,
        }
    }
}

impl Default for Cursor {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputEvent {
    PointerMove { x: f32, y: f32 },
    PointerDown { x: f32, y: f32, button: u32 },
    PointerUp { x: f32, y: f32, button: u32 },
    KeyDown { key: u32 },
    KeyUp { key: u32 },
}

#[derive(Clone)]
pub struct DamageRect {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
}

impl DamageRect {
    pub fn from_rect(r: Rect) -> Self {
        Self {
            x: r.x,
            y: r.y,
            width: r.w as usize,
            height: r.h as usize,
        }
    }

    pub fn union(&self, other: &DamageRect) -> DamageRect {
        let x1 = self.x.min(other.x);
        let y1 = self.y.min(other.y);
        let x2 = (self.x + self.width as i32).max(other.x + other.width as i32);
        let y2 = (self.y + self.height as i32).max(other.y + other.height as i32);
        DamageRect {
            x: x1,
            y: y1,
            width: (x2 - x1) as usize,
            height: (y2 - y1) as usize,
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let xi = x as i32;
        let yi = y as i32;
        xi >= self.x && xi < self.x + self.width as i32 && yi >= self.y && yi < self.y + self.height as i32
    }
}

pub struct WindowManager {
    pub windows: Vec<u32>,
    pub window_data: Vec<Window>,
    pub focused_window: Option<u32>,
    pub cursor: Cursor,
    pub damage_rects: Vec<DamageRect>,
    pub wallpaper: Option<Surface>,
    pub framebuffer_width: usize,
    pub framebuffer_height: usize,
    pub taskbar_visible: bool,
    pub clock_text: String,
    pub dragging_window: Option<u32>,
    pub drag_start_x: f32,
    pub drag_start_y: f32,
    pub resize_start_rect: Option<Rect>,
    pub resize_edge: Option<ResizeEdge>,
    pub last_composed_frame: Option<Surface>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl WindowManager {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            windows: Vec::new(),
            window_data: Vec::new(),
            focused_window: None,
            cursor: Cursor::new(),
            damage_rects: Vec::new(),
            wallpaper: None,
            framebuffer_width: width,
            framebuffer_height: height,
            taskbar_visible: true,
            clock_text: String::new(),
            dragging_window: None,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
            resize_start_rect: None,
            resize_edge: None,
            last_composed_frame: None,
        }
    }

    pub fn create_window(&mut self, id: u32, title: String, x: f32, y: f32, width: f32, height: f32) {
        let window = Window {
            id,
            title,
            rect: Rect::new(x as i32, y as i32, width as i32, height as i32),
            state: WindowState::Normal,
            focused: false,
            surface: None,
        };
        self.window_data.push(window);
        self.windows.push(id);
        self.focus_window(Some(id));
    }

    pub fn focus_window(&mut self, id: Option<u32>) {
        if let Some(old_focused) = self.focused_window {
            let old_rect = if let Some(win) = self.window_data.iter_mut().find(|w| w.id == old_focused) {
                win.focused = false;
                let r = win.rect;
                Some(r)
            } else {
                None
            };
            if let Some(r) = old_rect {
                self.add_damage_rect(&DamageRect::from_rect(r));
            }
        }

        if let Some(new_id) = id {
            let exists = self.window_data.iter().any(|w| w.id == new_id);
            if !exists {
                return;
            }

            let new_rect = if let Some(win) = self.window_data.iter_mut().find(|w| w.id == new_id) {
                win.focused = true;
                let r = win.rect;
                Some(r)
            } else {
                None
            };
            if let Some(r) = new_rect {
                self.add_damage_rect(&DamageRect::from_rect(r));
            }

            if let Some(idx) = self.windows.iter().position(|w| *w == new_id) {
                self.windows.remove(idx);
                self.windows.push(new_id);
            }

            self.focused_window = Some(new_id);
        } else {
            self.focused_window = None;
        }
    }

    pub fn get_window_mut(&mut self, id: u32) -> Option<&mut Window> {
        self.window_data.iter_mut().find(|w| w.id == id)
    }

    pub fn get_window(&self, id: u32) -> Option<&Window> {
        self.window_data.iter().find(|w| w.id == id)
    }

    pub fn move_window(&mut self, id: u32, x: f32, y: f32) {
        let (old_rect, new_rect) = if let Some(win) = self.get_window_mut(id) {
            let old_r = win.rect;
            win.rect.x = x as i32;
            win.rect.y = y as i32;
            let new_r = win.rect;
            (old_r, new_r)
        } else {
            return;
        };
        self.add_damage_rect(&DamageRect::from_rect(old_rect));
        self.add_damage_rect(&DamageRect::from_rect(new_rect));
    }

    pub fn resize_window(&mut self, id: u32, width: f32, height: f32) {
        let (old_rect, new_rect) = if let Some(win) = self.get_window_mut(id) {
            let new_width = width.max(design::WINDOW_MIN_WIDTH) as i32;
            let new_height = height.max(design::WINDOW_MIN_HEIGHT) as i32;
            let old_r = win.rect;
            win.rect.w = new_width;
            win.rect.h = new_height;
            let new_r = win.rect;
            (old_r, new_r)
        } else {
            return;
        };
        self.add_damage_rect(&DamageRect::from_rect(old_rect));
        self.add_damage_rect(&DamageRect::from_rect(new_rect));
    }

    pub fn minimise_window(&mut self, id: u32) {
        let rect = if let Some(win) = self.get_window_mut(id) {
            win.state = WindowState::Minimised;
            let r = win.rect;
            Some(r)
        } else {
            None
        };
        if let Some(r) = rect {
            self.add_damage_rect(&DamageRect::from_rect(r));
        }
        if self.focused_window == Some(id) {
            self.focus_window(None);
        }
    }

    pub fn restore_window(&mut self, id: u32) {
        let rect = if let Some(win) = self.get_window_mut(id) {
            win.state = WindowState::Normal;
            let r = win.rect;
            Some(r)
        } else {
            None
        };
        if let Some(r) = rect {
            self.add_damage_rect(&DamageRect::from_rect(r));
        }
        self.focus_window(Some(id));
    }

    pub fn maximise_window(&mut self, id: u32) {
        let taskbar_height = if self.taskbar_visible { design::TASKBAR_HEIGHT } else { 0.0 };
        let height = self.framebuffer_height as f32 - taskbar_height;
        let framebuffer_width = self.framebuffer_width;

        let rect = if let Some(win) = self.get_window_mut(id) {
            win.state = WindowState::Maximised;
            win.rect = Rect::new(0, 0, framebuffer_width as i32, height as i32);
            let r = win.rect;
            Some(r)
        } else {
            None
        };
        if let Some(r) = rect {
            self.add_damage_rect(&DamageRect::from_rect(r));
        }
    }

    pub fn add_damage_rect(&mut self, rect: &DamageRect) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }

        if self.damage_rects.is_empty() {
            self.damage_rects.push(DamageRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            });
            return;
        }

        let mut unioned = DamageRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        };

        for existing in &self.damage_rects {
            unioned = unioned.union(existing);
        }

        self.damage_rects.clear();
        self.damage_rects.push(unioned);
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Option<HitTestResult> {
        for &id in self.windows.iter().rev() {
            if let Some(win) = self.get_window(id) {
                if win.state == WindowState::Minimised {
                    continue;
                }

                let rect = win.rect;
                if x >= rect.x as f32 && x < (rect.x + rect.w) as f32
                    && y >= rect.y as f32 && y < (rect.y + rect.h) as f32
                {
                    let rel_x = x - rect.x as f32;
                    let rel_y = y - rect.y as f32;

                    let corner_margin = 10.0;
                    let edge_margin = 4.0;

                    if rel_x < corner_margin && rel_y < corner_margin {
                        return Some(HitTestResult::ResizeTopLeft(id));
                    }
                    if rel_x > rect.w as f32 - corner_margin && rel_y < corner_margin {
                        return Some(HitTestResult::ResizeTopRight(id));
                    }
                    if rel_x < corner_margin && rel_y > rect.h as f32 - corner_margin {
                        return Some(HitTestResult::ResizeBottomLeft(id));
                    }
                    if rel_x > rect.w as f32 - corner_margin && rel_y > rect.h as f32 - corner_margin {
                        return Some(HitTestResult::ResizeBottomRight(id));
                    }

                    if rel_x < edge_margin {
                        return Some(HitTestResult::ResizeLeft(id));
                    }
                    if rel_x > rect.w as f32 - edge_margin {
                        return Some(HitTestResult::ResizeRight(id));
                    }
                    if rel_y < edge_margin {
                        return Some(HitTestResult::ResizeTop(id));
                    }
                    if rel_y > rect.h as f32 - edge_margin {
                        return Some(HitTestResult::ResizeBottom(id));
                    }

                    if rel_y < design::TITLE_BAR_HEIGHT {
                        #[allow(clippy::manual_range_contains)]
                        if (16.0..50.0).contains(&rel_x) && (5.0..29.0).contains(&rel_y) {
                            if rel_x < 28.0 {
                                return Some(HitTestResult::CloseButton(id));
                            } else {
                                return Some(HitTestResult::MinimiseButton(id));
                            }
                        }
                        return Some(HitTestResult::TitleBar(id));
                    }

                    return Some(HitTestResult::ClientArea(id));
                }
            }
        }

        let taskbar_y = self.framebuffer_height as f32 - design::TASKBAR_HEIGHT;
        if self.taskbar_visible && y >= taskbar_y {
            return Some(HitTestResult::Taskbar);
        }

        None
    }

    pub fn handle_input(&mut self, event: InputEvent) {
        match event {
            InputEvent::PointerMove { x, y } => {
                self.cursor.x = x;
                self.cursor.y = y;

                if let Some(win_id) = self.dragging_window {
                    let dx = x - self.drag_start_x;
                    let dy = y - self.drag_start_y;

                    if let Some(edge) = self.resize_edge {
                        if let Some(start_rect) = self.resize_start_rect {
                            let mut new_rect = start_rect;

                            match edge {
                                ResizeEdge::Top => {
                                    new_rect.y = (start_rect.y as f32 + dy) as i32;
                                    new_rect.h = (start_rect.h as f32 - dy) as i32;
                                }
                                ResizeEdge::Bottom => {
                                    new_rect.h = (start_rect.h as f32 + dy) as i32;
                                }
                                ResizeEdge::Left => {
                                    new_rect.x = (start_rect.x as f32 + dx) as i32;
                                    new_rect.w = (start_rect.w as f32 - dx) as i32;
                                }
                                ResizeEdge::Right => {
                                    new_rect.w = (start_rect.w as f32 + dx) as i32;
                                }
                                ResizeEdge::TopLeft => {
                                    new_rect.x = (start_rect.x as f32 + dx) as i32;
                                    new_rect.y = (start_rect.y as f32 + dy) as i32;
                                    new_rect.w = (start_rect.w as f32 - dx) as i32;
                                    new_rect.h = (start_rect.h as f32 - dy) as i32;
                                }
                                ResizeEdge::TopRight => {
                                    new_rect.y = (start_rect.y as f32 + dy) as i32;
                                    new_rect.w = (start_rect.w as f32 + dx) as i32;
                                    new_rect.h = (start_rect.h as f32 - dy) as i32;
                                }
                                ResizeEdge::BottomLeft => {
                                    new_rect.x = (start_rect.x as f32 + dx) as i32;
                                    new_rect.w = (start_rect.w as f32 - dx) as i32;
                                    new_rect.h = (start_rect.h as f32 + dy) as i32;
                                }
                                ResizeEdge::BottomRight => {
                                    new_rect.w = (start_rect.w as f32 + dx) as i32;
                                    new_rect.h = (start_rect.h as f32 + dy) as i32;
                                }
                            }

                            new_rect.w = (new_rect.w as f32).max(design::WINDOW_MIN_WIDTH) as i32;
                            new_rect.h = (new_rect.h as f32).max(design::WINDOW_MIN_HEIGHT) as i32;

                            if let Some(win) = self.get_window_mut(win_id) {
                                win.rect = new_rect;
                                self.add_damage_rect(&DamageRect::from_rect(new_rect));
                            }
                        }
                    } else {
                        self.move_window(win_id, self.drag_start_x + dx, self.drag_start_y + dy);
                    }
                }

                if self.dragging_window.is_none() {
                    if let Some(hit) = self.hit_test(x, y) {
                        match hit {
                            HitTestResult::ResizeTop(_) | HitTestResult::ResizeBottom(_) => {
                                self.cursor.shape = CursorShape::ResizeV;
                            }
                            HitTestResult::ResizeLeft(_) | HitTestResult::ResizeRight(_) => {
                                self.cursor.shape = CursorShape::ResizeH;
                            }
                            HitTestResult::ResizeTopLeft(_) | HitTestResult::ResizeBottomRight(_) => {
                                self.cursor.shape = CursorShape::ResizeNWSE;
                            }
                            HitTestResult::ResizeTopRight(_) | HitTestResult::ResizeBottomLeft(_) => {
                                self.cursor.shape = CursorShape::ResizeNESW;
                            }
                            _ => self.cursor.shape = CursorShape::Arrow,
                        }
                    } else {
                        self.cursor.shape = CursorShape::Arrow;
                    }
                }
            }
            InputEvent::PointerDown { x, y, .. } => {
                if let Some(hit) = self.hit_test(x, y) {
                    match hit {
                        HitTestResult::ClientArea(id) | HitTestResult::TitleBar(id) => {
                            self.focus_window(Some(id));
                            if matches!(hit, HitTestResult::TitleBar(_)) {
                                let (win_x, win_y) = if let Some(win) = self.get_window(id) {
                                    (win.rect.x as f32, win.rect.y as f32)
                                } else {
                                    (x, y)
                                };
                                self.dragging_window = Some(id);
                                self.drag_start_x = win_x;
                                self.drag_start_y = win_y;
                                self.cursor.shape = CursorShape::Move;
                            }
                        }
                        HitTestResult::CloseButton(id) => {
                            self.windows.retain(|w| *w != id);
                            self.window_data.retain(|w| w.id != id);
                            if self.focused_window == Some(id) {
                                self.focused_window = None;
                                if let Some(&new_focus) = self.windows.last() {
                                    self.focus_window(Some(new_focus));
                                }
                            }
                        }
                        HitTestResult::MinimiseButton(id) => {
                            self.minimise_window(id);
                        }
                        HitTestResult::ResizeTop(id) => {
                            let rect = self.get_window(id).map(|w| w.rect);
                            self.dragging_window = Some(id);
                            self.resize_edge = Some(ResizeEdge::Top);
                            self.resize_start_rect = rect;
                            self.drag_start_x = x;
                            self.drag_start_y = y;
                        }
                        HitTestResult::ResizeBottom(id) => {
                            let rect = self.get_window(id).map(|w| w.rect);
                            self.dragging_window = Some(id);
                            self.resize_edge = Some(ResizeEdge::Bottom);
                            self.resize_start_rect = rect;
                            self.drag_start_x = x;
                            self.drag_start_y = y;
                        }
                        HitTestResult::ResizeLeft(id) => {
                            let rect = self.get_window(id).map(|w| w.rect);
                            self.dragging_window = Some(id);
                            self.resize_edge = Some(ResizeEdge::Left);
                            self.resize_start_rect = rect;
                            self.drag_start_x = x;
                            self.drag_start_y = y;
                        }
                        HitTestResult::ResizeRight(id) => {
                            let rect = self.get_window(id).map(|w| w.rect);
                            self.dragging_window = Some(id);
                            self.resize_edge = Some(ResizeEdge::Right);
                            self.resize_start_rect = rect;
                            self.drag_start_x = x;
                            self.drag_start_y = y;
                        }
                        HitTestResult::ResizeTopLeft(id) => {
                            let rect = self.get_window(id).map(|w| w.rect);
                            self.dragging_window = Some(id);
                            self.resize_edge = Some(ResizeEdge::TopLeft);
                            self.resize_start_rect = rect;
                            self.drag_start_x = x;
                            self.drag_start_y = y;
                        }
                        HitTestResult::ResizeTopRight(id) => {
                            let rect = self.get_window(id).map(|w| w.rect);
                            self.dragging_window = Some(id);
                            self.resize_edge = Some(ResizeEdge::TopRight);
                            self.resize_start_rect = rect;
                            self.drag_start_x = x;
                            self.drag_start_y = y;
                        }
                        HitTestResult::ResizeBottomLeft(id) => {
                            let rect = self.get_window(id).map(|w| w.rect);
                            self.dragging_window = Some(id);
                            self.resize_edge = Some(ResizeEdge::BottomLeft);
                            self.resize_start_rect = rect;
                            self.drag_start_x = x;
                            self.drag_start_y = y;
                        }
                        HitTestResult::ResizeBottomRight(id) => {
                            let rect = self.get_window(id).map(|w| w.rect);
                            self.dragging_window = Some(id);
                            self.resize_edge = Some(ResizeEdge::BottomRight);
                            self.resize_start_rect = rect;
                            self.drag_start_x = x;
                            self.drag_start_y = y;
                        }
                        HitTestResult::Taskbar => {}
                    }
                }
            }
            InputEvent::PointerUp { .. } => {
                self.dragging_window = None;
                self.resize_edge = None;
                self.resize_start_rect = None;
            }
            InputEvent::KeyDown { key } => {
                if key == 9 {
                    self.cycle_focus_forward();
                }
            }
            InputEvent::KeyUp { .. } => {}
        }
    }

    pub fn cycle_focus_forward(&mut self) {
        if self.windows.is_empty() {
            return;
        }

        if let Some(current) = self.focused_window {
            if let Some(idx) = self.windows.iter().position(|w| *w == current) {
                let next_idx = (idx + 1) % self.windows.len();
                let next_id = self.windows[next_idx];
                self.focus_window(Some(next_id));
            }
        } else if let Some(&first) = self.windows.first() {
            self.focus_window(Some(first));
        }
    }

    pub fn update_clock(&mut self, text: String) {
        if self.clock_text != text {
            self.clock_text = text;
            let taskbar_y = self.framebuffer_height as f32 - design::TASKBAR_HEIGHT;
            self.add_damage_rect(&DamageRect {
                x: 0,
                y: taskbar_y as i32,
                width: self.framebuffer_width,
                height: design::TASKBAR_HEIGHT as usize,
            });
        }
    }

    pub fn composite_frame(&mut self) -> Surface {
        let mut frame = Surface::new(self.framebuffer_width, self.framebuffer_height);

        use otter_gfx::gradient::{fill_radial_gradient, fill_linear_gradient, ColorStop};
        use otter_gfx::shapes::fill_rounded_rect;
        use otter_gfx::shadow::{build_shadow_mask, draw_shadow_mask};

        {
            let mut canvas = frame.canvas();
            fill_linear_gradient(
                &mut canvas,
                Point::new(0.0, 0.0),
                Point::new(0.0, self.framebuffer_height as f32),
                &[ColorStop::new(0.0, design::BG_TOP), ColorStop::new(1.0, design::BG_BOTTOM)],
            );
            fill_radial_gradient(
                &mut canvas,
                Point::new(160.0, 140.0),
                480.0,
                &[ColorStop::new(0.0, design::GLOW), ColorStop::new(1.0, design::GLOW_FADE)],
            );
        }

        for &id in &self.windows {
            if let Some(win) = self.get_window(id) {
                if win.state == WindowState::Minimised {
                    continue;
                }

                let (offset_y, blur, alpha): (f32, f32, f32) =
                    if win.focused { (12.0, 32.0, 0.45) } else { (6.0, 18.0, 0.30) };

                let rect = win.rect;
                let w = rect.w as f32;
                let h = rect.h as f32;

                let mask = build_shadow_mask(w, h, design::CORNER_RADIUS, blur, 0.0);
                draw_shadow_mask(
                    &mut frame.canvas(),
                    &mask,
                    rect.x as f32,
                    rect.y as f32 + offset_y,
                    Color::rgba(0, 0, 0, f32_round(alpha * 255.0) as u8),
                );
            }
        }

        for &id in &self.windows {
            if let Some(win) = self.get_window(id) {
                if win.state == WindowState::Minimised {
                    continue;
                }

                let rect = win.rect;
                let body_color = if let Some(_surf) = &win.surface {
                    design::SURFACE
                } else {
                    design::SURFACE_DARK
                };

                let mut canvas = frame.canvas();
                fill_rounded_rect(
                    &mut canvas,
                    rect.x as f32,
                    rect.y as f32,
                    rect.w as f32,
                    rect.h as f32,
                    design::CORNER_RADIUS,
                    body_color,
                );

                canvas.push_clip(Rect::new(rect.x, rect.y, rect.w, 34));
                let chrome_color = if win.focused {
                    design::CHROME_FOCUS
                } else {
                    design::CHROME
                };
                fill_rounded_rect(
                    &mut canvas,
                    rect.x as f32,
                    rect.y as f32,
                    rect.w as f32,
                    rect.h as f32,
                    design::CORNER_RADIUS,
                    chrome_color,
                );
                canvas.pop_clip();

                if let Some(_surf) = &win.surface {
                    let _text_color = design::TEXT;
                    let mut _canvas = frame.canvas();
                } else {
                    let _text_color = design::TEXT_INVERSE;
                    let mut _canvas = frame.canvas();
                }
            }
        }

        if self.taskbar_visible {
            let taskbar_y = self.framebuffer_height as f32 - design::TASKBAR_HEIGHT;
            let mut canvas = frame.canvas();

            fill_rounded_rect(
                &mut canvas,
                design::TASKBAR_PADDING,
                taskbar_y,
                self.framebuffer_width as f32 - 2.0 * design::TASKBAR_PADDING,
                design::TASKBAR_HEIGHT,
                design::TASKBAR_CORNER_RADIUS,
                design::CHROME,
            );
        }

        self.damage_rects.clear();
        frame
    }
}

pub enum HitTestResult {
    TitleBar(u32),
    ClientArea(u32),
    CloseButton(u32),
    MinimiseButton(u32),
    ResizeTop(u32),
    ResizeBottom(u32),
    ResizeLeft(u32),
    ResizeRight(u32),
    ResizeTopLeft(u32),
    ResizeTopRight(u32),
    ResizeBottomLeft(u32),
    ResizeBottomRight(u32),
    Taskbar,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_window_creation() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);
        assert_eq!(wm.windows.len(), 1);
        assert_eq!(wm.focused_window, Some(1));
    }

    #[test]
    fn test_window_focus() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Win1"), 100.0, 100.0, 800.0, 600.0);
        wm.create_window(2, String::from("Win2"), 200.0, 200.0, 800.0, 600.0);

        assert_eq!(wm.focused_window, Some(2));
        wm.focus_window(Some(1));
        assert_eq!(wm.focused_window, Some(1));
        assert!(wm.get_window(1).unwrap().focused);
        assert!(!wm.get_window(2).unwrap().focused);
    }

    #[test]
    fn test_window_stacking_order() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Win1"), 100.0, 100.0, 800.0, 600.0);
        wm.create_window(2, String::from("Win2"), 200.0, 200.0, 800.0, 600.0);
        wm.create_window(3, String::from("Win3"), 300.0, 300.0, 800.0, 600.0);

        assert_eq!(wm.windows, vec![1, 2, 3]);
        wm.focus_window(Some(1));
        assert_eq!(wm.windows, vec![2, 3, 1]);
    }

    #[test]
    fn test_window_minimise() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);
        wm.minimise_window(1);
        assert_eq!(wm.get_window(1).unwrap().state, WindowState::Minimised);
    }

    #[test]
    fn test_window_maximise() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);
        wm.maximise_window(1);
        assert_eq!(wm.get_window(1).unwrap().state, WindowState::Maximised);
        let win = wm.get_window(1).unwrap();
        assert_eq!(win.rect.x, 0);
        assert_eq!(win.rect.y, 0);
        assert_eq!(win.rect.w, 1920);
    }

    #[test]
    fn test_damage_tracking() {
        let mut wm = WindowManager::new(1920, 1080);
        assert!(wm.damage_rects.is_empty());

        let rect = DamageRect { x: 10, y: 10, width: 100, height: 100 };
        wm.add_damage_rect(&rect);
        assert_eq!(wm.damage_rects.len(), 1);
    }

    #[test]
    fn test_damage_union() {
        let rect1 = DamageRect { x: 0, y: 0, width: 100, height: 100 };
        let rect2 = DamageRect { x: 50, y: 50, width: 100, height: 100 };
        let union = rect1.union(&rect2);

        assert_eq!(union.x, 0);
        assert_eq!(union.y, 0);
        assert_eq!(union.width, 150);
        assert_eq!(union.height, 150);
    }

    #[test]
    fn test_hit_test_client_area() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);

        if let Some(HitTestResult::ClientArea(id)) = wm.hit_test(400.0, 400.0) {
            assert_eq!(id, 1);
        } else {
            panic!("Expected ClientArea hit");
        }
    }

    #[test]
    fn test_hit_test_title_bar() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);

        if let Some(HitTestResult::TitleBar(id)) = wm.hit_test(400.0, 110.0) {
            assert_eq!(id, 1);
        } else {
            panic!("Expected TitleBar hit");
        }
    }

    #[test]
    fn test_hit_test_buttons() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);

        if let Some(HitTestResult::CloseButton(id)) = wm.hit_test(122.0, 117.0) {
            assert_eq!(id, 1);
        } else {
            panic!("Expected CloseButton hit");
        }
    }

    #[test]
    fn test_hit_test_resize_corners() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);

        if let Some(HitTestResult::ResizeBottomRight(_)) = wm.hit_test(895.0, 695.0) {
        } else {
            panic!("Expected ResizeBottomRight hit");
        }
    }

    #[test]
    fn test_cycle_focus() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Win1"), 100.0, 100.0, 800.0, 600.0);
        wm.create_window(2, String::from("Win2"), 200.0, 200.0, 800.0, 600.0);
        wm.create_window(3, String::from("Win3"), 300.0, 300.0, 800.0, 600.0);

        assert_eq!(wm.focused_window, Some(3));
        wm.cycle_focus_forward();
        assert_eq!(wm.focused_window, Some(1));
        wm.cycle_focus_forward();
        assert_eq!(wm.focused_window, Some(2));
        wm.cycle_focus_forward();
        assert_eq!(wm.focused_window, Some(3));
    }

    #[test]
    fn test_pointer_events() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);

        wm.handle_input(InputEvent::PointerMove { x: 400.0, y: 400.0 });
        assert_eq!(wm.cursor.x, 400.0);
        assert_eq!(wm.cursor.y, 400.0);
    }

    #[test]
    fn test_window_move() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);
        wm.move_window(1, 200.0, 200.0);

        let win = wm.get_window(1).unwrap();
        assert_eq!(win.rect.x, 200);
        assert_eq!(win.rect.y, 200);
    }

    #[test]
    fn test_window_resize() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);
        wm.resize_window(1, 1000.0, 700.0);

        let win = wm.get_window(1).unwrap();
        assert_eq!(win.rect.w, 1000);
        assert_eq!(win.rect.h, 700);
    }

    #[test]
    fn test_window_resize_minimum() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 800.0, 600.0);
        wm.resize_window(1, 50.0, 50.0);

        let win = wm.get_window(1).unwrap();
        assert!(win.rect.w as f32 >= design::WINDOW_MIN_WIDTH);
        assert!(win.rect.h as f32 >= design::WINDOW_MIN_HEIGHT);
    }

    #[test]
    fn test_multiple_window_operations() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Win1"), 100.0, 100.0, 400.0, 300.0);
        wm.create_window(2, String::from("Win2"), 200.0, 200.0, 400.0, 300.0);
        wm.create_window(3, String::from("Win3"), 300.0, 300.0, 400.0, 300.0);

        wm.move_window(1, 150.0, 150.0);
        wm.resize_window(2, 500.0, 400.0);
        wm.move_window(3, 400.0, 400.0);

        let w1 = wm.get_window(1).unwrap();
        assert_eq!(w1.rect.x, 150);
        assert_eq!(w1.rect.y, 150);

        let w2 = wm.get_window(2).unwrap();
        assert_eq!(w2.rect.w, 500);
        assert_eq!(w2.rect.h, 400);

        let w3 = wm.get_window(3).unwrap();
        assert_eq!(w3.rect.x, 400);
        assert_eq!(w3.rect.y, 400);
    }

    #[test]
    fn test_window_state_transitions() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 400.0, 300.0);

        assert_eq!(wm.get_window(1).unwrap().state, WindowState::Normal);

        wm.minimise_window(1);
        assert_eq!(wm.get_window(1).unwrap().state, WindowState::Minimised);

        wm.restore_window(1);
        assert_eq!(wm.get_window(1).unwrap().state, WindowState::Normal);

        wm.maximise_window(1);
        assert_eq!(wm.get_window(1).unwrap().state, WindowState::Maximised);
    }

    #[test]
    fn test_focus_null() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 400.0, 300.0);
        assert_eq!(wm.focused_window, Some(1));

        wm.focus_window(None);
        assert_eq!(wm.focused_window, None);
    }

    #[test]
    fn test_hit_test_taskbar() {
        let wm = WindowManager::new(1920, 1080);
        let taskbar_y = 1080.0 - design::TASKBAR_HEIGHT;

        if let Some(HitTestResult::Taskbar) = wm.hit_test(960.0, taskbar_y + 20.0) {
        } else {
            panic!("Expected Taskbar hit");
        }
    }

    #[test]
    fn test_hit_test_outside_windows() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 400.0, 300.0);

        let result = wm.hit_test(50.0, 50.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_multiple_windows_with_moves() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("A"), 100.0, 100.0, 300.0, 300.0);
        wm.create_window(2, String::from("B"), 400.0, 100.0, 300.0, 300.0);
        wm.create_window(3, String::from("C"), 700.0, 100.0, 300.0, 300.0);

        wm.move_window(2, 150.0, 150.0);

        let w1 = wm.get_window(1).unwrap();
        let w2 = wm.get_window(2).unwrap();
        let w3 = wm.get_window(3).unwrap();

        assert_eq!(w1.rect.x, 100);
        assert_eq!(w2.rect.x, 150);
        assert_eq!(w3.rect.x, 700);
    }

    #[test]
    fn test_restore_after_minimise_stays_focused() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 400.0, 300.0);

        wm.minimise_window(1);
        assert_eq!(wm.focused_window, None);

        wm.restore_window(1);
        assert_eq!(wm.focused_window, Some(1));
        assert!(wm.get_window(1).unwrap().focused);
    }

    #[test]
    fn test_empty_window_manager() {
        let wm = WindowManager::new(1920, 1080);
        assert!(wm.windows.is_empty());
        assert_eq!(wm.focused_window, None);
        assert!(wm.damage_rects.is_empty());
    }

    #[test]
    fn test_cursor_initialization() {
        let cursor = Cursor::new();
        assert_eq!(cursor.x, 0.0);
        assert_eq!(cursor.y, 0.0);
        assert_eq!(cursor.shape, CursorShape::Arrow);
        assert!(cursor.visible);
    }

    #[test]
    fn test_resize_all_edges() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 400.0, 300.0);

        let test_cases = vec![
            (103.0, 103.0, HitTestResult::ResizeTopLeft(1)),
            (497.0, 103.0, HitTestResult::ResizeTopRight(1)),
            (103.0, 397.0, HitTestResult::ResizeBottomLeft(1)),
            (497.0, 397.0, HitTestResult::ResizeBottomRight(1)),
        ];

        for (x, y, expected_variant) in test_cases {
            if let Some(result) = wm.hit_test(x, y) {
                match (result, expected_variant) {
                    (HitTestResult::ResizeTopLeft(_), HitTestResult::ResizeTopLeft(_)) => {}
                    (HitTestResult::ResizeTopRight(_), HitTestResult::ResizeTopRight(_)) => {}
                    (HitTestResult::ResizeBottomLeft(_), HitTestResult::ResizeBottomLeft(_)) => {}
                    (HitTestResult::ResizeBottomRight(_), HitTestResult::ResizeBottomRight(_)) => {}
                    _ => panic!("Hit test mismatch at ({}, {})", x, y),
                }
            } else {
                panic!("Expected hit at ({}, {})", x, y);
            }
        }
    }

    #[test]
    fn test_window_deletion() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Win1"), 100.0, 100.0, 400.0, 300.0);
        wm.create_window(2, String::from("Win2"), 200.0, 200.0, 400.0, 300.0);

        assert_eq!(wm.windows.len(), 2);

        wm.windows.retain(|w| *w != 1);
        wm.window_data.retain(|w| w.id != 1);

        assert_eq!(wm.windows.len(), 1);
        assert!(wm.get_window(1).is_none());
        assert!(wm.get_window(2).is_some());
    }

    #[test]
    fn test_damage_rect_contains() {
        let rect = DamageRect { x: 100, y: 100, width: 200, height: 200 };

        assert!(rect.contains(150.0, 150.0));
        assert!(!rect.contains(50.0, 150.0));
        assert!(!rect.contains(150.0, 50.0));
        assert!(!rect.contains(350.0, 150.0));
    }

    #[test]
    fn test_framebuffer_dimensions() {
        let wm = WindowManager::new(1920, 1080);
        assert_eq!(wm.framebuffer_width, 1920);
        assert_eq!(wm.framebuffer_height, 1080);
    }

    #[test]
    fn test_clock_text_empty() {
        let wm = WindowManager::new(1920, 1080);
        assert_eq!(wm.clock_text.len(), 0);
    }

    #[test]
    fn test_no_focus_on_nonexistent_window() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 400.0, 300.0);
        wm.focus_window(Some(1));
        assert_eq!(wm.focused_window, Some(1));

        wm.focus_window(Some(999));
        assert_eq!(wm.focused_window, Some(1));
    }

    #[test]
    fn test_window_surface_pixels() {
        let mut surf = WindowSurface::new(100, 100);
        assert_eq!(surf.get_pixel(0, 0), 0);

        surf.set_pixel(50, 50, 0xFF0000FF);
        assert_eq!(surf.get_pixel(50, 50), 0xFF0000FF);

        assert_eq!(surf.get_pixel(101, 50), 0);
        assert_eq!(surf.get_pixel(50, 101), 0);
    }

    #[test]
    fn test_hit_test_title_bar_buttons() {
        let mut wm = WindowManager::new(1920, 1080);
        wm.create_window(1, String::from("Test"), 100.0, 100.0, 400.0, 300.0);

        if let Some(HitTestResult::CloseButton(id)) = wm.hit_test(122.0, 117.0) {
            assert_eq!(id, 1);
        } else {
            panic!("Expected CloseButton");
        }

        if let Some(HitTestResult::MinimiseButton(id)) = wm.hit_test(142.0, 117.0) {
            assert_eq!(id, 1);
        } else {
            panic!("Expected MinimiseButton");
        }
    }
}
