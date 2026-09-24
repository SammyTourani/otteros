#![no_std]
#![cfg_attr(test, allow(unused))]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use otter_ui::*;
use otter_gfx::color::Color;
use otter_gfx::surface::Surface;
use otter_gfx::text::GlyphCache;

// ===== App Trait (from M4-T6a) =====

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppRequest {
    Close,
    SetTitle(String),
}

pub trait App {
    fn title(&self) -> String;
    fn handle(&mut self, event: Event, now_ms: u64) -> Vec<AppRequest>;
    fn render(&mut self, surface: &mut Surface, fonts: &Fonts, cache: &mut GlyphCache);
    fn min_size(&self) -> (u32, u32);
}

// ===== Assistant Trait =====

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssistantEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolRequest {
        id: u32,
        name: String,
        args: String,
        mutating: bool,
    },
    Done {
        stop_reason: String,
    },
    Error(String),
}

pub trait Assistant {
    fn start(&mut self, conversation: String);
    fn poll(&mut self) -> Vec<AssistantEvent>;
    fn stop(&mut self);
    fn allow_tool(&mut self, tool_id: u32, result: String);
    fn deny_tool(&mut self, tool_id: u32);
}

// ===== FileSystem Trait =====

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: u64,
}

pub trait FileSystem {
    fn list(&mut self, path: &str) -> Result<Vec<FileEntry>, String>;
    fn read(&mut self, path: &str) -> Result<Vec<u8>, String>;
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), String>;
    fn delete(&mut self, path: &str) -> Result<(), String>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), String>;
}

// ===== Otter Assistant App =====

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    pub id: u32,
    pub name: String,
    pub args: String,
    pub mutating: bool,
    pub state: String,
    pub result: String,
}

#[allow(dead_code)]
pub struct OtterApp {
    title: String,
    messages: Vec<Message>,
    composer_text: String,
    composer_focused: bool,
    backend: String,
    streaming: bool,
    tokens_per_sec: f32,
    current_thinking: String,
    tool_calls: Vec<ToolCall>,
    next_tool_id: u32,
    scroll_y: f32,
    selected_tool_idx: Option<usize>,
    action_log_scroll: f32,
}

impl OtterApp {
    pub fn new() -> Self {
        OtterApp {
            title: "Otter".to_string(),
            messages: Vec::new(),
            composer_text: String::new(),
            composer_focused: true,
            backend: "claude".to_string(),
            streaming: false,
            tokens_per_sec: 0.0,
            current_thinking: String::new(),
            tool_calls: Vec::new(),
            next_tool_id: 0,
            scroll_y: 0.0,
            selected_tool_idx: None,
            action_log_scroll: 0.0,
        }
    }

    pub fn add_user_message(&mut self, text: String) {
        self.messages.push(Message {
            role: "user".to_string(),
            content: text,
        });
    }

    pub fn add_assistant_message(&mut self, text: String) {
        self.messages.push(Message {
            role: "assistant".to_string(),
            content: text,
        });
    }

    pub fn add_tool_call(&mut self, name: String, args: String, mutating: bool) -> u32 {
        let id = self.next_tool_id;
        self.next_tool_id += 1;
        self.tool_calls.push(ToolCall {
            id,
            name,
            args,
            mutating,
            state: "pending".to_string(),
            result: String::new(),
        });
        id
    }

    pub fn allow_tool(&mut self, id: u32, result: String) {
        for tool in &mut self.tool_calls {
            if tool.id == id {
                tool.state = "allowed".to_string();
                tool.result = result.clone();
            }
        }
    }

    pub fn deny_tool(&mut self, id: u32, reason: String) {
        for tool in &mut self.tool_calls {
            if tool.id == id {
                tool.state = "denied".to_string();
                tool.result = reason.clone();
            }
        }
    }

    pub fn set_streaming(&mut self, streaming: bool, tokens_per_sec: f32) {
        self.streaming = streaming;
        self.tokens_per_sec = tokens_per_sec;
    }

    pub fn set_thinking(&mut self, thinking: String) {
        self.current_thinking = thinking;
    }

    pub fn get_messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn get_tool_calls(&self) -> &[ToolCall] {
        &self.tool_calls
    }
}

impl Default for OtterApp {
    fn default() -> Self {
        Self::new()
    }
}

impl App for OtterApp {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn handle(&mut self, event: Event, _now_ms: u64) -> Vec<AppRequest> {
        match event {
            Event::PointerDown { x: _, y, button: 0 } => {
                if y > 400.0 && y < 440.0 {
                    self.composer_focused = true;
                }
                Vec::new()
            }
            Event::KeyDown { key: 13, text: Some(_), modifiers } if self.composer_focused => {
                if modifiers & 4 == 0 {
                    if !self.composer_text.is_empty() {
                        self.add_user_message(self.composer_text.clone());
                        self.composer_text.clear();
                    }
                } else {
                    self.composer_text.push('\n');
                }
                Vec::new()
            }
            Event::KeyDown { key: 8, .. } if self.composer_focused => {
                self.composer_text.pop();
                Vec::new()
            }
            Event::KeyDown { text: Some(ch), .. } if self.composer_focused => {
                self.composer_text.push_str(&ch);
                Vec::new()
            }
            Event::Wheel { dy, .. } => {
                self.scroll_y = (self.scroll_y - dy).max(0.0);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn render(&mut self, surface: &mut Surface, fonts: &Fonts, cache: &mut GlyphCache) {
        otter_gfx::shapes::fill_rect(
            &mut surface.canvas(),
            0.0,
            0.0,
            800.0,
            600.0,
            theme::SURFACE_DARK,
        );

        let mut y = 20.0;
        for msg in &self.messages {
            let is_user = msg.role == "user";
            let bg_color = if is_user {
                theme::ACCENT
            } else {
                Color::rgb(0x1E, 0x2E, 0x35)
            };
            let text_color = if is_user {
                theme::text_on_accent()
            } else {
                theme::TEXT_INVERSE
            };

            otter_gfx::shapes::fill_rounded_rect(
                &mut surface.canvas(),
                if is_user { 400.0 } else { 20.0 },
                y,
                360.0,
                50.0,
                theme::CORNER_RADIUS_WIDGET,
                bg_color,
            );

            let content = &msg.content;
            let display_len = content.len().min(50);
            otter_gfx::text::draw_text(
                &mut surface.canvas(),
                cache,
                fonts.regular,
                0,
                &content[..display_len],
                if is_user { 420.0 } else { 40.0 },
                y + 28.0,
                14.0,
                0.0,
                text_color,
            );

            y += 60.0;
        }

        otter_gfx::shapes::fill_rounded_rect(
            &mut surface.canvas(),
            20.0,
            500.0,
            760.0,
            50.0,
            theme::CORNER_RADIUS_WIDGET,
            theme::SURFACE,
        );

        otter_gfx::text::draw_text(
            &mut surface.canvas(),
            cache,
            fonts.regular,
            0,
            &self.composer_text,
            40.0,
            530.0,
            14.0,
            0.0,
            theme::TEXT,
        );
    }

    fn min_size(&self) -> (u32, u32) {
        (800, 600)
    }
}

// ===== Files App =====

pub struct FilesApp {
    title: String,
    current_path: String,
    entries: Vec<FileEntry>,
    selected_idx: Option<usize>,
    sort_by: String,
    sort_desc: bool,
}

impl FilesApp {
    pub fn new(path: String) -> Self {
        FilesApp {
            title: alloc::format!("Files - {}", path),
            current_path: path,
            entries: Vec::new(),
            selected_idx: None,
            sort_by: String::new(),
            sort_desc: false,
        }
    }

    pub fn set_entries(&mut self, entries: Vec<FileEntry>) {
        self.entries = entries;
        self.selected_idx = None;
    }

    pub fn get_entries(&self) -> &[FileEntry] {
        &self.entries
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected_idx
    }

    pub fn sort(&mut self, column: &str) {
        if self.sort_by == column {
            self.sort_desc = !self.sort_desc;
        } else {
            self.sort_by = column.to_string();
            self.sort_desc = false;
        }

        match self.sort_by.as_str() {
            "name" => {
                self.entries.sort_by(|a, b| {
                    let cmp = a.name.cmp(&b.name);
                    if self.sort_desc { cmp.reverse() } else { cmp }
                });
            }
            "size" => {
                self.entries.sort_by(|a, b| {
                    let cmp = a.size.cmp(&b.size);
                    if self.sort_desc { cmp.reverse() } else { cmp }
                });
            }
            "date" => {
                self.entries.sort_by(|a, b| {
                    let cmp = a.mtime.cmp(&b.mtime);
                    if self.sort_desc { cmp.reverse() } else { cmp }
                });
            }
            _ => {}
        }
    }

    pub fn current_path(&self) -> &str {
        &self.current_path
    }

    pub fn navigate_to(&mut self, path: String) {
        self.current_path = path.clone();
        self.title = alloc::format!("Files - {}", path);
        self.entries.clear();
        self.selected_idx = None;
    }
}

impl Default for FilesApp {
    fn default() -> Self {
        Self::new("/".to_string())
    }
}

impl App for FilesApp {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn handle(&mut self, event: Event, _now_ms: u64) -> Vec<AppRequest> {
        match event {
            Event::PointerDown { x: _, y, button: 0 } => {
                let idx = ((y - 80.0) / 32.0) as usize;
                if idx < self.entries.len() {
                    self.selected_idx = Some(idx);
                }
                Vec::new()
            }
            Event::KeyDown { key: 40, .. } => {
                if let Some(idx) = self.selected_idx {
                    if idx + 1 < self.entries.len() {
                        self.selected_idx = Some(idx + 1);
                    }
                } else if !self.entries.is_empty() {
                    self.selected_idx = Some(0);
                }
                Vec::new()
            }
            Event::KeyDown { key: 38, .. } => {
                if let Some(idx) = self.selected_idx {
                    if idx > 0 {
                        self.selected_idx = Some(idx - 1);
                    }
                } else if !self.entries.is_empty() {
                    self.selected_idx = Some(0);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn render(&mut self, surface: &mut Surface, fonts: &Fonts, cache: &mut GlyphCache) {
        otter_gfx::shapes::fill_rect(
            &mut surface.canvas(),
            0.0,
            0.0,
            800.0,
            600.0,
            theme::SURFACE,
        );

        otter_gfx::text::draw_text(
            &mut surface.canvas(),
            cache,
            fonts.semibold,
            0,
            "Files",
            24.0,
            40.0,
            18.0,
            0.0,
            theme::TEXT,
        );

        otter_gfx::text::draw_text(
            &mut surface.canvas(),
            cache,
            fonts.regular,
            0,
            "Name",
            24.0,
            70.0,
            12.0,
            0.0,
            theme::TEXT_MUTED,
        );

        otter_gfx::text::draw_text(
            &mut surface.canvas(),
            cache,
            fonts.regular,
            0,
            "Size",
            400.0,
            70.0,
            12.0,
            0.0,
            theme::TEXT_MUTED,
        );

        for (i, entry) in self.entries.iter().enumerate() {
            let y = 80.0 + i as f32 * 32.0;
            let is_selected = self.selected_idx == Some(i);

            let bg_color = if is_selected { theme::ACCENT } else { theme::SURFACE };
            let text_color = if is_selected {
                theme::TEXT_INVERSE
            } else {
                theme::TEXT
            };

            otter_gfx::shapes::fill_rounded_rect(
                &mut surface.canvas(),
                24.0,
                y,
                752.0,
                28.0,
                theme::CORNER_RADIUS_WIDGET,
                bg_color,
            );

            otter_gfx::text::draw_text(
                &mut surface.canvas(),
                cache,
                fonts.regular,
                0,
                &entry.name,
                36.0,
                y + 18.0,
                14.0,
                0.0,
                text_color,
            );

            otter_gfx::text::draw_text(
                &mut surface.canvas(),
                cache,
                fonts.regular,
                0,
                &alloc::format!("{}", entry.size),
                400.0,
                y + 18.0,
                14.0,
                0.0,
                text_color,
            );
        }
    }

    fn min_size(&self) -> (u32, u32) {
        (600, 400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_otter_app_new() {
        let app = OtterApp::new();
        assert_eq!(app.title, "Otter");
        assert_eq!(app.messages.len(), 0);
    }

    #[test]
    fn test_otter_app_add_user_message() {
        let mut app = OtterApp::new();
        app.add_user_message("Hello".to_string());
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].role, "user");
    }

    #[test]
    fn test_otter_app_add_assistant_message() {
        let mut app = OtterApp::new();
        app.add_assistant_message("Hi".to_string());
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].role, "assistant");
    }

    #[test]
    fn test_otter_app_add_tool_call() {
        let mut app = OtterApp::new();
        let id = app.add_tool_call("write".to_string(), "{}".to_string(), true);
        assert_eq!(id, 0);
        assert_eq!(app.tool_calls.len(), 1);
    }

    #[test]
    fn test_otter_app_allow_tool() {
        let mut app = OtterApp::new();
        let id = app.add_tool_call("write".to_string(), "{}".to_string(), true);
        app.allow_tool(id, "ok".to_string());
        assert_eq!(app.tool_calls[0].state, "allowed");
    }

    #[test]
    fn test_otter_app_deny_tool() {
        let mut app = OtterApp::new();
        let id = app.add_tool_call("delete".to_string(), "{}".to_string(), true);
        app.deny_tool(id, "denied".to_string());
        assert_eq!(app.tool_calls[0].state, "denied");
    }

    #[test]
    fn test_otter_app_streaming() {
        let mut app = OtterApp::new();
        app.set_streaming(true, 25.0);
        assert!(app.streaming);
    }

    #[test]
    fn test_otter_app_thinking() {
        let mut app = OtterApp::new();
        app.set_thinking("...".to_string());
        assert_eq!(app.current_thinking, "...");
    }

    #[test]
    fn test_otter_app_title() {
        let app = OtterApp::new();
        assert_eq!(app.title(), "Otter");
    }

    #[test]
    fn test_otter_app_min_size() {
        let app = OtterApp::new();
        assert_eq!(app.min_size(), (800, 600));
    }

    #[test]
    fn test_files_app_new() {
        let app = FilesApp::new("/home".to_string());
        assert_eq!(app.current_path, "/home");
    }

    #[test]
    fn test_files_app_set_entries() {
        let mut app = FilesApp::new("/".to_string());
        let entries = vec![
            FileEntry {
                name: "file1.txt".to_string(),
                is_dir: false,
                size: 1024,
                mtime: 0,
            },
        ];
        app.set_entries(entries);
        assert_eq!(app.entries.len(), 1);
    }

    #[test]
    fn test_files_app_sort_by_name() {
        let mut app = FilesApp::new("/".to_string());
        let entries = vec![
            FileEntry {
                name: "c.txt".to_string(),
                is_dir: false,
                size: 100,
                mtime: 0,
            },
            FileEntry {
                name: "a.txt".to_string(),
                is_dir: false,
                size: 200,
                mtime: 0,
            },
        ];
        app.set_entries(entries);
        app.sort("name");
        assert_eq!(app.entries[0].name, "a.txt");
    }

    #[test]
    fn test_files_app_sort_by_size() {
        let mut app = FilesApp::new("/".to_string());
        let entries = vec![
            FileEntry {
                name: "a.txt".to_string(),
                is_dir: false,
                size: 300,
                mtime: 0,
            },
            FileEntry {
                name: "b.txt".to_string(),
                is_dir: false,
                size: 100,
                mtime: 0,
            },
        ];
        app.set_entries(entries);
        app.sort("size");
        assert_eq!(app.entries[0].size, 100);
    }

    #[test]
    fn test_files_app_sort_desc() {
        let mut app = FilesApp::new("/".to_string());
        let entries = vec![
            FileEntry {
                name: "a.txt".to_string(),
                is_dir: false,
                size: 100,
                mtime: 0,
            },
            FileEntry {
                name: "b.txt".to_string(),
                is_dir: false,
                size: 200,
                mtime: 0,
            },
        ];
        app.set_entries(entries);
        app.sort("size");
        app.sort("size");
        assert_eq!(app.entries[0].size, 200);
    }

    #[test]
    fn test_files_app_navigate() {
        let mut app = FilesApp::new("/home".to_string());
        app.navigate_to("/data".to_string());
        assert_eq!(app.current_path, "/data");
    }

    #[test]
    fn test_files_app_title() {
        let app = FilesApp::new("/home".to_string());
        assert!(app.title.contains("Files"));
    }

    #[test]
    fn test_files_app_min_size() {
        let app = FilesApp::new("/".to_string());
        assert_eq!(app.min_size(), (600, 400));
    }

    #[test]
    fn test_message_user() {
        let msg = Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
        };
        assert_eq!(msg.role, "user");
    }

    #[test]
    fn test_message_assistant() {
        let msg = Message {
            role: "assistant".to_string(),
            content: "Hi".to_string(),
        };
        assert_eq!(msg.role, "assistant");
    }

    #[test]
    fn test_file_entry() {
        let entry = FileEntry {
            name: "test.txt".to_string(),
            is_dir: false,
            size: 512,
            mtime: 1000,
        };
        assert!(!entry.is_dir);
    }

    #[test]
    fn test_file_entry_dir() {
        let entry = FileEntry {
            name: "mydir".to_string(),
            is_dir: true,
            size: 0,
            mtime: 1000,
        };
        assert!(entry.is_dir);
    }

    #[test]
    fn test_tool_call_pending() {
        let tool = ToolCall {
            id: 1,
            name: "write".to_string(),
            args: "{}".to_string(),
            mutating: true,
            state: "pending".to_string(),
            result: String::new(),
        };
        assert!(tool.mutating);
    }

    #[test]
    fn test_tool_call_read_only() {
        let tool = ToolCall {
            id: 2,
            name: "read".to_string(),
            args: "{}".to_string(),
            mutating: false,
            state: "done".to_string(),
            result: "content".to_string(),
        };
        assert!(!tool.mutating);
    }

    #[test]
    fn test_assistant_event_text() {
        let evt = AssistantEvent::TextDelta("hello".to_string());
        match evt {
            AssistantEvent::TextDelta(s) => assert_eq!(s, "hello"),
            _ => panic!(),
        }
    }

    #[test]
    fn test_assistant_event_tool() {
        let evt = AssistantEvent::ToolRequest {
            id: 1,
            name: "write".to_string(),
            args: "{}".to_string(),
            mutating: true,
        };
        match evt {
            AssistantEvent::ToolRequest { id, .. } => assert_eq!(id, 1),
            _ => panic!(),
        }
    }

    #[test]
    fn test_assistant_event_done() {
        let evt = AssistantEvent::Done {
            stop_reason: "end".to_string(),
        };
        match evt {
            AssistantEvent::Done { stop_reason } => assert_eq!(stop_reason, "end"),
            _ => panic!(),
        }
    }

    #[test]
    fn test_assistant_event_error() {
        let evt = AssistantEvent::Error("timeout".to_string());
        match evt {
            AssistantEvent::Error(msg) => assert_eq!(msg, "timeout"),
            _ => panic!(),
        }
    }

    #[test]
    fn test_app_request_close() {
        let req = AppRequest::Close;
        assert_eq!(req, AppRequest::Close);
    }

    #[test]
    fn test_app_request_title() {
        let req = AppRequest::SetTitle("New".to_string());
        match req {
            AppRequest::SetTitle(title) => assert_eq!(title, "New"),
            _ => panic!(),
        }
    }

    #[test]
    fn test_files_app_select() {
        let mut app = FilesApp::new("/".to_string());
        let entries = vec![
            FileEntry {
                name: "file1.txt".to_string(),
                is_dir: false,
                size: 100,
                mtime: 0,
            },
            FileEntry {
                name: "file2.txt".to_string(),
                is_dir: false,
                size: 200,
                mtime: 0,
            },
        ];
        app.set_entries(entries);
        app.handle(Event::PointerDown { x: 100.0, y: 112.0, button: 0 }, 0);
        assert_eq!(app.selected_idx, Some(1));
    }

    #[test]
    fn test_otter_app_multiple_messages() {
        let mut app = OtterApp::new();
        app.add_user_message("Q".to_string());
        app.add_assistant_message("A".to_string());
        assert_eq!(app.messages.len(), 2);
    }

    #[test]
    fn test_otter_app_multiple_tools() {
        let mut app = OtterApp::new();
        let id1 = app.add_tool_call("tool1".to_string(), "{}".to_string(), true);
        let id2 = app.add_tool_call("tool2".to_string(), "{}".to_string(), false);
        assert_eq!(id1, 0);
        assert_eq!(id2, 1);
    }

    #[test]
    fn test_files_app_get_entries() {
        let mut app = FilesApp::new("/".to_string());
        let entries = vec![FileEntry {
            name: "test.txt".to_string(),
            is_dir: false,
            size: 100,
            mtime: 0,
        }];
        app.set_entries(entries);
        assert_eq!(app.get_entries().len(), 1);
    }

    #[test]
    fn test_otter_app_get_messages() {
        let mut app = OtterApp::new();
        app.add_user_message("test".to_string());
        assert_eq!(app.get_messages().len(), 1);
    }

    #[test]
    fn test_otter_app_get_tool_calls() {
        let mut app = OtterApp::new();
        app.add_tool_call("tool".to_string(), "{}".to_string(), true);
        assert_eq!(app.get_tool_calls().len(), 1);
    }

    #[test]
    fn test_files_app_default() {
        let app = FilesApp::default();
        assert_eq!(app.current_path, "/");
    }

    #[test]
    fn test_files_app_sort_by_date() {
        let mut app = FilesApp::new("/".to_string());
        let entries = vec![
            FileEntry {
                name: "a.txt".to_string(),
                is_dir: false,
                size: 100,
                mtime: 3000,
            },
            FileEntry {
                name: "b.txt".to_string(),
                is_dir: false,
                size: 200,
                mtime: 1000,
            },
        ];
        app.set_entries(entries);
        app.sort("date");
        assert_eq!(app.entries[0].mtime, 1000);
    }

    #[test]
    fn test_otter_app_default() {
        let app = OtterApp::default();
        assert_eq!(app.title, "Otter");
    }

    #[test]
    fn test_app_request_eq() {
        assert_eq!(AppRequest::Close, AppRequest::Close);
    }
}
