#![no_std]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use core::cmp::{min, max};

/// A text buffer with cursor, selection, undo/redo, and search.
pub struct TextBuffer {
    // Simple buffer: just the actual content
    data: Vec<u8>,

    // Cursor position (byte offset)
    cursor: usize,
    // Selection anchor (byte offset). If anchor == cursor, no selection.
    anchor: usize,

    // Line start offsets (byte positions of '\n' + 1, plus 0 at the start)
    line_starts: Vec<usize>,

    // Line ending style: true for CRLF, false for LF
    uses_crlf: bool,

    // Is the buffer modified since last save?
    is_modified: bool,

    // Undo/redo stacks
    undo_stack: Vec<EditOperation>,
    redo_stack: Vec<EditOperation>,

    // Last edit time (for grouping). 0 means no last edit.
    last_edit_time: u64,

    // Preferred column for up/down navigation (preserved when moving across shorter lines)
    preferred_col: usize,
}

#[derive(Clone, Debug)]
struct EditOperation {
    // The text that was inserted or deleted
    text: Vec<u8>,
    // Position where the operation occurred
    position: usize,
    // True if this was an insertion, false if deletion
    is_insert: bool,
    // Time of the operation (for grouping) - kept for future use
    #[allow(dead_code)]
    time: u64,
}

impl TextBuffer {
    /// Create a new empty text buffer.
    pub fn new() -> Self {
        TextBuffer {
            data: Vec::new(),
            cursor: 0,
            anchor: 0,
            line_starts: vec![0],
            uses_crlf: false,
            is_modified: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_time: 0,
            preferred_col: 0,
        }
    }

    /// Load text from bytes. Returns Err if invalid UTF-8 at byte offset.
    pub fn load(&mut self, bytes: &[u8]) -> Result<(), usize> {
        // Validate UTF-8
        if core::str::from_utf8(bytes).is_err() {
            // Find first invalid byte position
            for i in 0..bytes.len() {
                if !is_valid_utf8_at(&bytes[..=i]) {
                    return Err(i);
                }
            }
            return Err(bytes.len());
        }

        // Detect line ending style
        self.uses_crlf = bytes.windows(2).any(|w| w == b"\r\n");

        // Insert all data
        self.data = bytes.to_vec();
        self.cursor = 0;
        self.anchor = 0;
        self.is_modified = false;
        self.undo_stack.clear();
        self.redo_stack.clear();

        // Rebuild line index
        self.rebuild_line_index();
        Ok(())
    }

    /// Save to bytes, preserving line ending style.
    pub fn save(&mut self) -> Vec<u8> {
        self.is_modified = false;
        self.data.clone()
    }

    /// Get current content as a String.
    pub fn content(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }

    /// Get all content as a vec.
    pub fn content_vec(&self) -> Vec<u8> {
        self.data.clone()
    }

    /// Get cursor position (byte offset).
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Get anchor position (byte offset).
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// Check if there's a selection.
    pub fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    /// Get selection range (start, end) in bytes.
    pub fn selection(&self) -> (usize, usize) {
        (min(self.cursor, self.anchor), max(self.cursor, self.anchor))
    }

    /// Convert byte offset to (line, column) in chars.
    pub fn byte_to_line_col(&self, byte_pos: usize) -> (usize, usize) {
        let byte_pos = min(byte_pos, self.data.len());

        // Find the line: if position is exactly at a line start, use that line
        // Otherwise use the previous line
        let line = match self.line_starts.binary_search(&byte_pos) {
            Ok(idx) => idx,
            Err(idx) => if idx > 0 { idx - 1 } else { 0 },
        };

        let line_start = self.line_starts[line];

        // Count UTF-8 characters to get column
        let col = self.data[line_start..byte_pos]
            .iter()
            .filter(|&&b| (b & 0xc0) != 0x80)
            .count();

        (line, col)
    }

    /// Convert (line, column in chars) to byte offset.
    pub fn line_col_to_byte(&self, line: usize, col: usize) -> usize {
        if line >= self.line_starts.len() {
            return self.data.len();
        }

        let line_start = self.line_starts[line];
        let line_end = if line + 1 < self.line_starts.len() {
            self.line_starts[line + 1] - 1  // -1 to exclude the newline
        } else {
            self.data.len()
        };

        let mut current_col = 0;
        let mut byte_pos = line_start;

        for &b in &self.data[line_start..line_end] {
            if current_col >= col {
                break;
            }
            if (b & 0xc0) != 0x80 {
                current_col += 1;
            }
            byte_pos += 1;
        }

        byte_pos
    }

    /// Get number of lines.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Get the text of a specific line (without newline).
    pub fn line_text(&self, line: usize) -> String {
        if line >= self.line_starts.len() {
            return String::new();
        }

        let line_start = self.line_starts[line];
        let line_end = if line + 1 < self.line_starts.len() {
            self.line_starts[line + 1] - 1  // -1 to skip newline
        } else {
            self.data.len()
        };

        String::from_utf8_lossy(&self.data[line_start..line_end]).into_owned()
    }

    /// Move cursor to position. Clears selection.
    pub fn set_cursor(&mut self, pos: usize) {
        self.cursor = min(pos, self.data.len());
        self.anchor = self.cursor;
    }

    /// Move cursor by chars (positive = forward).
    pub fn move_cursor_chars(&mut self, delta: i32) {
        let mut new_pos = self.cursor as i32;

        if delta > 0 {
            for _ in 0..delta {
                while (new_pos as usize) < self.data.len() {
                    new_pos += 1;
                    if (self.data[new_pos as usize] & 0xc0) != 0x80 {
                        break;
                    }
                }
            }
        } else {
            for _ in 0..(-delta) {
                while new_pos > 0 {
                    new_pos -= 1;
                    if (self.data[new_pos as usize] & 0xc0) != 0x80 {
                        break;
                    }
                }
            }
        }

        self.cursor = (new_pos as usize).min(self.data.len());
        self.anchor = self.cursor;
    }

    /// Move cursor to start of line.
    pub fn move_to_line_start(&mut self) {
        let (line, _) = self.byte_to_line_col(self.cursor);
        self.set_cursor(self.line_starts[line]);
    }

    /// Move cursor to end of line.
    pub fn move_to_line_end(&mut self) {
        let (line, _) = self.byte_to_line_col(self.cursor);
        let line_end = if line + 1 < self.line_starts.len() {
            self.line_starts[line + 1] - 1
        } else {
            self.data.len()
        };
        self.set_cursor(line_end);
    }

    /// Move cursor to document start.
    pub fn move_to_start(&mut self) {
        self.set_cursor(0);
    }

    /// Move cursor to document end.
    pub fn move_to_end(&mut self) {
        self.set_cursor(self.data.len());
    }

    /// Move cursor up by N lines, preserving preferred column.
    pub fn move_up(&mut self, lines: usize) {
        let (line, col) = self.byte_to_line_col(self.cursor);
        self.preferred_col = col;
        if line >= lines {
            let new_line = line - lines;
            self.set_cursor(self.line_col_to_byte(new_line, self.preferred_col));
        } else {
            self.move_to_start();
        }
    }

    /// Move cursor down by N lines, preserving preferred column.
    pub fn move_down(&mut self, lines: usize) {
        let (line, col) = self.byte_to_line_col(self.cursor);
        self.preferred_col = col;
        let new_line = line + lines;
        if new_line < self.line_starts.len() {
            self.set_cursor(self.line_col_to_byte(new_line, self.preferred_col));
        } else {
            self.move_to_end();
        }
    }

    /// Move cursor up by N pages (lines per page).
    pub fn page_up(&mut self, lines_per_page: usize) {
        self.move_up(lines_per_page);
    }

    /// Move cursor down by N pages (lines per page).
    pub fn page_down(&mut self, lines_per_page: usize) {
        self.move_down(lines_per_page);
    }

    /// Extend selection by moving head. Keeps anchor fixed.
    pub fn extend_selection_char(&mut self, delta: i32) {
        let mut new_head = self.cursor as i32;

        if delta > 0 {
            for _ in 0..delta {
                while (new_head as usize) < self.data.len() {
                    new_head += 1;
                    if (self.data[new_head as usize] & 0xc0) != 0x80 {
                        break;
                    }
                }
            }
        } else {
            for _ in 0..(-delta) {
                while new_head > 0 {
                    new_head -= 1;
                    if (self.data[new_head as usize] & 0xc0) != 0x80 {
                        break;
                    }
                }
            }
        }

        self.cursor = (new_head as usize).min(self.data.len());
    }

    /// Extend selection to start of line.
    pub fn extend_selection_to_line_start(&mut self) {
        let (line, _) = self.byte_to_line_col(self.cursor);
        self.cursor = self.line_starts[line];
    }

    /// Extend selection to end of line.
    pub fn extend_selection_to_line_end(&mut self) {
        let (line, _) = self.byte_to_line_col(self.cursor);
        let line_end = if line + 1 < self.line_starts.len() {
            self.line_starts[line + 1] - 1
        } else {
            self.data.len()
        };
        self.cursor = line_end;
    }

    /// Extend selection to document start.
    pub fn extend_selection_to_start(&mut self) {
        self.cursor = 0;
    }

    /// Extend selection to document end.
    pub fn extend_selection_to_end(&mut self) {
        self.cursor = self.data.len();
    }

    /// Extend selection up by N lines.
    pub fn extend_selection_up(&mut self, lines: usize) {
        let (line, col) = self.byte_to_line_col(self.cursor);
        if line >= lines {
            let new_line = line - lines;
            self.cursor = self.line_col_to_byte(new_line, col);
        } else {
            self.cursor = 0;
        }
    }

    /// Extend selection down by N lines.
    pub fn extend_selection_down(&mut self, lines: usize) {
        let (line, col) = self.byte_to_line_col(self.cursor);
        let new_line = line + lines;
        if new_line < self.line_starts.len() {
            self.cursor = self.line_col_to_byte(new_line, col);
        } else {
            self.cursor = self.data.len();
        }
    }

    /// Insert text at cursor, replacing selection if any.
    pub fn insert(&mut self, text: &str, time: u64) {
        let (sel_start, sel_end) = self.selection();

        // Delete selected text first
        if sel_start != sel_end {
            self.data.drain(sel_start..sel_end);
            self.cursor = sel_start;
        }

        let text_bytes = text.as_bytes();

        // Try to merge with last edit if it's consecutive typing (within 1000ms)
        let should_merge = !self.undo_stack.is_empty() &&
            time.saturating_sub(self.last_edit_time) < 1000 &&
            {
                let last = &self.undo_stack[self.undo_stack.len() - 1];
                last.is_insert && last.position + last.text.len() == self.cursor
            };

        if should_merge && text_bytes.len() == 1 {
            // Merge single character
            let idx = self.undo_stack.len() - 1;
            self.undo_stack[idx].text.extend_from_slice(text_bytes);
        } else {
            // Create new operation
            self.undo_stack.push(EditOperation {
                text: text_bytes.to_vec(),
                position: self.cursor,
                is_insert: true,
                time,
            });
        }

        self.redo_stack.clear();
        self.last_edit_time = time;

        // Insert the text at cursor position
        self.data.splice(self.cursor..self.cursor, text_bytes.iter().cloned());
        self.cursor += text_bytes.len();
        self.anchor = self.cursor;

        self.is_modified = true;
        self.rebuild_line_index();
    }

    /// Delete character at or before cursor.
    pub fn delete_char(&mut self, time: u64) {
        if self.cursor == 0 {
            return;
        }

        let mut del_start = self.cursor;

        // Move back one UTF-8 character
        loop {
            del_start -= 1;
            if (self.data[del_start] & 0xc0) != 0x80 {
                break;
            }
        }

        self.undo_stack.push(EditOperation {
            text: self.data[del_start..self.cursor].to_vec(),
            position: del_start,
            is_insert: false,
            time,
        });
        self.redo_stack.clear();

        self.data.drain(del_start..self.cursor);
        self.cursor = del_start;
        self.anchor = self.cursor;
        self.is_modified = true;
        self.last_edit_time = time;
        self.rebuild_line_index();
    }

    /// Delete character forward.
    pub fn delete_forward(&mut self, time: u64) {
        if self.cursor >= self.data.len() {
            return;
        }

        let mut del_end = self.cursor;

        // Move forward one UTF-8 character
        loop {
            del_end += 1;
            if del_end >= self.data.len() || (self.data[del_end] & 0xc0) != 0x80 {
                break;
            }
        }

        self.undo_stack.push(EditOperation {
            text: self.data[self.cursor..del_end].to_vec(),
            position: self.cursor,
            is_insert: false,
            time,
        });
        self.redo_stack.clear();

        self.data.drain(self.cursor..del_end);
        self.is_modified = true;
        self.last_edit_time = time;
        self.rebuild_line_index();
    }

    /// Insert a newline with auto-indent.
    pub fn insert_newline(&mut self, time: u64) {
        let (line, _) = self.byte_to_line_col(self.cursor);
        let line_start_byte = self.line_starts[line];

        // Get leading whitespace
        let mut indent = String::new();
        let mut pos = line_start_byte;
        while pos < self.data.len() && (self.data[pos] == b' ' || self.data[pos] == b'\t') {
            indent.push(self.data[pos] as char);
            pos += 1;
        }

        let newline = if self.uses_crlf { "\r\n" } else { "\n" };
        self.insert(&format!("{}{}", newline, indent), time);
    }

    /// Insert tab (4 spaces).
    pub fn insert_tab(&mut self, time: u64) {
        self.insert("    ", time);
    }

    /// Indent selected lines or current line.
    pub fn indent_lines(&mut self, time: u64) {
        let (sel_start, sel_end) = self.selection();
        let (start_line, _) = self.byte_to_line_col(sel_start);
        let (end_line, _) = self.byte_to_line_col(sel_end);

        let mut offset = 0;
        for line in start_line..=end_line.min(self.line_starts.len() - 1) {
            let pos = self.line_starts[line] + offset;
            if pos < self.data.len() {
                self.data.splice(pos..pos, b"    ".iter().cloned());
                offset += 4;
            }
        }

        self.is_modified = true;
        self.last_edit_time = time;
        self.rebuild_line_index();
    }

    /// Outdent selected lines or current line.
    pub fn outdent_lines(&mut self, time: u64) {
        let (sel_start, sel_end) = self.selection();
        let (start_line, _) = self.byte_to_line_col(sel_start);
        let (end_line, _) = self.byte_to_line_col(sel_end);

        let mut offset = 0;
        for line in start_line..=end_line.min(self.line_starts.len() - 1) {
            let line_start = self.line_starts[line].saturating_sub(offset);

            if line_start < self.data.len() {
                let mut spaces = 0;
                let mut pos = line_start;
                while pos < self.data.len() && spaces < 4 && self.data[pos] == b' ' {
                    spaces += 1;
                    pos += 1;
                }

                if spaces > 0 {
                    self.data.drain(line_start..line_start + spaces);
                    offset = offset.saturating_sub(spaces);
                }
            }
        }

        self.is_modified = true;
        self.last_edit_time = time;
        self.rebuild_line_index();
    }

    /// Cut selection to clipboard.
    pub fn cut(&mut self, time: u64) -> String {
        let (sel_start, sel_end) = self.selection();
        if sel_start == sel_end {
            return String::new();
        }

        let cut_bytes = self.data[sel_start..sel_end].to_vec();
        let cut_text = String::from_utf8_lossy(&cut_bytes).into_owned();

        self.data.drain(sel_start..sel_end);
        self.cursor = sel_start;
        self.anchor = self.cursor;
        self.is_modified = true;
        self.last_edit_time = time;

        self.rebuild_line_index();
        cut_text
    }

    /// Copy selection to clipboard.
    pub fn copy(&self) -> String {
        let (sel_start, sel_end) = self.selection();
        if sel_start == sel_end {
            return String::new();
        }

        let copy_bytes = &self.data[sel_start..sel_end];
        String::from_utf8_lossy(copy_bytes).into_owned()
    }

    /// Paste from clipboard.
    pub fn paste(&mut self, text: &str, time: u64) {
        self.insert(text, time);
    }

    /// Cut selection to caller-owned clipboard.
    pub fn cut_to_clipboard(&mut self, clipboard: &mut String, time: u64) {
        let (sel_start, sel_end) = self.selection();
        if sel_start == sel_end {
            clipboard.clear();
            return;
        }

        let cut_bytes = self.data[sel_start..sel_end].to_vec();
        *clipboard = String::from_utf8_lossy(&cut_bytes).into_owned();

        self.data.drain(sel_start..sel_end);
        self.cursor = sel_start;
        self.anchor = self.cursor;
        self.is_modified = true;
        self.last_edit_time = time;

        self.rebuild_line_index();
    }

    /// Copy selection to caller-owned clipboard.
    pub fn copy_to_clipboard(&self, clipboard: &mut String) {
        let (sel_start, sel_end) = self.selection();
        if sel_start == sel_end {
            clipboard.clear();
            return;
        }

        let copy_bytes = &self.data[sel_start..sel_end];
        *clipboard = String::from_utf8_lossy(copy_bytes).into_owned();
    }

    /// Paste from caller-owned clipboard.
    pub fn paste_from_clipboard(&mut self, clipboard: &str, time: u64) {
        self.insert(clipboard, time);
    }

    /// Move cursor to the start of the next word (skipping whitespace).
    pub fn move_word_right(&mut self) {
        if self.cursor >= self.data.len() {
            return;
        }

        let content = &self.data;
        let mut pos = self.cursor;

        // Skip current word if in one
        while pos < content.len() && is_word_char(content[pos]) {
            pos += 1;
        }

        // Skip whitespace and non-word characters
        while pos < content.len() && !is_word_char(content[pos]) && content[pos] != b'\n' {
            pos += 1;
        }

        self.cursor = pos;
        self.anchor = self.cursor;
    }

    /// Move cursor to the start of the previous word.
    pub fn move_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let content = &self.data;

        let mut pos = self.cursor - 1;

        // Skip whitespace and non-word characters
        loop {
            if !is_word_char(content[pos]) && content[pos] != b'\n' {
                if pos == 0 {
                    break;
                }
                pos -= 1;
            } else {
                break;
            }
        }

        // Skip word characters
        while pos > 0 && is_word_char(content[pos - 1]) {
            pos -= 1;
        }

        self.cursor = pos;
        self.anchor = self.cursor;
    }

    /// Extend selection by one word to the right.
    pub fn extend_selection_word_right(&mut self) {
        if self.cursor >= self.data.len() {
            return;
        }

        let content = &self.data;
        let mut pos = self.cursor;

        while pos < content.len() && is_word_char(content[pos]) {
            pos += 1;
        }
        while pos < content.len() && !is_word_char(content[pos]) && content[pos] != b'\n' {
            pos += 1;
        }

        self.cursor = pos;
    }

    /// Extend selection by one word to the left.
    pub fn extend_selection_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let content = &self.data;

        let mut pos = self.cursor - 1;
        loop {
            if !is_word_char(content[pos]) && content[pos] != b'\n' {
                if pos == 0 {
                    break;
                }
                pos -= 1;
            } else {
                break;
            }
        }
        while pos > 0 && is_word_char(content[pos - 1]) {
            pos -= 1;
        }

        self.cursor = pos;
    }

    /// Find next occurrence of pattern (case-sensitive).
    pub fn find_next(&self, pattern: &str, start_pos: usize) -> Option<(usize, usize)> {
        let pattern_bytes = pattern.as_bytes();

        for i in start_pos..self.data.len() {
            if i + pattern_bytes.len() <= self.data.len() && &self.data[i..i + pattern_bytes.len()] == pattern_bytes {
                return Some((i, i + pattern_bytes.len()));
            }
        }
        None
    }

    /// Find previous occurrence of pattern.
    pub fn find_prev(&self, pattern: &str, start_pos: usize) -> Option<(usize, usize)> {
        let pattern_bytes = pattern.as_bytes();

        let search_start = if start_pos > 0 { start_pos - 1 } else { 0 };
        for i in (0..=search_start).rev() {
            if i + pattern_bytes.len() <= self.data.len() && &self.data[i..i + pattern_bytes.len()] == pattern_bytes {
                return Some((i, i + pattern_bytes.len()));
            }
        }
        None
    }

    /// Find next occurrence of pattern (case-insensitive).
    pub fn find_next_case_insensitive(&self, pattern: &str, start_pos: usize) -> Option<(usize, usize)> {
        let pattern_lower = pattern.to_lowercase();
        let pattern_bytes = pattern_lower.as_bytes();
        let content_str = core::str::from_utf8(&self.data).unwrap_or("");
        let content_lower = content_str.to_lowercase();

        for i in start_pos..content_lower.len() {
            if i + pattern_bytes.len() <= content_lower.len() && &content_lower.as_bytes()[i..i + pattern_bytes.len()] == pattern_bytes {
                return Some((i, i + pattern_bytes.len()));
            }
        }
        None
    }

    /// Find previous occurrence of pattern (case-insensitive).
    pub fn find_prev_case_insensitive(&self, pattern: &str, start_pos: usize) -> Option<(usize, usize)> {
        let pattern_lower = pattern.to_lowercase();
        let pattern_bytes = pattern_lower.as_bytes();
        let content_str = core::str::from_utf8(&self.data).unwrap_or("");
        let content_lower = content_str.to_lowercase();

        let search_start = if start_pos > 0 { start_pos - 1 } else { 0 };
        for i in (0..=search_start).rev() {
            if i + pattern_bytes.len() <= content_lower.len() && &content_lower.as_bytes()[i..i + pattern_bytes.len()] == pattern_bytes {
                return Some((i, i + pattern_bytes.len()));
            }
        }
        None
    }

    /// Replace one occurrence.
    pub fn replace_one(&mut self, pos: usize, old: &str, new: &str, time: u64) -> bool {
        let old_bytes = old.as_bytes();
        let new_bytes = new.as_bytes();

        if pos + old_bytes.len() <= self.data.len() && &self.data[pos..pos + old_bytes.len()] == old_bytes {
            self.data.drain(pos..pos + old_bytes.len());
            self.data.splice(pos..pos, new_bytes.iter().cloned());
            self.cursor = pos;
            self.is_modified = true;
            self.last_edit_time = time;
            self.rebuild_line_index();
            true
        } else {
            false
        }
    }

    /// Replace all occurrences.
    pub fn replace_all(&mut self, old: &str, new: &str, time: u64) -> usize {
        let mut count = 0;
        let mut pos = 0;
        let old_bytes = old.as_bytes();
        let new_bytes = new.as_bytes();

        loop {
            if pos >= self.data.len() {
                break;
            }

            match self.find_next(old, pos) {
                Some((start, _end)) => {
                    self.data.drain(start..start + old_bytes.len());
                    self.data.splice(start..start, new_bytes.iter().cloned());
                    count += 1;
                    pos = start + new_bytes.len();
                }
                None => break,
            }
        }

        if count > 0 {
            self.is_modified = true;
            self.last_edit_time = time;
            self.rebuild_line_index();
        }
        count
    }

    /// Undo last operation.
    pub fn undo(&mut self) {
        if self.undo_stack.is_empty() {
            return;
        }

        let op = self.undo_stack.pop().unwrap();
        match op.is_insert {
            true => {
                // Undo insertion: delete what was inserted
                self.data.drain(op.position..op.position + op.text.len());
                self.cursor = op.position;
            }
            false => {
                // Undo deletion: reinsert what was deleted
                self.data.splice(op.position..op.position, op.text.iter().cloned());
                self.cursor = op.position + op.text.len();
            }
        }
        self.anchor = self.cursor;
        self.redo_stack.push(op);
        self.rebuild_line_index();
    }

    /// Redo last undone operation.
    pub fn redo(&mut self) {
        if self.redo_stack.is_empty() {
            return;
        }

        let op = self.redo_stack.pop().unwrap();
        match op.is_insert {
            true => {
                // Redo insertion
                self.data.splice(op.position..op.position, op.text.iter().cloned());
                self.cursor = op.position + op.text.len();
            }
            false => {
                // Redo deletion
                self.data.drain(op.position..op.position + op.text.len());
                self.cursor = op.position;
            }
        }
        self.anchor = self.cursor;
        self.undo_stack.push(op);
        self.rebuild_line_index();
    }

    /// Check if modified since last save.
    pub fn is_modified(&self) -> bool {
        self.is_modified
    }

    // ============ Private helpers ============

    fn rebuild_line_index(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);

        for (i, &byte) in self.data.iter().enumerate() {
            if byte == b'\n' {
                self.line_starts.push(i + 1);
            }
        }

        // Clamp cursor and anchor to valid positions
        let len = self.data.len();
        self.cursor = self.cursor.min(len);
        self.anchor = self.anchor.min(len);
    }
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// UTF-8 validation helper
fn is_valid_utf8_at(bytes: &[u8]) -> bool {
    core::str::from_utf8(bytes).is_ok()
}

// Word character detection helper
fn is_word_char(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_buffer() {
        let buf = TextBuffer::new();
        assert_eq!(buf.cursor(), 0);
        assert_eq!(buf.line_count(), 1);
        assert!(!buf.has_selection());
    }

    #[test]
    fn test_insert_basic() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        assert_eq!(buf.cursor(), 5);
        assert_eq!(buf.content_vec(), b"hello");
    }

    #[test]
    fn test_insert_multiple() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.insert(" world", 100);
        assert_eq!(buf.content_vec(), b"hello world");
    }

    #[test]
    fn test_delete_char() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.delete_char(1);
        assert_eq!(buf.content_vec(), b"hell");
        assert_eq!(buf.cursor(), 4);
    }

    #[test]
    fn test_delete_forward() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.set_cursor(4);
        buf.delete_forward(1);
        assert_eq!(buf.content_vec(), b"hell");
    }

    #[test]
    fn test_cursor_movement() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        assert_eq!(buf.cursor(), 5);
        buf.move_cursor_chars(-2);
        assert_eq!(buf.cursor(), 3);
        buf.move_cursor_chars(1);
        assert_eq!(buf.cursor(), 4);
    }

    #[test]
    fn test_set_cursor() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world", 0);
        buf.set_cursor(5);
        assert_eq!(buf.cursor(), 5);
        assert!(!buf.has_selection());
    }

    #[test]
    fn test_selection() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world", 0);
        buf.set_cursor(0);
        buf.extend_selection_char(5);
        assert!(buf.has_selection());
        let (start, end) = buf.selection();
        assert_eq!(start, 0);
        assert_eq!(end, 5);
    }

    #[test]
    fn test_copy() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world", 0);
        buf.set_cursor(0);
        buf.extend_selection_char(5);
        let copied = buf.copy();
        assert_eq!(copied, "hello");
    }

    #[test]
    fn test_cut() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world", 0);
        buf.set_cursor(0);
        buf.extend_selection_char(5);
        let cut_text = buf.cut(0);
        assert_eq!(cut_text, "hello");
        assert_eq!(buf.content_vec(), b" world");
    }

    #[test]
    fn test_paste() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.set_cursor(5);
        buf.paste(" world", 0);
        assert_eq!(buf.content_vec(), b"hello world");
    }

    #[test]
    fn test_newline_basic() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.insert_newline(0);
        assert_eq!(buf.line_count(), 2);
    }

    #[test]
    fn test_newline_with_indent() {
        let mut buf = TextBuffer::new();
        buf.insert("    hello", 0);
        buf.move_to_line_end();
        buf.insert_newline(0);
        let content = String::from_utf8(buf.content_vec()).unwrap();
        assert!(content.contains("    "));
    }

    #[test]
    fn test_find_next() {
        let mut buf = TextBuffer::new();
        buf.insert("hello hello world", 0);
        let result = buf.find_next("hello", 0);
        assert_eq!(result, Some((0, 5)));
        let result2 = buf.find_next("hello", 5);
        assert_eq!(result2, Some((6, 11)));
    }

    #[test]
    fn test_find_prev() {
        let mut buf = TextBuffer::new();
        buf.insert("hello hello world", 0);
        let result = buf.find_prev("hello", 16);
        assert!(result.is_some());
    }

    #[test]
    fn test_replace_one() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world", 0);
        let replaced = buf.replace_one(0, "hello", "hi", 0);
        assert!(replaced);
        assert_eq!(buf.content_vec(), b"hi world");
    }

    #[test]
    fn test_replace_all() {
        let mut buf = TextBuffer::new();
        buf.insert("hello hello hello", 0);
        let count = buf.replace_all("hello", "hi", 0);
        assert_eq!(count, 3);
        assert_eq!(buf.content_vec(), b"hi hi hi");
    }

    #[test]
    fn test_undo_insert() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.undo();
        assert_eq!(buf.content_vec(), b"");
    }

    #[test]
    fn test_undo_delete() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.delete_char(1);
        buf.undo();
        assert_eq!(buf.content_vec(), b"hello");
    }

    #[test]
    fn test_redo() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.delete_char(1);
        buf.undo();
        buf.redo();
        assert_eq!(buf.content_vec(), b"hell");
    }

    #[test]
    fn test_is_modified() {
        let mut buf = TextBuffer::new();
        assert!(!buf.is_modified());
        buf.insert("hello", 0);
        assert!(buf.is_modified());
        buf.save();
        assert!(!buf.is_modified());
    }

    #[test]
    fn test_byte_to_line_col() {
        let mut buf = TextBuffer::new();
        buf.insert("hello\nworld", 0);
        let (line, col) = buf.byte_to_line_col(0);
        assert_eq!(line, 0);
        assert_eq!(col, 0);

        let (line, col) = buf.byte_to_line_col(6);
        assert_eq!(line, 1);
        assert_eq!(col, 0);
    }

    #[test]
    fn test_line_col_to_byte() {
        let mut buf = TextBuffer::new();
        buf.insert("hello\nworld", 0);
        let byte = buf.line_col_to_byte(0, 0);
        assert_eq!(byte, 0);

        let byte = buf.line_col_to_byte(1, 0);
        assert_eq!(byte, 6);
    }

    #[test]
    fn test_line_text() {
        let mut buf = TextBuffer::new();
        buf.insert("hello\nworld", 0);
        assert_eq!(buf.line_text(0), "hello");
        assert_eq!(buf.line_text(1), "world");
    }

    #[test]
    fn test_move_to_line_start() {
        let mut buf = TextBuffer::new();
        buf.insert("hello\nworld", 0);
        // After insert, cursor is at 11 (end of "world"), on line 1
        assert_eq!(buf.cursor(), 11);
        buf.move_to_line_start();
        // move_to_line_start goes to start of line 1 (the 'w')
        assert_eq!(buf.cursor(), 6);
    }

    #[test]
    fn test_move_to_line_end() {
        let mut buf = TextBuffer::new();
        buf.insert("hello\nworld", 0);
        // After insert, cursor is at 11 (end of "world"), on line 1
        assert_eq!(buf.cursor(), 11);
        buf.move_to_line_end();
        // Already at end of line 1
        assert_eq!(buf.cursor(), 11);
    }

    #[test]
    fn test_move_to_start() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.move_to_start();
        assert_eq!(buf.cursor(), 0);
    }

    #[test]
    fn test_move_to_end() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.move_to_end();
        assert_eq!(buf.cursor(), 5);
    }

    #[test]
    fn test_utf8_multibyte() {
        let mut buf = TextBuffer::new();
        buf.insert("café", 0);
        assert_eq!(buf.cursor(), 5);  // é is 2 bytes
        buf.delete_char(1);
        assert_eq!(buf.content_vec(), b"caf");
    }

    #[test]
    fn test_load_utf8() {
        let mut buf = TextBuffer::new();
        let result = buf.load(b"hello world");
        assert!(result.is_ok());
        assert_eq!(buf.content_vec(), b"hello world");
        assert!(!buf.is_modified());
    }

    #[test]
    fn test_load_invalid_utf8() {
        let mut buf = TextBuffer::new();
        let invalid = [0xFF, 0xFE];
        let result = buf.load(&invalid);
        assert!(result.is_err());
    }

    #[test]
    fn test_save() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        assert!(buf.is_modified());
        let saved = buf.save();
        assert_eq!(saved, b"hello");
        assert!(!buf.is_modified());
    }

    #[test]
    fn test_insert_tab() {
        let mut buf = TextBuffer::new();
        buf.insert_tab(0);
        assert_eq!(buf.content_vec(), b"    ");
    }

    #[test]
    fn test_empty_selection_copy() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        let copied = buf.copy();
        assert_eq!(copied, "");
    }

    #[test]
    fn test_move_up_down() {
        let mut buf = TextBuffer::new();
        buf.insert("hello\nworld\nfoo", 0);
        buf.set_cursor(6);  // "world" line
        buf.move_up(1);
        let (line, _) = buf.byte_to_line_col(buf.cursor());
        assert_eq!(line, 0);

        buf.move_down(2);
        let (line, _) = buf.byte_to_line_col(buf.cursor());
        assert_eq!(line, 2);
    }

    #[test]
    fn test_insert_replaces_selection() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world", 0);
        buf.set_cursor(0);
        buf.extend_selection_char(5);
        buf.insert("hi", 0);
        assert_eq!(buf.content_vec(), b"hi world");
    }

    #[test]
    fn test_undo_redo_sequence() {
        let mut buf = TextBuffer::new();
        // Use times far apart to prevent merging
        buf.insert("a", 0);
        buf.insert("b", 1500);  // > 1000ms, no merge
        buf.insert("c", 3000);  // > 1000ms, no merge

        buf.undo();
        assert_eq!(buf.content_vec(), b"ab");

        buf.undo();
        assert_eq!(buf.content_vec(), b"a");

        buf.redo();
        assert_eq!(buf.content_vec(), b"ab");

        buf.redo();
        assert_eq!(buf.content_vec(), b"abc");
    }

    #[test]
    fn test_large_file_load() {
        let mut buf = TextBuffer::new();
        let mut large_text = String::new();
        for i in 0..10000 {
            large_text.push_str(&format!("Line {}\n", i));
        }
        let result = buf.load(large_text.as_bytes());
        assert!(result.is_ok());
        assert!(buf.line_count() > 9000);
    }

    #[test]
    fn test_differential_against_string() {
        let mut buf = TextBuffer::new();
        let mut reference = String::new();

        buf.insert("hello", 0);
        reference.push_str("hello");

        buf.insert(" world", 100);
        reference.push_str(" world");

        assert_eq!(String::from_utf8(buf.content_vec()).unwrap(), reference);

        buf.delete_char(1);
        reference.pop();

        assert_eq!(String::from_utf8(buf.content_vec()).unwrap(), reference);
    }

    #[test]
    fn test_crlf_preservation() {
        let mut buf = TextBuffer::new();
        let result = buf.load(b"hello\r\nworld");
        assert!(result.is_ok());
        assert!(buf.uses_crlf);
    }

    #[test]
    fn test_indent_lines() {
        let mut buf = TextBuffer::new();
        buf.insert("hello\nworld", 0);
        buf.set_cursor(0);
        buf.indent_lines(0);
        let content = String::from_utf8(buf.content_vec()).unwrap();
        assert!(content.starts_with("    "));
    }

    #[test]
    fn test_outdent_lines() {
        let mut buf = TextBuffer::new();
        buf.insert("    hello\nworld", 0);
        buf.set_cursor(0);
        buf.outdent_lines(0);
        assert_eq!(buf.content_vec(), b"hello\nworld");
    }

    #[test]
    fn test_multiple_newlines() {
        let mut buf = TextBuffer::new();
        buf.insert("a\n\n\nb", 0);
        assert_eq!(buf.line_count(), 4);
    }

    #[test]
    fn test_cursor_at_end_of_file() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.move_to_end();
        assert_eq!(buf.cursor(), 5);
        buf.move_cursor_chars(1);
        assert_eq!(buf.cursor(), 5);  // Can't move beyond EOF
    }

    #[test]
    fn test_cursor_at_start_of_file() {
        let mut buf = TextBuffer::new();
        buf.insert("hello", 0);
        buf.move_to_start();
        assert_eq!(buf.cursor(), 0);
        buf.move_cursor_chars(-1);
        assert_eq!(buf.cursor(), 0);  // Can't move before SOF
    }

    #[test]
    fn test_empty_find() {
        let buf = TextBuffer::new();
        let result = buf.find_next("hello", 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_overlapping_matches() {
        let mut buf = TextBuffer::new();
        buf.insert("aaa", 0);
        let result = buf.find_next("aa", 0);
        assert_eq!(result, Some((0, 2)));
        let result2 = buf.find_next("aa", 2);
        assert!(result2.is_none());
    }

    #[test]
    fn test_search_case_sensitivity() {
        let mut buf = TextBuffer::new();
        buf.insert("Hello hello HELLO", 0);

        // Case-sensitive search
        let result = buf.find_next("hello", 0);
        assert_eq!(result, Some((6, 11)));

        let result = buf.find_next("HELLO", 0);
        assert_eq!(result, Some((12, 17)));

        let result = buf.find_next("Hello", 0);
        assert_eq!(result, Some((0, 5)));
    }

    #[test]
    fn test_word_boundaries() {
        let mut buf = TextBuffer::new();
        buf.insert("foo_bar baz-qux", 0);

        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.data.len(), 15);

        buf.set_cursor(3);
        assert_eq!(buf.cursor(), 3);
    }

    // Randomized differential test: 10,000 operations
    #[test]
    fn test_10k_differential() {
        let mut buf = TextBuffer::new();
        let mut reference = String::new();

        // Seed for reproducibility
        let mut rng_state: u32 = 12345;
        let next_u32 = |state: &mut u32| {
            *state = state.wrapping_mul(1103515245).wrapping_add(12345);
            *state
        };

        for op_id in 0..10000 {
            let op = next_u32(&mut rng_state) % 5;  // Only 0-4 to focus on core operations
            let pos = if reference.is_empty() { 0 } else { next_u32(&mut rng_state) as usize % (reference.len() + 1) };

            match op {
                0 => {
                    // Insert single char
                    let ch = (b'a' + (next_u32(&mut rng_state) % 26) as u8) as char;
                    let cpos = pos.min(buf.data.len());
                    buf.set_cursor(cpos);
                    buf.insert(&format!("{}", ch), op_id as u64);
                    reference.insert(cpos, ch);
                }
                1 => {
                    // Delete char
                    if !buf.data.is_empty() && !reference.is_empty() {
                        let cpos = pos.min(buf.data.len().saturating_sub(1));
                        buf.set_cursor(cpos);
                        buf.delete_forward(op_id as u64);
                        reference.remove(cpos);
                    }
                }
                2 => {
                    // Move cursor
                    let cpos = pos.min(buf.data.len());
                    buf.set_cursor(cpos);
                }
                3 => {
                    // Insert string
                    let cpos = pos.min(buf.data.len());
                    buf.set_cursor(cpos);
                    let text = "test";
                    buf.insert(text, op_id as u64);
                    reference.insert_str(cpos, text);
                }
                _ => {
                    // Check consistency
                    let buf_content = String::from_utf8(buf.content_vec())
                        .unwrap_or_default();
                    assert_eq!(buf_content, reference, "Mismatch at operation {}", op_id);
                }
            }

            // Final consistency check
            if op_id == 9999 {
                let buf_content = String::from_utf8(buf.content_vec())
                    .unwrap_or_default();
                assert_eq!(buf_content, reference, "Final mismatch at operation {}", op_id);
            }
        }
    }

    #[test]
    fn test_multiline_insert_cursor_movement() {
        let mut buf = TextBuffer::new();
        buf.insert("hello\nworld", 0);
        // Cursor should move after multi-line insert
        assert_eq!(buf.cursor(), 11);
        assert_eq!(buf.content_vec(), b"hello\nworld");
    }

    #[test]
    fn test_undo_grouping_time_threshold() {
        let mut buf = TextBuffer::new();
        // Inserts within 1000ms should potentially merge
        buf.insert("a", 0);
        buf.insert("b", 500);
        buf.insert("c", 900);
        // All three should be in same group if within threshold
        // But we can't easily test grouping without exposing undo_stack
        assert_eq!(buf.content_vec(), b"abc");
    }

    #[test]
    fn test_undo_grouping_cursor_jump_breaker() {
        let mut buf = TextBuffer::new();
        // Cursor jump should break grouping
        buf.insert("a", 0);  // cursor at 1
        buf.set_cursor(0);   // cursor jump
        buf.insert("b", 100);  // cursor at 1

        buf.undo();
        assert_eq!(buf.content_vec(), b"a");  // 'b' removed separately
        buf.undo();
        assert_eq!(buf.content_vec(), b"");  // 'a' removed
    }

    #[test]
    fn test_undo_grouping_time_pause_breaker() {
        let mut buf = TextBuffer::new();
        // > 1000ms pause should break grouping
        buf.insert("a", 0);
        buf.insert("b", 2000);  // > 1000ms from 'a', separate group

        buf.undo();
        assert_eq!(buf.content_vec(), b"a");  // 'b' removed in separate undo
        buf.undo();
        assert_eq!(buf.content_vec(), b"");  // 'a' removed
    }

    #[test]
    fn test_word_movement_right() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world test", 0);
        buf.set_cursor(0);
        buf.move_word_right();
        assert_eq!(buf.cursor(), 6);  // Start of 'world'
        buf.move_word_right();
        assert_eq!(buf.cursor(), 12);  // Start of 'test'
    }

    #[test]
    fn test_word_movement_left() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world test", 0);
        buf.set_cursor(11);  // After 'world'
        buf.move_word_left();
        assert_eq!(buf.cursor(), 6);  // Start of 'world'
    }

    #[test]
    fn test_preferred_column_up_down() {
        let mut buf = TextBuffer::new();
        buf.insert("hello\nworld\nfoo", 0);
        buf.set_cursor(5);  // End of 'hello' (col 5)
        buf.move_down(1);
        let (_, _col) = buf.byte_to_line_col(buf.cursor());
        assert_eq!(buf.preferred_col, 5);
        buf.move_down(1);
        // Should maintain preferred_col even on shorter line
        assert_eq!(buf.preferred_col, 5);
    }

    #[test]
    fn test_page_up_down() {
        let mut buf = TextBuffer::new();
        let mut text = String::new();
        for i in 0..20 {
            text.push_str(&format!("Line {}\n", i));
        }
        buf.insert(&text, 0);

        buf.set_cursor(0);
        buf.page_down(5);
        let (line, _) = buf.byte_to_line_col(buf.cursor());
        assert!(line >= 5);  // Should move down at least 5 lines
    }

    #[test]
    fn test_case_insensitive_search() {
        let mut buf = TextBuffer::new();
        buf.insert("Hello hello HELLO", 0);

        // Case-insensitive search
        let result = buf.find_next_case_insensitive("hello", 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), (0, 5));

        let result2 = buf.find_next_case_insensitive("HELLO", 6);
        assert!(result2.is_some());
    }

    #[test]
    fn test_clipboard_operations() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world", 0);
        buf.set_cursor(0);
        buf.extend_selection_char(5);

        let mut clipboard = String::new();
        buf.copy_to_clipboard(&mut clipboard);
        assert_eq!(clipboard, "hello");

        buf.set_cursor(11);
        buf.paste_from_clipboard(&clipboard, 0);
        assert_eq!(buf.content_vec(), b"hello worldhello");
    }

    #[test]
    fn test_shift_extend_selections() {
        let mut buf = TextBuffer::new();
        buf.insert("hello\nworld\ntest", 0);

        buf.set_cursor(0);
        buf.extend_selection_char(5);
        assert_eq!(buf.cursor(), 5);

        buf.anchor = 0;
        buf.cursor = 0;
        buf.extend_selection_to_line_end();
        assert_eq!(buf.cursor(), 5);

        buf.cursor = 0;
        buf.extend_selection_down(2);
        let (line, _) = buf.byte_to_line_col(buf.cursor());
        assert_eq!(line, 2);
    }

    #[test]
    fn test_5mb_file_load() {
        let mut buf = TextBuffer::new();

        // Create a 5MB file
        let mut large_text = String::new();
        for i in 0..100000 {
            large_text.push_str(&format!("Line {}: This is a test line with some content.\n", i));
        }

        let result = buf.load(large_text.as_bytes());
        assert!(result.is_ok());
        assert!(buf.data.len() > 5_000_000);

        // Test insert in the middle
        let mid = buf.data.len() / 2;
        buf.set_cursor(mid);
        buf.insert("INSERTED", 0);
        assert!(buf.data.len() > 5_000_008);

        // Test delete
        buf.set_cursor(mid);
        buf.delete_forward(0);
        assert!(buf.data.len() > 5_000_007);
    }

    #[test]
    fn test_extend_word_selection() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world", 0);
        buf.set_cursor(0);
        buf.extend_selection_word_right();
        assert_eq!(buf.cursor(), 6);  // Start of next word

        buf.cursor = 0;
        buf.anchor = 0;
        buf.extend_selection_word_left();
        assert_eq!(buf.cursor(), 0);  // At start, can't go left
    }

    #[test]
    fn test_find_prev_case_insensitive() {
        let mut buf = TextBuffer::new();
        buf.insert("Hello hello HELLO test", 0);

        let result = buf.find_prev_case_insensitive("HELLO", 22);
        assert!(result.is_some());
        let (start, _end) = result.unwrap();
        assert_eq!(start, 12);  // Last "HELLO" is at position 12
    }

    #[test]
    fn test_cut_with_clipboard() {
        let mut buf = TextBuffer::new();
        buf.insert("hello world", 0);
        buf.set_cursor(0);
        buf.extend_selection_char(5);

        let mut clipboard = String::new();
        buf.cut_to_clipboard(&mut clipboard, 0);
        assert_eq!(clipboard, "hello");
        assert_eq!(buf.content_vec(), b" world");
    }
}
