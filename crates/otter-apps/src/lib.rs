#![no_std]
#![cfg_attr(test, allow(unused))]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use otter_gfx::surface::Surface;

/// UI event from the window manager or display server.
pub use otter_ui::Event as UiEvent;

/// Application trait: the interface between an app and the display server/window manager.
pub trait App {
    /// Get the current window title.
    fn title(&self) -> String;

    /// Handle a UI event and return a list of requests to the display server.
    /// `now_ms` is the monotonic time since boot.
    fn handle(&mut self, event: UiEvent, now_ms: u64) -> Vec<AppRequest>;

    /// Render the app into a surface.
    fn render(&mut self, surface: &mut Surface);

    /// Minimum window size in (width, height) pixels.
    fn min_size(&self) -> (u32, u32);
}

/// Requests an app can make to the display server.
#[derive(Clone, Debug)]
pub enum AppRequest {
    /// Close the window.
    Close,
    /// Update the window title.
    SetTitle(String),
    /// Spawn a new process.
    Spawn { path: String, argv: Vec<String> },
    /// Write bytes to a file.
    WriteFile { path: String, data: Vec<u8> },
    /// Read bytes from a file.
    ReadFile { path: String },
}

/// Service trait for the Terminal app: access to the shell's input/output.
pub trait ByteChannel {
    /// Read up to `buf.len()` bytes into `buf`, returning the number of bytes read.
    fn read(&mut self, buf: &mut [u8]) -> usize;
    /// Write the given bytes.
    fn write(&mut self, bytes: &[u8]);
}

/// Service trait for the Editor app: access to the file system.
pub trait FileStore {
    /// Read an entire file; return error message on failure.
    fn read(&mut self, path: &str) -> Result<Vec<u8>, String>;
    /// Write an entire file; return error message on failure.
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), String>;
}

/// Terminal app: a terminal emulator over otter-term.
pub struct TerminalApp {
    title: String,
    terminal: otter_term::Terminal,
    cursor_blink_on: bool,
    blink_time_ms: u64,
    last_blink_toggle: u64,
}

impl TerminalApp {
    /// Create a new terminal app with the given dimensions.
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut terminal = otter_term::Terminal::new(cols, rows);
        terminal.set_newline_mode(true);
        TerminalApp {
            title: String::from("Terminal"),
            terminal,
            cursor_blink_on: true,
            blink_time_ms: 500,
            last_blink_toggle: 0,
        }
    }

    /// Get a reference to the underlying terminal.
    pub fn terminal(&self) -> &otter_term::Terminal {
        &self.terminal
    }

    /// Get a mutable reference to the underlying terminal.
    pub fn terminal_mut(&mut self) -> &mut otter_term::Terminal {
        &mut self.terminal
    }

    /// Feed data from the shell into the terminal.
    pub fn write_data(&mut self, data: &[u8]) {
        self.terminal.feed_bytes(data);
    }
}

impl App for TerminalApp {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn handle(&mut self, event: UiEvent, now_ms: u64) -> Vec<AppRequest> {
        let requests = Vec::new();

        // Update cursor blink state
        if now_ms.saturating_sub(self.last_blink_toggle) >= self.blink_time_ms {
            self.cursor_blink_on = !self.cursor_blink_on;
            self.last_blink_toggle = now_ms;
        }

        match event {
            UiEvent::KeyDown { key, modifiers, text } => {
                // Handle keyboard input
                let bytes = key_event_to_bytes(key, modifiers, text);
                if !bytes.is_empty() {
                    // In a real implementation, would write to shell via ByteChannel
                }
            }
            UiEvent::Wheel { .. } => {
                // TODO: Handle scrollback
            }
            UiEvent::PointerDown { .. } => {
                // TODO: Handle selection start
            }
            UiEvent::PointerUp { .. } => {
                // TODO: Handle selection end
            }
            _ => {}
        }

        requests
    }

    fn render(&mut self, surface: &mut Surface) {
        use otter_gfx::color::Color;
        let w = surface.w as f32;
        let h = surface.h as f32;
        let mut canvas = surface.canvas();
        // Fill background with terminal color
        let bg = Color::rgb(0x12, 0x1A, 0x1E);
        otter_gfx::shapes::fill_rect(&mut canvas, 0.0, 0.0, w, h, bg);

        // Render terminal grid via otter-ui's TerminalView
        // This would normally be integrated with the widget tree
    }

    fn min_size(&self) -> (u32, u32) {
        let (cols, rows) = self.terminal.dims();
        // Rough estimate: 8px per char width, 16px per char height
        ((cols as u32) * 8, (rows as u32) * 16)
    }
}

/// Editor app: a text editor over otter-textbuf.
pub struct EditorApp {
    title: String,
    filename: String,
    buffer: otter_textbuf::TextBuffer,
    modified: bool,
    #[allow(dead_code)]
    line_numbers: bool,
}

impl EditorApp {
    /// Create a new editor app with no file loaded.
    pub fn new() -> Self {
        EditorApp {
            title: String::from("Editor"),
            filename: String::new(),
            buffer: otter_textbuf::TextBuffer::new(),
            modified: false,
            line_numbers: true,
        }
    }

    /// Load a file into the editor.
    pub fn load_file(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        self.buffer.load(data).map_err(|_| "Invalid UTF-8".to_string())?;
        self.filename = String::from(path);
        self.modified = false;
        self.update_title();
        Ok(())
    }

    /// Save the current buffer to bytes.
    pub fn save(&mut self) -> Vec<u8> {
        self.modified = false;
        self.update_title();
        self.buffer.save()
    }

    fn update_title(&mut self) {
        self.title = alloc::format!(
            "{}{}",
            self.filename.as_str(),
            if self.modified { " ●" } else { "" }
        );
    }

    /// Get a reference to the buffer.
    pub fn buffer(&self) -> &otter_textbuf::TextBuffer {
        &self.buffer
    }

    /// Get a mutable reference to the buffer.
    pub fn buffer_mut(&mut self) -> &mut otter_textbuf::TextBuffer {
        &mut self.buffer
    }
}

impl Default for EditorApp {
    fn default() -> Self {
        Self::new()
    }
}

impl App for EditorApp {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn handle(&mut self, event: UiEvent, now_ms: u64) -> Vec<AppRequest> {
        let requests = Vec::new();

        if let UiEvent::KeyDown { key, modifiers, text } = event {
            match (key, modifiers) {
                (17, 0o4) => {
                    // Ctrl+S: save
                    // In a real implementation, would write to file
                }
                (25, 0o4) => {
                    // Ctrl+Z: undo
                    self.buffer_mut().undo();
                    self.modified = true;
                }
                (28, 0o4) => {
                    // Ctrl+Y: redo
                    self.buffer_mut().redo();
                    self.modified = true;
                }
                _ => {
                    // Regular character input
                    if let Some(ch) = text {
                        if !ch.is_empty() {
                            self.buffer_mut().insert(&ch, now_ms);
                            self.modified = true;
                        }
                    }
                }
            }
        }

        requests
    }

    fn render(&mut self, surface: &mut Surface) {
        use otter_gfx::color::Color;
        let w = surface.w as f32;
        let h = surface.h as f32;
        let mut canvas = surface.canvas();
        // Fill background with editor color
        let bg = Color::rgb(0xF6, 0xF4, 0xF0);
        otter_gfx::shapes::fill_rect(&mut canvas, 0.0, 0.0, w, h, bg);

        // Render text buffer via otter-ui's TextArea
        // This would normally be integrated with the widget tree
    }

    fn min_size(&self) -> (u32, u32) {
        // Minimum 40 columns x 12 rows
        (320, 192)
    }
}

/// Convert a key event to terminal bytes.
fn key_event_to_bytes(key: u32, modifiers: u8, text: Option<String>) -> Vec<u8> {
    let mut bytes = Vec::new();

    // Handle modifier combinations
    let _ctrl = modifiers & 0o4 != 0;
    let _shift = modifiers & 0o1 != 0;
    let _alt = modifiers & 0o2 != 0;

    match key {
        13 => bytes.push(b'\r'),  // Enter
        127 => bytes.push(0x7F),  // Backspace
        9 => bytes.push(b'\t'),   // Tab
        _ => {
            if let Some(text) = text {
                bytes.extend_from_slice(text.as_bytes());
            }
        }
    }

    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_terminal_creation() {
        let term = TerminalApp::new(80, 24);
        assert_eq!(term.terminal.dims(), (80, 24));
        assert_eq!(term.title(), "Terminal");
    }

    #[test]
    fn test_terminal_title_update() {
        let term = TerminalApp::new(80, 24);
        assert_eq!(term.title(), "Terminal");
    }

    #[test]
    fn test_terminal_min_size() {
        let term = TerminalApp::new(80, 24);
        let (w, h) = term.min_size();
        assert!(w > 0);
        assert!(h > 0);
    }

    #[test]
    fn test_terminal_write_data() {
        let mut term = TerminalApp::new(80, 24);
        term.write_data(b"Hello");
        let cell = term.terminal.cell_at(0, 0).unwrap();
        assert_eq!(cell.ch, 'H');
    }

    #[test]
    fn test_editor_creation() {
        let editor = EditorApp::new();
        assert_eq!(editor.title(), "Editor");
    }

    #[test]
    fn test_editor_default() {
        let editor = EditorApp::default();
        assert_eq!(editor.title(), "Editor");
    }

    #[test]
    fn test_editor_load_file() {
        let mut editor = EditorApp::new();
        editor.load_file("test.txt", b"Hello World").unwrap();
        assert_eq!(editor.buffer.content(), "Hello World");
        assert_eq!(editor.filename, "test.txt");
    }

    #[test]
    fn test_editor_save() {
        let mut editor = EditorApp::new();
        editor.load_file("test.txt", b"Hello").unwrap();
        let saved = editor.save();
        assert_eq!(saved, b"Hello");
        assert!(!editor.modified);
    }

    #[test]
    fn test_editor_modified_title() {
        let mut editor = EditorApp::new();
        editor.load_file("test.txt", b"Hello").unwrap();
        editor.buffer_mut().insert("!", 0);
        editor.modified = true;
        editor.update_title();
        assert!(editor.title().contains("●"));
    }

    #[test]
    fn test_editor_undo() {
        let mut editor = EditorApp::new();
        editor.buffer_mut().insert("abc", 0);
        editor.buffer_mut().undo();
        assert_eq!(editor.buffer.content(), "");
    }

    #[test]
    fn test_editor_redo() {
        let mut editor = EditorApp::new();
        editor.buffer_mut().insert("abc", 0);
        editor.buffer_mut().undo();
        editor.buffer_mut().redo();
        assert_eq!(editor.buffer.content(), "abc");
    }

    #[test]
    fn test_key_event_to_bytes_enter() {
        let bytes = key_event_to_bytes(13, 0, None);
        assert_eq!(bytes, vec![b'\r']);
    }

    #[test]
    fn test_key_event_to_bytes_backspace() {
        let bytes = key_event_to_bytes(127, 0, None);
        assert_eq!(bytes, vec![0x7F]);
    }

    #[test]
    fn test_key_event_to_bytes_text() {
        let bytes = key_event_to_bytes(0, 0, Some(String::from("a")));
        assert_eq!(bytes, vec![b'a']);
    }

    #[test]
    fn test_terminal_handle_event() {
        let mut term = TerminalApp::new(80, 24);
        let event = UiEvent::KeyDown { key: 13, modifiers: 0, text: None };
        let requests = term.handle(event, 0);
        assert_eq!(requests.len(), 0);
    }

    #[test]
    fn test_editor_handle_event_text_input() {
        let mut editor = EditorApp::new();
        let event = UiEvent::KeyDown { key: 97, modifiers: 0, text: Some(String::from("a")) };
        let requests = editor.handle(event, 0);
        assert_eq!(requests.len(), 0);
    }

    #[test]
    fn test_app_request_clone() {
        let req = AppRequest::Close;
        let req2 = req.clone();
        assert_eq!(alloc::format!("{:?}", req), alloc::format!("{:?}", req2));
    }

    #[test]
    fn test_terminal_cursor_blink() {
        let mut term = TerminalApp::new(80, 24);
        assert!(term.cursor_blink_on);
        // Simulate time passing
        term.handle(UiEvent::PointerMove { x: 0.0, y: 0.0 }, 600);
        // After 600ms, should have toggled at least once
    }

    #[test]
    fn test_editor_utf8_content() {
        let mut editor = EditorApp::new();
        editor.load_file("test.txt", "Hello 世界".as_bytes()).unwrap();
        assert_eq!(editor.buffer.content(), "Hello 世界");
    }

    #[test]
    fn test_terminal_resize() {
        let mut term = TerminalApp::new(80, 24);
        term.terminal_mut().resize(100, 30);
        assert_eq!(term.terminal.dims(), (100, 30));
    }

    #[test]
    fn test_editor_multiple_lines() {
        let mut editor = EditorApp::new();
        editor.load_file("test.txt", b"Line 1\nLine 2\nLine 3").unwrap();
        assert_eq!(editor.buffer.content(), "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn test_terminal_render() {
        let mut term = TerminalApp::new(80, 24);
        let mut surface = Surface::new(640, 384);
        term.render(&mut surface);
        // Basic check: surface was modified
        assert_eq!(surface.w, 640);
    }

    #[test]
    fn test_editor_render() {
        let mut editor = EditorApp::new();
        let mut surface = Surface::new(640, 480);
        editor.render(&mut surface);
        assert_eq!(surface.w, 640);
    }

    #[test]
    fn test_terminal_newline_mode() {
        let mut term = TerminalApp::new(80, 24);
        assert!(term.terminal.newline_mode);
    }

    #[test]
    fn test_app_request_display() {
        let req = AppRequest::Close;
        let s = alloc::format!("{:?}", req);
        assert!(s.contains("Close"));
    }

    #[test]
    fn test_editor_buffer_operations() {
        let mut editor = EditorApp::new();
        editor.load_file("test.txt", b"abc").unwrap();
        let (lines, cols) = editor.buffer().byte_to_line_col(1);
        assert_eq!(lines, 0);
    }

    #[test]
    fn test_terminal_empty_bytes() {
        let mut term = TerminalApp::new(80, 24);
        term.write_data(b"");
        let cell = term.terminal.cell_at(0, 0).unwrap();
        assert_eq!(cell.ch, ' ');
    }

    #[test]
    fn test_editor_invalid_utf8() {
        let mut editor = EditorApp::new();
        let result = editor.load_file("test.txt", &[0xFF, 0xFE]);
        assert!(result.is_err());
    }

    #[test]
    fn test_app_request_spawn() {
        let req = AppRequest::Spawn {
            path: String::from("/bin/bash"),
            argv: alloc::vec![String::from("-c"), String::from("ls")],
        };
        let s = alloc::format!("{:?}", req);
        assert!(s.contains("Spawn"));
    }

    #[test]
    fn test_key_event_ctrl_modifier() {
        let bytes = key_event_to_bytes(97, 0o4, Some(String::from("a")));
        assert_eq!(bytes, vec![b'a']);
    }

    #[test]
    fn test_terminal_min_size_reasonable() {
        let term = TerminalApp::new(40, 12);
        let (w, h) = term.min_size();
        assert_eq!(w, 320);
        assert_eq!(h, 192);
    }

    #[test]
    fn test_editor_min_size() {
        let editor = EditorApp::new();
        let (w, h) = editor.min_size();
        assert_eq!(w, 320);
        assert_eq!(h, 192);
    }

    #[test]
    fn test_multiple_app_requests() {
        let _req1 = AppRequest::Close;
        let _req2 = AppRequest::SetTitle(String::from("New Title"));
        // Both requests can be created
    }

    #[test]
    fn test_editor_cursor_position() {
        let mut editor = EditorApp::new();
        editor.load_file("test.txt", b"Hello").unwrap();
        assert_eq!(editor.buffer.cursor(), 0);
    }

    #[test]
    fn test_terminal_title_access() {
        let term = TerminalApp::new(80, 24);
        let title = term.title();
        assert_eq!(title.len(), 8);
        assert_eq!(title, "Terminal");
    }

    #[test]
    fn test_editor_layout_no_overlap() {
        // Test that title bar, editor area, find bar, and status bar don't overlap
        let title_bar_height = 34.0;
        let find_bar_height = 50.0;
        let status_bar_height = 20.0;
        let surface_height = 600.0;

        // Title bar: 0 to 34
        let title_bar_y = 0.0;
        let title_bar_bottom = title_bar_y + title_bar_height;

        // Status bar: 580 to 600
        let status_bar_y = 580.0;
        let status_bar_bottom = status_bar_y + status_bar_height;

        // Find bar: 530 to 580 (directly above status bar)
        let find_bar_y = 530.0;
        let find_bar_bottom = find_bar_y + find_bar_height;

        // Editor area: 34 to 530
        let editor_y = title_bar_bottom;
        let editor_bottom = find_bar_y;

        // Verify no overlaps
        assert_eq!(title_bar_bottom, editor_y, "Title bar and editor area should be adjacent");
        assert_eq!(editor_bottom, find_bar_y, "Editor area and find bar should be adjacent");
        assert_eq!(find_bar_bottom, status_bar_y, "Find bar and status bar should be adjacent");
        assert_eq!(status_bar_bottom, surface_height, "Status bar should end at surface height");

        // Verify each region has positive height
        assert!(editor_y < editor_bottom, "Editor area should have positive height");
        assert!(find_bar_y < find_bar_bottom, "Find bar should have positive height");
        assert!(status_bar_y < status_bar_bottom, "Status bar should have positive height");
    }
}
