use alloc::string::{String, ToString};

use super::{Token, TokenizerState};

impl super::Tokenizer {
    pub(crate) fn doctype_state(&mut self, _ch: Option<char>) {
        self.state = TokenizerState::BeforeDocTypeName;
        self.doctype_force_quirks = true;
        self.pos -= 1; // Reconsume
    }

    pub(crate) fn before_doctype_name_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') | Some('\x0C') => {} // Skip ASCII whitespace (including form feed)
            Some('>') => {
                // DOCTYPE with no name - set to quirks mode
                self.doctype_name = None;
                self.doctype_force_quirks = true;
                self.emit_doctype();
                self.state = TokenizerState::Data;
            }
            Some('\0') => {
                // NULL character becomes replacement character
                self.doctype_name = Some("\u{FFFD}".to_string());
                self.state = TokenizerState::DocTypeName;
            }
            Some(c) if c.is_ascii_uppercase() => {
                self.doctype_name = Some(c.to_ascii_lowercase().to_string());
                self.state = TokenizerState::DocTypeName;
            }
            Some(c) => {
                self.doctype_name = Some(c.to_string());
                self.state = TokenizerState::DocTypeName;
            }
            None => {
                self.emit_doctype();
            }
        }
    }

    pub(crate) fn doctype_name_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') | Some('\x0C') => {
                self.state = TokenizerState::AfterDocTypeName;
            }
            Some('>') => {
                self.emit_doctype_proper();
                self.state = TokenizerState::Data;
            }
            Some('\0') => {
                // NULL character becomes replacement character
                if let Some(ref mut name) = self.doctype_name {
                    name.push('\u{FFFD}');
                }
            }
            Some(c) if c.is_ascii_uppercase() => {
                if let Some(ref mut name) = self.doctype_name {
                    name.push(c.to_ascii_lowercase());
                }
            }
            Some(c) => {
                if let Some(ref mut name) = self.doctype_name {
                    name.push(c);
                }
            }
            None => {
                self.emit_doctype();
            }
        }
    }

    pub(crate) fn after_doctype_name_state(&mut self, _ch: Option<char>) {
        // Skip leading whitespace (including form feed)
        while self.pos < self.input.len() && matches!(self.input[self.pos], '\t' | '\n' | '\r' | ' ' | '\x0C') {
            self.pos += 1;
        }

        if self.pos >= self.input.len() {
            self.emit_doctype();
            self.state = super::TokenizerState::Data;
            return;
        }

        let remaining = &self.input[self.pos..];

        // Check for PUBLIC keyword (case-insensitive)
        if remaining.len() >= 6 {
            let keyword: String = remaining[..6].iter().collect::<String>().to_uppercase();
            if keyword == "PUBLIC" && (remaining.len() == 6 || matches!(remaining[6], '\t' | '\n' | '\r' | ' ' | '"' | '\'')) {
                self.pos += 6; // Skip "PUBLIC"
                self.parse_doctype_public_id();
                return;
            }
        }

        // Check for SYSTEM keyword (case-insensitive)
        if remaining.len() >= 6 {
            let keyword: String = remaining[..6].iter().collect::<String>().to_uppercase();
            if keyword == "SYSTEM" && (remaining.len() == 6 || matches!(remaining[6], '\t' | '\n' | '\r' | ' ' | '"' | '\'')) {
                self.pos += 6; // Skip "SYSTEM"
                self.parse_doctype_system_id();
                return;
            }
        }

        // No PUBLIC or SYSTEM - check for >
        while self.pos < self.input.len() && matches!(self.input[self.pos], '\t' | '\n' | '\r' | ' ' | '\x0C') {
            self.pos += 1;
        }

        if self.pos < self.input.len() && self.input[self.pos] == '>' {
            self.emit_doctype_proper();
            self.state = TokenizerState::Data;
        } else {
            self.state = TokenizerState::BogusDocType;
        }
    }

    pub(crate) fn parse_doctype_public_id(&mut self) {
        // Skip whitespace before quote (including form feed)
        while self.pos < self.input.len() && matches!(self.input[self.pos], '\t' | '\n' | '\r' | ' ' | '\x0C') {
            self.pos += 1;
        }

        if self.pos >= self.input.len() {
            self.emit_doctype();
            self.state = TokenizerState::Data;
            return;
        }

        // Get the quote character
        let quote = self.input[self.pos];
        if quote != '"' && quote != '\'' {
            // EOF after PUBLIC with no quote
            self.doctype_public_id = Some(String::new());
            self.state = TokenizerState::BogusDocType;
            return;
        }

        self.pos += 1; // Skip opening quote

        // Parse until closing quote or '>'
        let mut public_id = String::new();
        while self.pos < self.input.len() && self.input[self.pos] != quote && self.input[self.pos] != '>' {
            let ch = self.input[self.pos];
            if ch == '\0' {
                public_id.push('\u{FFFD}');
            } else {
                public_id.push(ch);
            }
            self.pos += 1;
        }

        self.doctype_public_id = Some(public_id);

        // Check if we hit '>' before closing quote (parse error)
        if self.pos < self.input.len() && self.input[self.pos] == '>' {
            self.emit_doctype();
            self.state = TokenizerState::Data;
            return;
        }

        if self.pos < self.input.len() && self.input[self.pos] == quote {
            self.pos += 1; // Skip closing quote
        }

        // After public ID, look for SYSTEM keyword, system ID quoted string, or >
        while self.pos < self.input.len() && matches!(self.input[self.pos], '\t' | '\n' | '\r' | ' ' | '\x0C') {
            self.pos += 1;
        }

        if self.pos >= self.input.len() {
            self.emit_doctype();
            self.state = TokenizerState::Data;
            return;
        }

        // Check for SYSTEM keyword
        let remaining = &self.input[self.pos..];
        if remaining.len() >= 6 {
            let keyword: String = remaining[..6].iter().collect::<String>().to_uppercase();
            if keyword == "SYSTEM" && (remaining.len() == 6 || matches!(remaining[6], '\t' | '\n' | '\r' | ' ' | '"' | '\'')) {
                self.pos += 6; // Skip "SYSTEM"
                self.parse_doctype_system_id();
                return;
            }
        }

        // Check for a quoted system ID without SYSTEM keyword
        if self.input[self.pos] == '"' || self.input[self.pos] == '\'' {
            // Parse the system ID directly (we're already at the quote)
            let quote = self.input[self.pos];
            self.pos += 1; // Skip opening quote

            let mut system_id = String::new();
            while self.pos < self.input.len() && self.input[self.pos] != quote && self.input[self.pos] != '>' {
                system_id.push(self.input[self.pos]);
                self.pos += 1;
            }

            self.doctype_system_id = Some(system_id);

            // Check if we hit '>' before closing quote (parse error)
            if self.pos < self.input.len() && self.input[self.pos] == '>' {
                self.emit_doctype();
                self.state = TokenizerState::Data;
                return;
            }

            if self.pos < self.input.len() && self.input[self.pos] == quote {
                self.pos += 1; // Skip closing quote
            }

            // After system ID, look for >
            while self.pos < self.input.len() && matches!(self.input[self.pos], '\t' | '\n' | '\r' | ' ') {
                self.pos += 1;
            }

            if self.pos < self.input.len() && self.input[self.pos] == '>' {
                self.emit_doctype_proper();
                self.state = TokenizerState::Data;
            } else {
                self.state = TokenizerState::BogusDocType;
            }
            return;
        }

        // Check for >
        if self.input[self.pos] == '>' {
            self.emit_doctype_proper();
            self.state = TokenizerState::Data;
        } else {
            self.state = TokenizerState::BogusDocType;
        }
    }

    pub(crate) fn parse_doctype_system_id(&mut self) {
        // Skip whitespace before quote (including form feed)
        while self.pos < self.input.len() && matches!(self.input[self.pos], '\t' | '\n' | '\r' | ' ' | '\x0C') {
            self.pos += 1;
        }

        if self.pos >= self.input.len() {
            self.emit_doctype();
            self.state = TokenizerState::Data;
            return;
        }

        // Get the quote character
        let quote = self.input[self.pos];
        if quote != '"' && quote != '\'' {
            // EOF after SYSTEM with no quote
            self.doctype_system_id = Some(String::new());
            self.state = TokenizerState::BogusDocType;
            return;
        }

        self.pos += 1; // Skip opening quote

        // Parse until closing quote or '>'
        let mut system_id = String::new();
        while self.pos < self.input.len() && self.input[self.pos] != quote && self.input[self.pos] != '>' {
            let ch = self.input[self.pos];
            if ch == '\0' {
                system_id.push('\u{FFFD}');
            } else {
                system_id.push(ch);
            }
            self.pos += 1;
        }

        self.doctype_system_id = Some(system_id);

        // Check if we hit '>' before closing quote (parse error)
        if self.pos < self.input.len() && self.input[self.pos] == '>' {
            self.emit_doctype();
            self.state = TokenizerState::Data;
            return;
        }

        if self.pos < self.input.len() && self.input[self.pos] == quote {
            self.pos += 1; // Skip closing quote
        }

        // After system ID, look for >
        while self.pos < self.input.len() && matches!(self.input[self.pos], '\t' | '\n' | '\r' | ' ' | '\x0C') {
            self.pos += 1;
        }

        if self.pos < self.input.len() && self.input[self.pos] == '>' {
            self.emit_doctype_proper();
            self.state = TokenizerState::Data;
        } else {
            self.state = TokenizerState::BogusDocType;
        }
    }

    pub(crate) fn bogus_doctype_state(&mut self, ch: Option<char>) {
        match ch {
            Some('>') => {
                self.emit_doctype();
                self.state = TokenizerState::Data;
            }
            None => {
                self.emit_doctype();
            }
            Some(_) => {}
        }
    }

    pub(crate) fn emit_doctype_proper(&mut self) {
        // Emit DOCTYPE properly terminated (with ">"), so force_quirks = false
        self.doctype_force_quirks = false;
        self.emit_doctype();
    }

    pub(crate) fn emit_doctype(&mut self) {
        self.tokens.push(Token::Doctype {
            name: self.doctype_name.take(),
            public_id: self.doctype_public_id.take(),
            system_id: self.doctype_system_id.take(),
            force_quirks: self.doctype_force_quirks,
        });
        self.doctype_force_quirks = false;
    }
}
