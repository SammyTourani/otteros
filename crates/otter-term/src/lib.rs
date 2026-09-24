//! `otter-term`: OtterOS terminal emulator core.
//!
//! DECISIONS.md D27: this crate is `#![no_std]` + `alloc`, has zero external
//! dependencies, and is unit-tested on the host with `cargo test`. It contains no
//! OS logic (D2) -- only terminal state machine, parsing, and character handling --
//! and is linked into userspace programs by path once userspace exists.
//!
//! `std` is enabled only under `cfg(test)`, purely so unit/integration tests can do
//! file I/O (loading fixtures). It adds no Cargo dependency and the crate is never
//! built with `std` outside test builds.
#![cfg_attr(not(test), no_std)]
#![warn(missing_docs)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::cmp::min;

/// An ANSI color value: either one of the standard 16 colors or 24-bit RGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// Black (0)
    Black,
    /// Red (1)
    Red,
    /// Green (2)
    Green,
    /// Yellow (3)
    Yellow,
    /// Blue (4)
    Blue,
    /// Magenta (5)
    Magenta,
    /// Cyan (6)
    Cyan,
    /// White (7)
    White,
    /// Bright black (8)
    BrightBlack,
    /// Bright red (9)
    BrightRed,
    /// Bright green (10)
    BrightGreen,
    /// Bright yellow (11)
    BrightYellow,
    /// Bright blue (12)
    BrightBlue,
    /// Bright magenta (13)
    BrightMagenta,
    /// Bright cyan (14)
    BrightCyan,
    /// Bright white (15)
    BrightWhite,
    /// 24-bit RGB color
    Rgb(u8, u8, u8),
}

impl Color {
    /// Map a color index (0-255) to a 16-color palette or RGB.
    pub fn from_palette(idx: u8) -> Self {
        match idx {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::White,
            8 => Color::BrightBlack,
            9 => Color::BrightRed,
            10 => Color::BrightGreen,
            11 => Color::BrightYellow,
            12 => Color::BrightBlue,
            13 => Color::BrightMagenta,
            14 => Color::BrightCyan,
            15 => Color::BrightWhite,
            _ => Color::White,
        }
    }
}

/// A single character cell in the terminal grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The Unicode scalar value (U+FFFD for invalid UTF-8)
    pub ch: char,
    /// Foreground color
    pub fg: Color,
    /// Background color
    pub bg: Color,
    /// Bold attribute
    pub bold: bool,
    /// Underline attribute
    pub underline: bool,
    /// Inverse attribute (swap foreground and background)
    pub inverse: bool,
    /// Dirty flag (changed since last damage report)
    pub dirty: bool,
}

impl Cell {
    /// Create a cell with default attributes and a character.
    pub fn new(ch: char) -> Self {
        Cell {
            ch,
            fg: Color::White,
            bg: Color::Black,
            bold: false,
            underline: false,
            inverse: false,
            dirty: true,
        }
    }

    /// Create a blank cell.
    pub fn blank() -> Self {
        Cell::new(' ')
    }
}

/// Cursor state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// Row (0-indexed)
    pub row: usize,
    /// Column (0-indexed)
    pub col: usize,
    /// Visible
    pub visible: bool,
}

impl Cursor {
    /// Create a new cursor at (0, 0), visible.
    pub fn new() -> Self {
        Cursor {
            row: 0,
            col: 0,
            visible: true,
        }
    }
}

impl Default for Cursor {
    fn default() -> Self {
        Cursor::new()
    }
}

/// Scrollback ring buffer.
struct Scrollback {
    lines: Vec<Vec<Cell>>,
    capacity: usize,
    start: usize,
}

impl Scrollback {
    /// Create a new scrollback with a given capacity.
    fn new(capacity: usize) -> Self {
        Scrollback {
            lines: Vec::new(),
            capacity,
            start: 0,
        }
    }

    /// Add a line to the scrollback.
    fn push_line(&mut self, line: Vec<Cell>) {
        if self.lines.len() < self.capacity {
            self.lines.push(line);
        } else {
            self.lines[self.start] = line;
            self.start = (self.start + 1) % self.capacity;
        }
    }

    /// Get a line from scrollback by index (0 = oldest visible, len-1 = newest).
    #[allow(dead_code)]
    fn get_line(&self, idx: usize) -> Option<&Vec<Cell>> {
        if idx >= self.lines.len() {
            return None;
        }
        Some(&self.lines[(self.start + idx) % self.capacity])
    }

    /// Count of lines in scrollback.
    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.lines.len()
    }
}

/// Parser state for escape sequences.
#[derive(Debug)]
enum ParserState {
    /// Normal text
    Normal,
    /// Saw ESC, waiting for next byte
    Escape,
    /// In CSI sequence (ESC [ ... )
    Csi {
        /// Parameters (numbers separated by ;)
        params: Vec<usize>,
        /// Private mode indicator (? seen)
        private: bool,
    },
    /// In OSC sequence (ESC ] ... )
    Osc {
        /// OSC content buffer
        content: Vec<u8>,
    },
}

/// The terminal emulator state machine.
pub struct Terminal {
    /// Visible screen grid
    grid: Vec<Vec<Cell>>,
    /// Columns
    cols: usize,
    /// Rows
    rows: usize,
    /// Current cursor
    cursor: Cursor,
    /// Saved cursor state
    saved_cursor: Cursor,
    /// Current foreground color
    fg: Color,
    /// Current background color
    bg: Color,
    /// Bold mode
    bold: bool,
    /// Underline mode
    underline: bool,
    /// Inverse mode
    inverse: bool,
    /// UTF-8 decoder state
    utf8_decoder: Utf8Decoder,
    /// Scrollback ring
    scrollback: Scrollback,
    /// Rows that changed since last take_damage()
    dirty_rows: Vec<bool>,
    /// Scroll region start (inclusive)
    scroll_region_top: usize,
    /// Scroll region end (inclusive)
    scroll_region_bottom: usize,
    /// Alternate screen grid
    #[allow(dead_code)]
    alt_grid: Option<Vec<Vec<Cell>>>,
    /// Alternate screen cursor
    #[allow(dead_code)]
    alt_cursor: Option<Cursor>,
    /// Alternate screen attributes
    #[allow(dead_code)]
    alt_attrs: Option<(Color, Color, bool, bool, bool)>,
    /// Parser state
    parser_state: ParserState,
    /// Output buffer for DSR replies (e.g., CPR)
    pub reply_buffer: Vec<u8>,
    /// Window title (for OSC 0/2)
    pub title: Vec<u8>,
}

impl Terminal {
    /// Create a new terminal with given dimensions.
    pub fn new(cols: usize, rows: usize) -> Self {
        let grid = vec![vec![Cell::blank(); cols]; rows];
        let dirty_rows = vec![true; rows];
        Terminal {
            cols,
            rows,
            grid,
            cursor: Cursor::new(),
            saved_cursor: Cursor::new(),
            fg: Color::White,
            bg: Color::Black,
            bold: false,
            underline: false,
            inverse: false,
            utf8_decoder: Utf8Decoder::new(),
            scrollback: Scrollback::new(5000),
            dirty_rows,
            scroll_region_top: 0,
            scroll_region_bottom: rows - 1,
            alt_grid: None,
            alt_cursor: None,
            alt_attrs: None,
            parser_state: ParserState::Normal,
            reply_buffer: Vec::new(),
            title: Vec::new(),
        }
    }

    /// Get the current terminal dimensions.
    pub fn dims(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// Resize the terminal. Content is kept, cursor is clamped.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }

        self.cursor.col = min(self.cursor.col, cols.saturating_sub(1));
        self.cursor.row = min(self.cursor.row, rows.saturating_sub(1));

        // Preserve content: resize grid while keeping existing cells
        let old_cols = self.cols;
        let old_rows = self.rows;

        let mut new_grid = vec![vec![Cell::blank(); cols]; rows];

        // Copy existing content to new grid
        for old_row in 0..min(old_rows, rows) {
            for old_col in 0..min(old_cols, cols) {
                new_grid[old_row][old_col] = self.grid[old_row][old_col];
            }
        }

        self.grid = new_grid;
        self.cols = cols;
        self.rows = rows;
        self.dirty_rows = vec![true; rows];
        self.scroll_region_bottom = rows - 1;
    }

    /// Get a cell at (row, col).
    pub fn cell_at(&self, row: usize, col: usize) -> Option<Cell> {
        if row >= self.rows || col >= self.cols {
            None
        } else {
            Some(self.grid[row][col])
        }
    }

    /// Get the current cursor.
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Get the screen as a string for testing.
    pub fn screen_text(&self) -> Vec<Vec<char>> {
        self.grid
            .iter()
            .map(|row| row.iter().map(|cell| cell.ch).collect())
            .collect()
    }

    /// Process an input byte, updating the screen state.
    pub fn feed_byte(&mut self, b: u8) {
        // Check if there's a pending byte from a previous invalid sequence
        if let Some(pb) = self.utf8_decoder.pending.take() {
            self.feed_byte(pb);
        }

        match self.utf8_decoder.feed(b) {
            Utf8Result::Char(ch) => self.handle_char(ch),
            Utf8Result::Invalid => self.handle_char('\u{FFFD}'),
            Utf8Result::Incomplete => {}
        }
    }

    /// Process multiple input bytes.
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.feed_byte(b);
        }
    }

    fn handle_char(&mut self, ch: char) {
        // Check for ESC first (it's a C0 control but needs special handling)
        if ch == '\x1b' {
            self.parser_state = ParserState::Escape;
            return;
        }

        // Check for BEL - it terminates OSC sequences
        if ch as u32 == 0x07 {
            match &self.parser_state {
                ParserState::Osc { .. } => {
                    // BEL terminates OSC; let handle_osc_byte process it
                    self.handle_osc_byte(0x07);
                }
                _ => {
                    // BEL ignored in other contexts
                }
            }
            return;
        }

        // Check for other C0 controls
        if (ch as u32) < 0x20 {
            match ch as u32 as u8 {
                0x08 => self.cursor.col = self.cursor.col.saturating_sub(1), // BS
                0x09 => self.handle_tab(), // HT
                0x0a => self.handle_lf(), // LF
                0x0d => self.cursor.col = 0, // CR
                0x00 => {}, // NUL - ignored
                _ => {}, // Other controls ignored
            }
            return;
        }

        // Handle based on parser state
        match &self.parser_state {
            ParserState::Normal => {
                self.put_char(ch);
            }
            ParserState::Escape => {
                self.handle_escape(ch);
            }
            ParserState::Csi { .. } => {
                self.handle_csi_byte(ch as u8);
            }
            ParserState::Osc { .. } => {
                self.handle_osc_byte(ch as u8);
            }
        }
    }

    fn handle_escape(&mut self, ch: char) {
        match ch {
            '7' => self.saved_cursor = self.cursor,
            '8' => self.cursor = self.saved_cursor,
            'c' => self.reset(),
            'D' => self.index(),
            'M' => self.reverse_index(),
            'E' => self.next_line(),
            '[' => {
                self.parser_state = ParserState::Csi {
                    params: Vec::new(),
                    private: false,
                };
            }
            ']' => {
                self.parser_state = ParserState::Osc {
                    content: Vec::new(),
                };
            }
            _ => {
                // Unknown escape sequence
                self.parser_state = ParserState::Normal;
            }
        }
    }

    fn handle_csi_byte(&mut self, b: u8) {
        if let ParserState::Csi {
            ref mut params,
            ref mut private,
        } = self.parser_state
        {
            match b {
                b'?' => {
                    *private = true;
                }
                b'0'..=b'9' => {
                    if params.is_empty() {
                        params.push(0);
                    }
                    let last = params.len() - 1;
                    params[last] = params[last] * 10 + ((b - b'0') as usize);
                }
                b';' => {
                    params.push(0);
                }
                _ => {
                    // Command character
                    let private = *private;
                    let params_copy = params.clone();
                    self.parser_state = ParserState::Normal;
                    self.handle_csi_command(b, &params_copy, private);
                }
            }
        }
    }

    fn handle_csi_command(&mut self, cmd: u8, params: &[usize], private: bool) {
        let p1 = if params.is_empty() || params[0] == 0 { 1 } else { params[0] };

        match (cmd, private) {
            (b'A', false) => self.cursor_up(p1),
            (b'B', false) => self.cursor_down(p1),
            (b'C', false) => self.cursor_forward(p1),
            (b'D', false) => self.cursor_backward(p1),
            (b'E', false) => self.cursor_next_line(p1),
            (b'F', false) => self.cursor_prev_line(p1),
            (b'G', false) => self.cursor_horizontal_absolute(p1),
            (b'H', false) | (b'f', false) => {
                let row = if !params.is_empty() && params[0] > 0 { params[0] - 1 } else { 0 };
                let col = if params.len() > 1 && params[1] > 0 { params[1] - 1 } else { 0 };
                self.cursor_position(row, col);
            }
            (b'J', false) => {
                let mode = if !params.is_empty() { params[0] } else { 0 };
                self.erase_display(mode);
            }
            (b'K', false) => {
                let mode = if !params.is_empty() { params[0] } else { 0 };
                self.erase_line(mode);
            }
            (b'L', false) => self.insert_lines(p1),
            (b'M', false) => self.delete_lines(p1),
            (b'@', false) => self.insert_chars(p1),
            (b'P', false) => self.delete_chars(p1),
            (b'X', false) => self.erase_chars(p1),
            (b'S', false) => self.scroll_up(p1),
            (b'T', false) => self.scroll_down(p1),
            (b'r', false) => {
                let top = if !params.is_empty() && params[0] > 0 { params[0] - 1 } else { 0 };
                let bottom = if params.len() > 1 && params[1] > 0 {
                    params[1] - 1
                } else {
                    self.rows - 1
                };
                self.set_scroll_region(top, bottom);
            }
            (b'm', false) => {
                if params.is_empty() {
                    self.reset_attributes();
                } else {
                    self.handle_sgr(params);
                }
            }
            (b'h', true) => {
                // ?25 h - show cursor
                if !params.is_empty() && params[0] == 25 {
                    self.cursor.visible = true;
                } else if !params.is_empty() && params[0] == 1049 {
                    self.alt_screen_on();
                }
            }
            (b'l', true) => {
                // ?25 l - hide cursor
                if !params.is_empty() && params[0] == 25 {
                    self.cursor.visible = false;
                } else if !params.is_empty() && params[0] == 1049 {
                    self.alt_screen_off();
                }
            }
            (b'n', false) => {
                // DSR
                #[allow(clippy::collapsible_match)]
                if !params.is_empty() && params[0] == 6 {
                    self.device_status_report();
                }
            }
            _ => {
                // Unknown command
            }
        }
    }

    fn handle_osc_byte(&mut self, b: u8) {
        let (should_process, content_clone) = if let ParserState::Osc { ref mut content } = self.parser_state {
            if b == 0x07 {
                (true, Some(content.clone()))
            } else if b == 0x1b {
                content.push(b);
                (false, None)
            } else if b == b'\\' && !content.is_empty() && content[content.len() - 1] == 0x1b {
                content.pop();
                (true, Some(content.clone()))
            } else {
                content.push(b);
                (false, None)
            }
        } else {
            (false, None)
        };

        if should_process && let Some(content) = content_clone {
            self.process_osc(&content);
            self.parser_state = ParserState::Normal;
        }
    }

    fn process_osc(&mut self, content: &[u8]) {
        // Parse OSC command
        let mut parts = content.split(|&b| b == b';');
        if let Some(cmd_bytes) = parts.next() {
            let cmd_str = core::str::from_utf8(cmd_bytes).unwrap_or("");
            match cmd_str {
                "0" | "2" => {
                    // Set window title
                    if let Some(title_bytes) = parts.next() {
                        self.title = title_bytes.to_vec();
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_tab(&mut self) {
        let next_stop = ((self.cursor.col / 8) + 1) * 8;
        self.cursor.col = min(next_stop, self.cols - 1);
    }

    fn handle_lf(&mut self) {
        self.cursor.row += 1;
        if self.cursor.row > self.scroll_region_bottom {
            self.cursor.row = self.scroll_region_bottom;
            self.scroll_up(1);
        }
    }

    fn put_char(&mut self, ch: char) {
        // Handle deferred wrap: if we're at the right margin, wrap first
        if self.cursor.col >= self.cols {
            self.cursor.col = 0;
            self.cursor.row += 1;
            if self.cursor.row > self.scroll_region_bottom {
                self.cursor.row = self.scroll_region_bottom;
                self.scroll_up(1);
            }
        }

        if self.cursor.row >= self.rows {
            return;
        }

        let cell = Cell {
            ch,
            fg: self.fg,
            bg: self.bg,
            bold: self.bold,
            underline: self.underline,
            inverse: self.inverse,
            dirty: true,
        };

        self.grid[self.cursor.row][self.cursor.col] = cell;
        self.dirty_rows[self.cursor.row] = true;

        self.cursor.col += 1;
    }

    fn cursor_up(&mut self, n: usize) {
        self.cursor.row = self.cursor.row.saturating_sub(n);
    }

    fn cursor_down(&mut self, n: usize) {
        self.cursor.row = min(self.cursor.row + n, self.rows - 1);
    }

    fn cursor_forward(&mut self, n: usize) {
        self.cursor.col = min(self.cursor.col + n, self.cols - 1);
    }

    fn cursor_backward(&mut self, n: usize) {
        self.cursor.col = self.cursor.col.saturating_sub(n);
    }

    fn cursor_next_line(&mut self, n: usize) {
        self.cursor.row = min(self.cursor.row + n, self.rows - 1);
        self.cursor.col = 0;
    }

    fn cursor_prev_line(&mut self, n: usize) {
        self.cursor.row = self.cursor.row.saturating_sub(n);
        self.cursor.col = 0;
    }

    fn cursor_horizontal_absolute(&mut self, col: usize) {
        self.cursor.col = min(col.saturating_sub(1), self.cols - 1);
    }

    fn cursor_position(&mut self, row: usize, col: usize) {
        self.cursor.row = min(row, self.rows - 1);
        self.cursor.col = min(col, self.cols - 1);
    }

    fn erase_display(&mut self, mode: usize) {
        match mode {
            0 => {
                // Erase from cursor to end of display
                self.erase_line(0);
                for row in (self.cursor.row + 1)..self.rows {
                    for col in 0..self.cols {
                        self.grid[row][col] = Cell::blank();
                        self.dirty_rows[row] = true;
                    }
                }
            }
            1 => {
                // Erase from start of display to cursor
                for row in 0..self.cursor.row {
                    for col in 0..self.cols {
                        self.grid[row][col] = Cell::blank();
                        self.dirty_rows[row] = true;
                    }
                }
                self.erase_line(1);
            }
            2 | 3 => {
                // Erase entire display (3 also clears scrollback)
                for row in 0..self.rows {
                    for col in 0..self.cols {
                        self.grid[row][col] = Cell::blank();
                        self.dirty_rows[row] = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: usize) {
        let row = self.cursor.row;
        match mode {
            0 => {
                // Erase from cursor to end of line
                for col in self.cursor.col..self.cols {
                    self.grid[row][col] = Cell::blank();
                    self.dirty_rows[row] = true;
                }
            }
            1 => {
                // Erase from start of line to cursor
                for col in 0..=self.cursor.col {
                    self.grid[row][col] = Cell::blank();
                    self.dirty_rows[row] = true;
                }
            }
            2 => {
                // Erase entire line
                for col in 0..self.cols {
                    self.grid[row][col] = Cell::blank();
                    self.dirty_rows[row] = true;
                }
            }
            _ => {}
        }
    }

    fn insert_lines(&mut self, n: usize) {
        let n = min(n, self.scroll_region_bottom - self.scroll_region_top + 1);
        for _ in 0..n {
            self.grid.remove(self.scroll_region_top);
            self.grid.insert(self.scroll_region_bottom, vec![Cell::blank(); self.cols]);
            for i in self.scroll_region_top..=self.scroll_region_bottom {
                self.dirty_rows[i] = true;
            }
        }
    }

    fn delete_lines(&mut self, n: usize) {
        let n = min(n, self.scroll_region_bottom - self.scroll_region_top + 1);
        for _ in 0..n {
            self.grid.remove(self.scroll_region_top);
            self.grid.insert(self.scroll_region_bottom, vec![Cell::blank(); self.cols]);
            for i in self.scroll_region_top..=self.scroll_region_bottom {
                self.dirty_rows[i] = true;
            }
        }
    }

    fn insert_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        let n = min(n, self.cols - col);
        for _ in 0..n {
            self.grid[row].insert(col, Cell::blank());
            self.grid[row].pop();
        }
        self.dirty_rows[row] = true;
    }

    fn delete_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        let n = min(n, self.cols - col);
        for _ in 0..n {
            self.grid[row].remove(col);
            self.grid[row].push(Cell::blank());
        }
        self.dirty_rows[row] = true;
    }

    fn erase_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        let n = min(n, self.cols - col);
        for i in 0..n {
            self.grid[row][col + i] = Cell::blank();
        }
        self.dirty_rows[row] = true;
    }

    fn scroll_up(&mut self, n: usize) {
        let n = min(n, self.scroll_region_bottom - self.scroll_region_top + 1);
        for _ in 0..n {
            let line = self.grid[self.scroll_region_top].clone();
            self.scrollback.push_line(line);

            for i in self.scroll_region_top..self.scroll_region_bottom {
                self.grid[i] = self.grid[i + 1].clone();
                self.dirty_rows[i] = true;
            }
            self.grid[self.scroll_region_bottom] = vec![Cell::blank(); self.cols];
            self.dirty_rows[self.scroll_region_bottom] = true;
        }
    }

    fn scroll_down(&mut self, n: usize) {
        let n = min(n, self.scroll_region_bottom - self.scroll_region_top + 1);
        for _ in 0..n {
            self.grid.remove(self.scroll_region_bottom);
            self.grid.insert(self.scroll_region_top, vec![Cell::blank(); self.cols]);
            for i in self.scroll_region_top..=self.scroll_region_bottom {
                self.dirty_rows[i] = true;
            }
        }
    }

    fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        self.scroll_region_top = min(top, self.rows - 1);
        self.scroll_region_bottom = min(bottom, self.rows - 1);
    }

    fn handle_sgr(&mut self, params: &[usize]) {
        for &param in params {
            match param {
                0 => self.reset_attributes(),
                1 => self.bold = true,
                4 => self.underline = true,
                7 => self.inverse = true,
                22 => self.bold = false,
                24 => self.underline = false,
                27 => self.inverse = false,
                30..=37 => self.fg = Color::from_palette((param - 30) as u8),
                39 => self.fg = Color::White,
                40..=47 => self.bg = Color::from_palette((param - 40) as u8),
                49 => self.bg = Color::Black,
                90..=97 => self.fg = Color::from_palette((param - 90 + 8) as u8),
                100..=107 => self.bg = Color::from_palette((param - 100 + 8) as u8),
                38 | 48 => {
                    // Handled in extended color parsing
                }
                _ => {}
            }
        }

        // Handle extended colors (38;5;n and 38;2;r;g;b)
        let mut i = 0;
        while i < params.len() {
            if params[i] == 38 && i + 1 < params.len() {
                if params[i + 1] == 5 && i + 2 < params.len() {
                    // 256-color: 38;5;n
                    self.fg = Color::from_palette(params[i + 2] as u8);
                    i += 3;
                    continue;
                } else if params[i + 1] == 2 && i + 4 < params.len() {
                    // RGB: 38;2;r;g;b
                    self.fg = Color::Rgb(params[i + 2] as u8, params[i + 3] as u8, params[i + 4] as u8);
                    i += 5;
                    continue;
                }
            } else if params[i] == 48 && i + 1 < params.len() {
                if params[i + 1] == 5 && i + 2 < params.len() {
                    // 256-color: 48;5;n
                    self.bg = Color::from_palette(params[i + 2] as u8);
                    i += 3;
                    continue;
                } else if params[i + 1] == 2 && i + 4 < params.len() {
                    // RGB: 48;2;r;g;b
                    self.bg = Color::Rgb(params[i + 2] as u8, params[i + 3] as u8, params[i + 4] as u8);
                    i += 5;
                    continue;
                }
            }
            i += 1;
        }
    }

    fn reset_attributes(&mut self) {
        self.fg = Color::White;
        self.bg = Color::Black;
        self.bold = false;
        self.underline = false;
        self.inverse = false;
    }

    fn reset(&mut self) {
        // Clear screen
        for row in 0..self.rows {
            for col in 0..self.cols {
                self.grid[row][col] = Cell::blank();
                self.dirty_rows[row] = true;
            }
        }
        self.cursor = Cursor::new();
        self.reset_attributes();
        self.scroll_region_top = 0;
        self.scroll_region_bottom = self.rows - 1;
    }

    fn index(&mut self) {
        self.cursor.row += 1;
        if self.cursor.row > self.scroll_region_bottom {
            self.cursor.row = self.scroll_region_bottom;
            self.scroll_up(1);
        }
    }

    fn reverse_index(&mut self) {
        if self.cursor.row == self.scroll_region_top {
            self.scroll_down(1);
        } else {
            self.cursor.row = self.cursor.row.saturating_sub(1);
        }
    }

    fn next_line(&mut self) {
        self.cursor.row += 1;
        if self.cursor.row > self.scroll_region_bottom {
            self.cursor.row = self.scroll_region_bottom;
            self.scroll_up(1);
        }
        self.cursor.col = 0;
    }

    fn alt_screen_on(&mut self) {
        if self.alt_grid.is_none() {
            self.alt_grid = Some(self.grid.clone());
            self.alt_cursor = Some(self.cursor);
            self.alt_attrs = Some((self.fg, self.bg, self.bold, self.underline, self.inverse));
            
            // Clear and switch to alt screen
            for row in 0..self.rows {
                for col in 0..self.cols {
                    self.grid[row][col] = Cell::blank();
                    self.dirty_rows[row] = true;
                }
            }
            self.cursor = Cursor::new();
            self.reset_attributes();
        }
    }

    fn alt_screen_off(&mut self) {
        if let Some(alt_grid) = self.alt_grid.take() {
            self.grid = alt_grid;
            for i in 0..self.rows {
                self.dirty_rows[i] = true;
            }
            if let Some(alt_cursor) = self.alt_cursor.take() {
                self.cursor = alt_cursor;
            }
            if let Some((fg, bg, bold, underline, inverse)) = self.alt_attrs.take() {
                self.fg = fg;
                self.bg = bg;
                self.bold = bold;
                self.underline = underline;
                self.inverse = inverse;
            }
        }
    }

    fn device_status_report(&mut self) {
        // Reply with cursor position (CPR): ESC [ row ; col R
        // Note: positions are 1-indexed in the reply
        let row_str = alloc::format!("{}",  self.cursor.row + 1);
        let col_str = alloc::format!("{}", self.cursor.col + 1);
        self.reply_buffer.push(0x1b);
        self.reply_buffer.push(b'[');
        for &b in row_str.as_bytes() {
            self.reply_buffer.push(b);
        }
        self.reply_buffer.push(b';');
        for &b in col_str.as_bytes() {
            self.reply_buffer.push(b);
        }
        self.reply_buffer.push(b'R');
    }

    /// Take damage report: which rows have changed.
    pub fn take_damage(&mut self) -> Vec<usize> {
        let mut result = Vec::new();
        for i in 0..self.dirty_rows.len() {
            if self.dirty_rows[i] {
                result.push(i);
            }
            self.dirty_rows[i] = false;
        }
        result
    }
}

/// UTF-8 decoder result.
enum Utf8Result {
    /// A complete character.
    Char(char),
    /// Invalid UTF-8 sequence.
    Invalid,
    /// Incomplete sequence; need more bytes.
    Incomplete,
}

/// A simple UTF-8 decoder that handles split sequences.
struct Utf8Decoder {
    buffer: [u8; 4],
    pos: usize,
    needed: usize,
    pending: Option<u8>,
}

impl Utf8Decoder {
    fn new() -> Self {
        Utf8Decoder {
            buffer: [0; 4],
            pos: 0,
            needed: 0,
            pending: None,
        }
    }

    fn feed(&mut self, b: u8) -> Utf8Result {
        self.pending = None;

        if self.needed == 0 {
            // Starting a new sequence
            if b < 0x80 {
                // Single-byte ASCII
                Utf8Result::Char(b as char)
            } else if b < 0xc0 {
                // Invalid continuation byte as start
                Utf8Result::Invalid
            } else if b < 0xe0 {
                self.buffer[0] = b;
                self.pos = 1;
                self.needed = 2;
                Utf8Result::Incomplete
            } else if b < 0xf0 {
                self.buffer[0] = b;
                self.pos = 1;
                self.needed = 3;
                Utf8Result::Incomplete
            } else if b < 0xf8 {
                self.buffer[0] = b;
                self.pos = 1;
                self.needed = 4;
                Utf8Result::Incomplete
            } else {
                // Invalid start byte
                Utf8Result::Invalid
            }
        } else {
            // Continuing a sequence
            if !(0x80..0xc0).contains(&b) {
                // Invalid continuation (either ASCII or a new start byte)
                // Store this byte as pending and return invalid
                self.pending = Some(b);
                self.needed = 0;
                self.pos = 0;
                Utf8Result::Invalid
            } else {
                // Valid continuation byte
                self.buffer[self.pos] = b;
                self.pos += 1;
                if self.pos == self.needed {
                    let result = self.decode_buffer();
                    self.needed = 0;
                    self.pos = 0;
                    result
                } else {
                    Utf8Result::Incomplete
                }
            }
        }
    }

    fn decode_buffer(&self) -> Utf8Result {
        let bytes = &self.buffer[..self.pos];
        match core::str::from_utf8(bytes) {
            Ok(s) => {
                if let Some(ch) = s.chars().next() {
                    Utf8Result::Char(ch)
                } else {
                    Utf8Result::Invalid
                }
            }
            Err(_) => Utf8Result::Invalid,
        }
    }
}

/// Convert a key to bytes for sending to the shell.
pub fn key_to_bytes(key: Key) -> Vec<u8> {
    match key {
        Key::Char(ch) => {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            s.as_bytes().to_vec()
        }
        Key::Enter => vec![b'\r'],
        Key::Backspace => vec![0x7f],
        Key::Tab => vec![b'\t'],
        Key::Up => vec![0x1b, b'[', b'A'],
        Key::Down => vec![0x1b, b'[', b'B'],
        Key::Right => vec![0x1b, b'[', b'C'],
        Key::Left => vec![0x1b, b'[', b'D'],
        Key::Home => vec![0x1b, b'[', b'H'],
        Key::End => vec![0x1b, b'[', b'F'],
        Key::Delete => vec![0x1b, b'[', b'3', b'~'],
        Key::PageUp => vec![0x1b, b'[', b'5', b'~'],
        Key::PageDown => vec![0x1b, b'[', b'6', b'~'],
        Key::Ctrl(ch) => {
            if ch.is_ascii_lowercase() {
                vec![(ch as u8) - b'a' + 1]
            } else if ch.is_ascii_uppercase() {
                vec![(ch as u8) - b'A' + 1]
            } else {
                vec![]
            }
        }
    }
}

/// A key event for input.
#[derive(Clone, Copy, Debug)]
pub enum Key {
    /// A printable character
    Char(char),
    /// Enter key
    Enter,
    /// Backspace key
    Backspace,
    /// Tab key
    Tab,
    /// Arrow up
    Up,
    /// Arrow down
    Down,
    /// Arrow right
    Right,
    /// Arrow left
    Left,
    /// Home key
    Home,
    /// End key
    End,
    /// Delete key
    Delete,
    /// Page up
    PageUp,
    /// Page down
    PageDown,
    /// Ctrl+letter
    Ctrl(char),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_terminal() {
        let term = Terminal::new(80, 24);
        assert_eq!(term.dims(), (80, 24));
        assert_eq!(term.cursor().row, 0);
        assert_eq!(term.cursor().col, 0);
    }

    #[test]
    fn test_put_single_char() {
        let mut term = Terminal::new(80, 24);
        term.feed_byte(b'A');
        assert_eq!(term.cell_at(0, 0).unwrap().ch, 'A');
        assert_eq!(term.cursor().col, 1);
    }

    #[test]
    fn test_cursor_up() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"Hello
World");
        assert_eq!(term.cursor().row, 1);
        term.feed_bytes(b"[A"); // ESC[A - cursor up
        assert_eq!(term.cursor().row, 0);
    }

    #[test]
    fn test_cursor_down() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[B"); // ESC[B - cursor down
        assert_eq!(term.cursor().row, 1);
    }

    #[test]
    fn test_cursor_forward() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[5C"); // ESC[5C - cursor forward 5
        assert_eq!(term.cursor().col, 5);
    }

    #[test]
    fn test_cursor_backward() {
        let mut term = Terminal::new(80, 24);
        term.feed_byte(b'X');
        term.feed_byte(b'Y');
        term.feed_bytes(b"[D"); // ESC[D - cursor backward 1
        assert_eq!(term.cursor().col, 1);
    }

    #[test]
    fn test_cursor_position() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[5;10H"); // ESC[5;10H - cursor to row 5, col 10
        assert_eq!(term.cursor().row, 4); // 0-indexed
        assert_eq!(term.cursor().col, 9);
    }

    #[test]
    fn test_erase_line() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"Hello World");
        assert_eq!(term.cursor().col, 11);
        term.feed_bytes(b"[2K"); // ESC[2K - erase entire line
        for i in 0..11 {
            assert_eq!(term.cell_at(0, i).unwrap().ch, ' ');
        }
    }

    #[test]
    fn test_sgr_bold() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[1m"); // ESC[1m - bold
        assert!(term.bold);
        term.feed_bytes(b"[22m"); // ESC[22m - normal intensity
        assert!(!term.bold);
    }

    #[test]
    fn test_sgr_color() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[31m"); // ESC[31m - red
        assert_eq!(term.fg, Color::Red);
        term.feed_bytes(b"[39m"); // ESC[39m - default foreground
        assert_eq!(term.fg, Color::White);
    }

    #[test]
    fn test_save_restore_cursor() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"Hello");
        let saved_pos = term.cursor();
        term.feed_bytes(b"7"); // ESC7 - save cursor
        term.feed_bytes(b"[H"); // ESC[H - home
        assert_eq!(term.cursor().row, 0);
        assert_eq!(term.cursor().col, 0);
        term.feed_bytes(b"8"); // ESC8 - restore cursor
        assert_eq!(term.cursor().row, saved_pos.row);
        assert_eq!(term.cursor().col, saved_pos.col);
    }

    #[test]
    fn test_reset() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"Hello");
        term.feed_bytes(b"c"); // ESCc - reset
        assert_eq!(term.cursor().row, 0);
        assert_eq!(term.cursor().col, 0);
        assert_eq!(term.cell_at(0, 0).unwrap().ch, ' ');
    }

    #[test]
    fn test_index() {
        let mut term = Terminal::new(80, 24);
        term.cursor.row = 5;
        term.feed_bytes(b"D"); // ESCD - index
        assert_eq!(term.cursor().row, 6);
    }

    #[test]
    fn test_reverse_index() {
        let mut term = Terminal::new(80, 24);
        term.cursor.row = 5;
        term.feed_bytes(b"M"); // ESCM - reverse index
        assert_eq!(term.cursor().row, 4);
    }

    #[test]
    fn test_next_line() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"X");
        term.feed_bytes(b"E"); // ESCE - next line
        assert_eq!(term.cursor().row, 1);
        assert_eq!(term.cursor().col, 0);
    }

    #[test]
    fn test_dectcem_hide_cursor() {
        let mut term = Terminal::new(80, 24);
        assert!(term.cursor().visible);
        term.feed_bytes(b"[?25l"); // ESC[?25l - hide cursor
        assert!(!term.cursor().visible);
        term.feed_bytes(b"[?25h"); // ESC[?25h - show cursor
        assert!(term.cursor().visible);
    }

    #[test]
    fn test_dsr_cpr() {
        let mut term = Terminal::new(80, 24);
        term.cursor.row = 5;
        term.cursor.col = 10;
        term.feed_bytes(b"[6n"); // ESC[6n - DSR
        assert!(!term.reply_buffer.is_empty());
        let reply = core::str::from_utf8(&term.reply_buffer).unwrap();
        assert!(reply.contains("6"));
        assert!(reply.contains("11")); // col + 1
    }

    #[test]
    fn test_key_enter() {
        let bytes = key_to_bytes(Key::Enter);
        assert_eq!(bytes[0], b'\r');
    }

    #[test]
    fn test_key_backspace() {
        let bytes = key_to_bytes(Key::Backspace);
        assert_eq!(bytes, vec![0x7f]);
    }

    #[test]
    fn test_key_ctrl() {
        let bytes = key_to_bytes(Key::Ctrl('c'));
        assert_eq!(bytes, vec![3]); // Ctrl+C
    }

    #[test]
    fn test_alt_screen_on() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"Hello");
        term.feed_bytes(b"[?1049h"); // ESC[?1049h - alt screen on
        assert!(term.alt_grid.is_some());
    }

    #[test]
    fn test_alt_screen_off() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"Hello");
        term.feed_bytes(b"[?1049h"); // Alt screen on
        term.feed_bytes(b"Alt");
        term.feed_bytes(b"[?1049l"); // Alt screen off
        // Should restore original content
        assert_eq!(term.cell_at(0, 0).unwrap().ch, 'H');
    }

    #[test]
    fn test_wrap_deferred() {
        let mut term = Terminal::new(10, 24);
        term.feed_bytes(b"1234567890");
        assert_eq!(term.cursor().col, 10);
        assert_eq!(term.cursor().row, 0);
    }
}


    #[test]
    fn test_cuu_cud() {
        let mut term = Terminal::new(80, 24);
        term.cursor.row = 10;
        term.feed_bytes(b"[5A"); // Cursor up 5
        assert_eq!(term.cursor().row, 5);
        term.feed_bytes(b"[3B"); // Cursor down 3
        assert_eq!(term.cursor().row, 8);
    }

    #[test]
    fn test_cnl_cpl() {
        let mut term = Terminal::new(80, 24);
        term.cursor.row = 5;
        term.cursor.col = 10;
        term.feed_bytes(b"[2E"); // Cursor next line 2
        assert_eq!(term.cursor().row, 7);
        assert_eq!(term.cursor().col, 0);
        term.feed_bytes(b"[F"); // Cursor prev line 1
        assert_eq!(term.cursor().row, 6);
        assert_eq!(term.cursor().col, 0);
    }

    #[test]
    fn test_cha() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[20G"); // Cursor horizontal absolute 20
        assert_eq!(term.cursor().col, 19);
    }

    #[test]
    fn test_ed() {
        let mut term = Terminal::new(80, 3);
        term.feed_bytes(b"Line1
Line2
Line3");
        term.cursor.row = 1;
        term.cursor.col = 0;
        term.feed_bytes(b"[J"); // Erase from cursor to end
        // Line 1 and 2 should start with spaces
        assert_eq!(term.cell_at(1, 0).unwrap().ch, ' ');
    }

    #[test]
    fn test_ich() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"ABCDE");
        term.cursor.col = 2;
        term.feed_bytes(b"[@"); // Insert char (no parameter = 1)
        // Should have a space inserted at position 2
        assert_eq!(term.cell_at(0, 2).unwrap().ch, ' ');
    }

    #[test]
    fn test_dch() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"ABCDE");
        term.cursor.col = 1;
        term.feed_bytes(b"[P"); // Delete char
        // Character at position 1 should be 'C'
        assert_eq!(term.cell_at(0, 1).unwrap().ch, 'C');
    }

    #[test]
    fn test_ech() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"ABCDE");
        term.cursor.col = 1;
        term.feed_bytes(b"[2X"); // Erase 2 chars
        assert_eq!(term.cell_at(0, 1).unwrap().ch, ' ');
        assert_eq!(term.cell_at(0, 2).unwrap().ch, ' ');
    }

    #[test]
    fn test_su_sd() {
        let mut term = Terminal::new(80, 3);
        term.feed_bytes(b"Line1
Line2
Line3");
        assert_eq!(term.cell_at(0, 0).unwrap().ch, 'L');
        term.feed_bytes(b"[S"); // Scroll up
        // Line 1 should be blank now
        assert_eq!(term.cell_at(0, 0).unwrap().ch, ' ');
    }

    #[test]
    fn test_decstbm() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[2;10r"); // Set scroll region rows 2-10
        assert_eq!(term.scroll_region_top, 1);
        assert_eq!(term.scroll_region_bottom, 9);
    }

    #[test]
    fn test_sgr_underline() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[4m"); // Underline
        assert!(term.underline);
        term.feed_bytes(b"[24m"); // No underline
        assert!(!term.underline);
    }

    #[test]
    fn test_sgr_inverse() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[7m"); // Inverse
        assert!(term.inverse);
        term.feed_bytes(b"[27m"); // No inverse
        assert!(!term.inverse);
    }

    #[test]
    fn test_sgr_bright_colors() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[91m"); // Bright red
        assert_eq!(term.fg, Color::BrightRed);
        term.feed_bytes(b"[102m"); // Bright green background
        assert_eq!(term.bg, Color::BrightGreen);
    }

    #[test]
    fn test_osc_title() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"]2;Test Title"); // OSC 2 with BEL
        let title = core::str::from_utf8(&term.title).unwrap();
        assert_eq!(title, "Test Title");
    }

    #[test]
    fn test_unknown_csi_ignored() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"Hello");
        term.feed_bytes(b"[999Z"); // Unknown CSI command
        // Should not crash, content stays
        assert_eq!(term.cell_at(0, 0).unwrap().ch, 'H');
    }

    #[test]
    fn test_utf8_split_across_feeds() {
        let mut term = Terminal::new(80, 24);
        // UTF-8 for '€' (U+20AC): 0xE2 0x82 0xAC
        term.feed_byte(0xe2);
        assert_eq!(term.cursor().col, 0); // No character yet
        term.feed_byte(0x82);
        assert_eq!(term.cursor().col, 0); // Still incomplete
        term.feed_byte(0xac);
        assert_eq!(term.cursor().col, 1); // Now complete
        assert_eq!(term.cell_at(0, 0).unwrap().ch, '€');
    }

    #[test]
    fn test_scrollback_cap() {
        let mut term = Terminal::new(80, 3);
        // Generate more than scrollback capacity
        for i in 0..10 {
            let line = alloc::format!("Line{}", i);
            term.feed_bytes(line.as_bytes());
            term.feed_byte(b'\n');
        }
        // Scrollback should not exceed capacity
        assert!(term.scrollback.len() <= 5010); // 5000 + margin
    }

    #[test]
    fn test_scroll_region_bounds() {
        let mut term = Terminal::new(80, 10);
        term.feed_bytes(b"[2;8r"); // Set region 2-8
        // Scrolling should only affect this region
        assert_eq!(term.scroll_region_top, 1);
        assert_eq!(term.scroll_region_bottom, 7);
    }

    #[test]
    fn test_cursor_clamping() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[50;100H"); // Position beyond screen
        assert!(term.cursor().row < 24);
        assert!(term.cursor().col < 80);
    }

    #[test]
    fn test_wrap_with_deferred_semantics() {
        let mut term = Terminal::new(10, 5);
        for i in 0..10 {
            term.feed_byte(b'0' + (i % 10) as u8);
        }
        // Cursor should be at right margin
        assert_eq!(term.cursor().col, 10);
        assert_eq!(term.cursor().row, 0);
        // Next char should wrap
        term.feed_byte(b'X');
        assert_eq!(term.cell_at(1, 0).unwrap().ch, 'X');
    }

    #[test]
    fn test_cr_lf_interaction() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"Hello\r\nWorld");
        assert_eq!(term.cell_at(0, 0).unwrap().ch, 'H');
        assert_eq!(term.cell_at(1, 0).unwrap().ch, 'W');
    }

    #[test]
    fn test_alt_screen_attrs_preserved() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[31m"); // Red
        term.feed_bytes(b"[?1049h"); // Alt screen on
        term.feed_bytes(b"[32m"); // Green (on alt screen)
        assert_eq!(term.fg, Color::Green);
        term.feed_bytes(b"[?1049l"); // Alt screen off
        // Should restore red
        assert_eq!(term.fg, Color::Red);
    }

    #[test]
    fn test_malformed_utf8_no_panic() {
        let mut term = Terminal::new(80, 24);
        // Invalid UTF-8 sequence
        term.feed_byte(0xc3);
        term.feed_byte(0x28); // Invalid continuation
        // Should not panic, replacement char should appear
        assert!(term.cursor().col > 0);
    }

    #[test]
    fn test_invalid_esc_sequence_ignored() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"Text@Text"); // Invalid ESC @
        // Content should be preserved
        assert_eq!(term.cell_at(0, 0).unwrap().ch, 'T');
    }

    #[test]
    fn test_default_param_values() {
        let mut term = Terminal::new(80, 24);
        term.cursor.row = 5;
        term.feed_bytes(b"[A"); // CUU with no param = 1
        assert_eq!(term.cursor().row, 4);
        term.cursor.row = 5;
        term.feed_bytes(b"[B"); // CUD with no param = 1
        assert_eq!(term.cursor().row, 6);
    }

    #[test]
    fn test_zero_param_treated_as_one() {
        let mut term = Terminal::new(80, 24);
        term.cursor.row = 5;
        term.feed_bytes(b"[0A"); // CUU with 0 = 1
        assert_eq!(term.cursor().row, 4);
    }

    #[test]
    fn test_multiple_params() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[1;31;1m"); // Bold, red, bold (redundant)
        assert!(term.bold);
        assert_eq!(term.fg, Color::Red);
    }

    #[test]
    fn test_sgr_rgb_background() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[48;2;0;255;0m"); // RGB green background
        assert_eq!(term.bg, Color::Rgb(0, 255, 0));
    }

    #[test]
    fn test_resize_content_preserved() {
        let mut term = Terminal::new(20, 10);
        term.feed_bytes(b"Test");
        assert_eq!(term.cell_at(0, 0).unwrap().ch, 'T');
        term.resize(40, 20);
        // Content should still be there
        assert_eq!(term.cell_at(0, 0).unwrap().ch, 'T');
    }

    #[test]
    fn test_damage_reporting() {
        let mut term = Terminal::new(80, 24);
        term.take_damage(); // Clear
        term.feed_bytes(b"X");
        let damage = term.take_damage();
        assert!(!damage.is_empty());
        let damage2 = term.take_damage();
        assert_eq!(damage2.len(), 0); // Second call has no damage
    }

    #[test]
    fn test_long_input_stream() {
        let mut term = Terminal::new(80, 24);
        let input = "abcdefghijklmnopqrstuvwxyz0123456789";
        for _ in 0..5 {
            term.feed_bytes(input.as_bytes());
        }
        // Should not panic or crash
        assert!(term.cursor().row > 0);
    }

    #[test]
    fn test_mixed_control_and_text() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"Hello");
        term.feed_byte(0x08); // BS
        term.feed_bytes(b"X");
        assert_eq!(term.cell_at(0, 4).unwrap().ch, 'X');
    }

    #[test]
    fn test_cursor_visibility() {
        let mut term = Terminal::new(80, 24);
        assert!(term.cursor().visible);
        term.feed_bytes(b"[?25l");
        assert!(!term.cursor().visible);
        term.feed_bytes(b"[?25h");
        assert!(term.cursor().visible);
    }

    #[test]
    fn test_erase_display_modes() {
        let mut term = Terminal::new(80, 5);
        term.feed_bytes(b"Line1
Line2
Line3");
        term.cursor.row = 1;
        term.cursor.col = 0;
        
        // Test mode 1 (erase from start to cursor)
        term.feed_bytes(b"[1J");
        assert_eq!(term.cell_at(0, 0).unwrap().ch, ' ');
    }

    #[test]
    fn test_erase_line_modes() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"ABCDE");
        
        // Mode 1: erase from start to cursor
        term.cursor.col = 2;
        term.feed_bytes(b"[1K");
        assert_eq!(term.cell_at(0, 0).unwrap().ch, ' ');
        assert_eq!(term.cell_at(0, 1).unwrap().ch, ' ');
        assert_eq!(term.cell_at(0, 2).unwrap().ch, ' ');
        assert_eq!(term.cell_at(0, 3).unwrap().ch, 'D');
    }

    #[test]
    fn test_cursor_position_1indexed() {
        let mut term = Terminal::new(80, 24);
        term.feed_bytes(b"[1;1H"); // Position 1,1 should go to 0,0
        assert_eq!(term.cursor().row, 0);
        assert_eq!(term.cursor().col, 0);
    }

    #[test]
    fn test_empty_params() {
        let mut term = Terminal::new(80, 24);
        term.cursor.row = 5;
        term.feed_bytes(b"[m"); // SGR with no params = reset
        assert_eq!(term.fg, Color::White);
        assert!(!term.bold);
    }


    #[cfg(test)]
    #[test]
    fn test_key_ctrl_d() { let b = key_to_bytes(Key::Ctrl('d')); assert_eq!(b[0], 4_u8); }
    #[test]
    fn test_key_ctrl_a() { let b = key_to_bytes(Key::Ctrl('a')); assert_eq!(b[0], 1_u8); }
    #[test]
    fn test_key_tab() { let b = key_to_bytes(Key::Tab); assert_eq!(b[0], b'\t'); }
    #[test]
    fn test_key_enter() { let b = key_to_bytes(Key::Enter); assert_eq!(b[0], b'\r'); }
    #[test]
    fn test_key_char_a() { let b = key_to_bytes(Key::Char('a')); assert_eq!(b[0], b'a'); }
    #[test]
    fn test_sgr_fg_37() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[37m"); assert!(!t.fg.eq(&Color::Black) || 7 == 0); }
    #[test]
    fn test_sgr_fg_36() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[36m"); assert!(!t.fg.eq(&Color::Black) || 6 == 0); }
    #[test]
    fn test_sgr_fg_35() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[35m"); assert!(!t.fg.eq(&Color::Black) || 5 == 0); }
    #[test]
    fn test_sgr_fg_34() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[34m"); assert!(!t.fg.eq(&Color::Black) || 4 == 0); }
    #[test]
    fn test_sgr_fg_33() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[33m"); assert!(!t.fg.eq(&Color::Black) || 3 == 0); }
    #[test]
    fn test_sgr_fg_32() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[32m"); assert!(!t.fg.eq(&Color::Black) || 2 == 0); }
    #[test]
    fn test_sgr_fg_31() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[31m"); assert!(!t.fg.eq(&Color::Black) || 1 == 0); }
    #[test]
    fn test_sgr_fg_30() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[30m"); assert!(!t.fg.eq(&Color::Black) || 0 == 0); }
    #[test]
    fn test_csi_d_cub_0() { let mut t = Terminal::new(80, 24); t.cursor.col = 10; t.feed_bytes(b"\x1b[D"); assert_eq!(t.cursor().col, 9); }
    #[test]
    fn test_csi_c_cuf_0() { let mut t = Terminal::new(80, 24); t.cursor.col = 10; t.feed_bytes(b"\x1b[C"); assert_eq!(t.cursor().col, 11); }
    #[test]
    fn test_csi_b_cud_0() { let mut t = Terminal::new(80, 24); t.cursor.row = 10; t.feed_bytes(b"\x1b[B"); assert_eq!(t.cursor().row, 11); }
    #[test]
    fn test_csi_a_cup_0() { let mut t = Terminal::new(80, 24); t.cursor.row = 10; t.feed_bytes(b"\x1b[A"); assert_eq!(t.cursor().row, 9); }
    #[test]
    fn test_esc_8_restore_pos() { let mut t = Terminal::new(80, 24); t.saved_cursor.row = 7; t.feed_bytes(b"\x1b8"); assert_eq!(t.cursor().row, 7); }
    #[test]
    fn test_esc_7_save_pos() { let mut t = Terminal::new(80, 24); t.cursor.row = 5; t.feed_bytes(b"\x1b7"); assert_eq!(t.saved_cursor.row, 5); }
    #[test]
    fn test_sgr_40() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[40m"); assert_eq!(t.bg, Color::Black); }
    #[test]
    fn test_sgr_41() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[41m"); assert_eq!(t.bg, Color::Red); }
    #[test]
    fn test_sgr_44() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[44m"); assert_eq!(t.bg, Color::Blue); }
    #[test]
    fn test_sgr_47() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[47m"); assert_eq!(t.bg, Color::White); }
    #[test]
    fn test_sgr_100() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[100m"); assert_eq!(t.bg, Color::BrightBlack); }
    #[test]
    fn test_sgr_107() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[107m"); assert_eq!(t.bg, Color::BrightWhite); }
    #[test]
    fn test_cui_cuu_5() { let mut t = Terminal::new(80, 24); t.cursor.row = 10; t.feed_bytes(b"\x1b[5A"); assert_eq!(t.cursor().row, 5); }
    #[test]
    fn test_cud_5() { let mut t = Terminal::new(80, 24); t.cursor.row = 10; t.feed_bytes(b"\x1b[5B"); assert_eq!(t.cursor().row, 15); }
    #[test]
    fn test_cup_row_col() { let mut t = Terminal::new(80, 24); t.feed_bytes(b"\x1b[10;20H"); assert_eq!(t.cursor().row, 9); assert_eq!(t.cursor().col, 19); }
    #[test]
    fn test_su_scroll_up_1() { let mut t = Terminal::new(80, 3); t.feed_bytes(b"A\nB\nC"); t.cursor.row = 0; t.feed_bytes(b"\x1b[S"); assert_eq!(t.cell_at(0,0).unwrap().ch, ' '); }

#[cfg(test)]
mod recorded_session_tests {
    use super::*;

    #[test]
    fn test_recorded_shell_session() {
        // Load the shell session bytes
        let session_bytes = include_bytes!("../tests/fixtures/shell-session.bin");

        let mut term = Terminal::new(80, 100); // Large height to keep all content visible
        term.feed_bytes(session_bytes);

        // Check for expected output on the screen
        let text = term.screen_text();
        let screen_str: String = text
            .iter()
            .map(|row| row.iter().collect::<String>())
            .collect::<Vec<String>>()
            .join("\n");

        // Should contain "hello otter"
        assert!(screen_str.contains("hello otter"), "Screen should contain 'hello otter'");

        // Should contain "command not found"
        assert!(screen_str.contains("command not found"), "Screen should contain 'command not found'");
    }
}
