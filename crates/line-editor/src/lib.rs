//! Pure line editor with no OS dependencies. Can be tested on the host.
//! Handles cursor movement, history navigation, and screen redraw with ANSI
//! escape sequences.

use std::string::String;
use std::vec::Vec;

/// A pure, stateful line editor with no OS calls.
#[derive(Debug)]
pub struct LineEditor {
    /// The current line being edited
    line: Vec<u8>,
    /// Cursor position within the line (0 <= cursor <= line.len())
    cursor: usize,
    /// Command history (up to 64 entries)
    pub history: Vec<String>,
    /// Index into history while walking (None = not in history)
    history_index: Option<usize>,
}

impl LineEditor {
    /// Create a new, empty line editor with the given history capacity.
    pub fn new() -> Self {
        LineEditor {
            line: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_index: None,
        }
    }

    /// Get the current line as a string.
    pub fn line(&self) -> String {
        String::from_utf8_lossy(&self.line).into_owned()
    }

    /// Add a command to history (keeps last 64).
    pub fn add_to_history(&mut self, cmd: &str) {
        if !cmd.is_empty() {
            if self.history.len() >= 64 {
                self.history.remove(0);
            }
            self.history.push(cmd.to_string());
        }
        self.history_index = None;
    }

    /// Clear the current line and reset cursor.
    pub fn clear_line(&mut self) {
        self.line.clear();
        self.cursor = 0;
        self.history_index = None;
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, ch: u8) {
        self.history_index = None;
        self.line.insert(self.cursor, ch);
        self.cursor += 1;
    }

    /// Delete character before the cursor (Backspace).
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.history_index = None;
            self.cursor -= 1;
            self.line.remove(self.cursor);
        }
    }

    /// Delete character at the cursor (Delete key).
    pub fn delete(&mut self) {
        if self.cursor < self.line.len() {
            self.history_index = None;
            self.line.remove(self.cursor);
        }
    }

    /// Move cursor left.
    pub fn cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor right.
    pub fn cursor_right(&mut self) {
        if self.cursor < self.line.len() {
            self.cursor += 1;
        }
    }

    /// Move cursor to start of line (Ctrl-A / Home).
    pub fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to end of line (Ctrl-E / End).
    pub fn cursor_end(&mut self) {
        self.cursor = self.line.len();
    }

    /// Clear the line (Ctrl-U).
    pub fn ctrl_u(&mut self) {
        self.line.clear();
        self.cursor = 0;
        self.history_index = None;
    }

    /// Walk up in history (Up arrow).
    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }

        let new_index = match self.history_index {
            None => self.history.len() - 1,
            Some(idx) if idx > 0 => idx - 1,
            Some(idx) => idx,
        };

        self.history_index = Some(new_index);
        let cmd = self.history[new_index].clone();
        self.line = cmd.into_bytes();
        self.cursor = self.line.len();
    }

    /// Walk down in history (Down arrow).
    pub fn history_down(&mut self) {
        match self.history_index {
            None => {},
            Some(idx) if idx + 1 < self.history.len() => {
                let new_index = idx + 1;
                self.history_index = Some(new_index);
                let cmd = self.history[new_index].clone();
                self.line = cmd.into_bytes();
                self.cursor = self.line.len();
            }
            Some(_) => {
                // At the end, go back to empty
                self.history_index = None;
                self.line.clear();
                self.cursor = 0;
            }
        }
    }

    /// Get the current cursor position.
    pub fn cursor_pos(&self) -> usize {
        self.cursor
    }

    /// Get the line length.
    pub fn line_len(&self) -> usize {
        self.line.len()
    }
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_chars() {
        let mut ed = LineEditor::new();
        ed.insert_char(b'h');
        ed.insert_char(b'i');
        assert_eq!(ed.line(), "hi");
        assert_eq!(ed.cursor_pos(), 2);
    }

    #[test]
    fn test_cursor_movement() {
        let mut ed = LineEditor::new();
        ed.insert_char(b'a');
        ed.insert_char(b'b');
        ed.insert_char(b'c');
        assert_eq!(ed.cursor_pos(), 3);

        ed.cursor_left();
        assert_eq!(ed.cursor_pos(), 2);

        ed.cursor_left();
        assert_eq!(ed.cursor_pos(), 1);

        ed.cursor_right();
        assert_eq!(ed.cursor_pos(), 2);

        ed.cursor_home();
        assert_eq!(ed.cursor_pos(), 0);

        ed.cursor_end();
        assert_eq!(ed.cursor_pos(), 3);
    }

    #[test]
    fn test_backspace() {
        let mut ed = LineEditor::new();
        ed.insert_char(b'h');
        ed.insert_char(b'e');
        ed.insert_char(b'l');
        ed.insert_char(b'l');
        ed.insert_char(b'o');
        assert_eq!(ed.line(), "hello");

        ed.backspace();
        assert_eq!(ed.line(), "hell");
        assert_eq!(ed.cursor_pos(), 4);
    }

    #[test]
    fn test_delete() {
        let mut ed = LineEditor::new();
        ed.insert_char(b'h');
        ed.insert_char(b'e');
        ed.insert_char(b'l');
        ed.insert_char(b'l');
        ed.insert_char(b'o');

        ed.cursor_home();
        ed.delete();
        assert_eq!(ed.line(), "ello");
    }

    #[test]
    fn test_mid_line_edit() {
        let mut ed = LineEditor::new();
        ed.insert_char(b'h');
        ed.insert_char(b'l');
        ed.insert_char(b'l');
        ed.insert_char(b'o');

        ed.cursor_left();
        ed.cursor_left();
        ed.cursor_left();
        ed.insert_char(b'e');

        assert_eq!(ed.line(), "hello");
        assert_eq!(ed.cursor_pos(), 2);
    }

    #[test]
    fn test_ctrl_u() {
        let mut ed = LineEditor::new();
        ed.insert_char(b'h');
        ed.insert_char(b'e');
        ed.insert_char(b'l');
        ed.insert_char(b'l');
        ed.insert_char(b'o');

        ed.ctrl_u();
        assert_eq!(ed.line(), "");
        assert_eq!(ed.cursor_pos(), 0);
    }

    #[test]
    fn test_history() {
        let mut ed = LineEditor::new();

        ed.insert_char(b'f');
        ed.insert_char(b'i');
        ed.insert_char(b'r');
        ed.insert_char(b's');
        ed.insert_char(b't');
        ed.add_to_history("first");
        ed.clear_line();

        ed.insert_char(b's');
        ed.insert_char(b'e');
        ed.insert_char(b'c');
        ed.insert_char(b'o');
        ed.insert_char(b'n');
        ed.insert_char(b'd');
        ed.add_to_history("second");
        ed.clear_line();

        ed.history_up();
        assert_eq!(ed.line(), "second");

        ed.history_up();
        assert_eq!(ed.line(), "first");

        ed.history_down();
        assert_eq!(ed.line(), "second");

        ed.history_down();
        assert_eq!(ed.line(), "");
    }

    #[test]
    fn test_history_limit() {
        let mut ed = LineEditor::new();

        // Add 70 items
        for i in 0..70 {
            ed.clear_line();
            let s = format!("cmd{}", i);
            for ch in s.bytes() {
                ed.insert_char(ch);
            }
            ed.add_to_history(&s);
        }

        // Should only have last 64
        assert_eq!(ed.history.len(), 64);
    }
}
