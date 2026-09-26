use alloc::string::String;

use super::data::{decode_numeric_entity, decode_named_entity};
use super::Token;

impl super::Tokenizer {
    /// Parse a character reference starting at &
    pub(crate) fn parse_entity(&mut self) {
        // Position should be at &
        if self.pos >= self.input.len() || self.input[self.pos] != '&' {
            self.current_attr_value.push('&');
            return;
        }

        let start_pos = self.pos;
        self.pos += 1;

        if self.pos >= self.input.len() {
            self.current_attr_value.push('&');
            return;
        }

        // Check for numeric reference
        if self.input[self.pos] == '#' {
            self.pos += 1;
            if self.pos >= self.input.len() {
                self.current_attr_value.push_str("&#");
                return;
            }

            let is_hex = self.input[self.pos] == 'x' || self.input[self.pos] == 'X';
            if is_hex {
                self.pos += 1;
            }

            let num_start = self.pos;
            let radix = if is_hex { 16 } else { 10 };

            // Consume digits
            while self.pos < self.input.len()
                && ((radix == 10 && self.input[self.pos].is_ascii_digit())
                    || (radix == 16 && self.input[self.pos].is_ascii_hexdigit()))
            {
                self.pos += 1;
            }

            if self.pos == num_start {
                // No digits found
                self.pos = start_pos;
                self.current_attr_value.push('&');
                return;
            }

            // Parse the number
            let num_str: String = self.input[num_start..self.pos]
                .iter()
                .collect();
            if let Ok(codepoint) = u64::from_str_radix(&num_str, radix) {
                if self.pos < self.input.len() && self.input[self.pos] == ';' {
                    self.pos += 1;
                }
                let ch = if codepoint > 0x10FFFF {
                    '\u{FFFD}'
                } else {
                    decode_numeric_entity(codepoint as u32).unwrap_or('\u{FFFD}')
                };
                self.current_attr_value.push(ch);
            } else {
                self.pos = start_pos;
                self.current_attr_value.push('&');
            }
        } else {
            // Named reference
            let name_start = self.pos;
            while self.pos < self.input.len()
                && (self.input[self.pos].is_ascii_alphanumeric() || self.input[self.pos] == ';')
            {
                self.pos += 1;
            }

            let name_str: String = self.input[name_start..self.pos]
                .iter()
                .collect();

            // Try to match named entity
            if let Some(decoded) = decode_named_entity(&name_str) {
                self.current_attr_value.push_str(&decoded);
            } else if name_str.ends_with(';') {
                // Try without semicolon
                let name_no_semi = &name_str[..name_str.len() - 1];
                if let Some(decoded) = decode_named_entity(name_no_semi) {
                    if self.pos < self.input.len()
                        && !(self.input[self.pos] == '=' || self.input[self.pos].is_ascii_alphanumeric())
                    {
                        self.current_attr_value.push_str(&decoded);
                    } else {
                        // Attribute-value exception: don't decode
                        self.current_attr_value.push_str(&name_str);
                    }
                } else {
                    self.current_attr_value.push_str(&name_str);
                }
            } else {
                // Try without semicolon (legacy names)
                if let Some(decoded) = decode_named_entity(&name_str) {
                    if self.pos >= self.input.len()
                        || !(self.input[self.pos] == '=' || self.input[self.pos].is_ascii_alphanumeric())
                    {
                        self.current_attr_value.push_str(&decoded);
                    } else {
                        // Attribute-value exception
                        self.current_attr_value.push('&');
                        self.pos = start_pos + 1;
                    }
                } else {
                    self.current_attr_value.push('&');
                    self.pos = start_pos + 1;
                }
            }
        }
    }

    pub(crate) fn parse_entity_in_rcdata(&mut self) {
        // Position should be at &
        if self.pos >= self.input.len() || self.input[self.pos] != '&' {
            self.tokens.push(Token::Character('&'));
            return;
        }

        let start_pos = self.pos;
        self.pos += 1;

        if self.pos >= self.input.len() {
            self.tokens.push(Token::Character('&'));
            return;
        }

        // Check for numeric reference
        if self.input[self.pos] == '#' {
            self.pos += 1;
            if self.pos >= self.input.len() {
                self.tokens.push(Token::Character('&'));
                self.tokens.push(Token::Character('#'));
                return;
            }

            let is_hex = self.input[self.pos] == 'x' || self.input[self.pos] == 'X';
            if is_hex {
                self.pos += 1;
            }

            let num_start = self.pos;
            let radix = if is_hex { 16 } else { 10 };

            // Consume digits
            while self.pos < self.input.len()
                && ((radix == 10 && self.input[self.pos].is_ascii_digit())
                    || (radix == 16 && self.input[self.pos].is_ascii_hexdigit()))
            {
                self.pos += 1;
            }

            if self.pos == num_start {
                // No digits found
                self.pos = start_pos;
                self.tokens.push(Token::Character('&'));
                return;
            }

            // Parse the number
            let num_str: String = self.input[num_start..self.pos]
                .iter()
                .collect();
            if let Ok(codepoint) = u64::from_str_radix(&num_str, radix) {
                let consumed_semicolon = self.pos < self.input.len() && self.input[self.pos] == ';';
                if consumed_semicolon {
                    self.pos += 1;
                }
                let ch = if codepoint > 0x10FFFF {
                    '\u{FFFD}'
                } else {
                    decode_numeric_entity(codepoint as u32).unwrap_or('\u{FFFD}')
                };
                self.tokens.push(Token::Character(ch));
            } else {
                self.pos = start_pos;
                self.tokens.push(Token::Character('&'));
            }
        } else {
            // Named reference
            let name_start = self.pos;
            while self.pos < self.input.len()
                && (self.input[self.pos].is_ascii_alphanumeric() || self.input[self.pos] == ';')
            {
                self.pos += 1;
            }

            let name_str: String = self.input[name_start..self.pos]
                .iter()
                .collect();

            // Try to match named entity
            if let Some(decoded) = decode_named_entity(&name_str) {
                for c in decoded.chars() {
                    self.tokens.push(Token::Character(c));
                }
            } else if name_str.ends_with(';') {
                // Try without semicolon
                let name_no_semi = &name_str[..name_str.len() - 1];
                if let Some(decoded) = decode_named_entity(name_no_semi) {
                    for c in decoded.chars() {
                        self.tokens.push(Token::Character(c));
                    }
                } else {
                    // No match - emit literal
                    self.tokens.push(Token::Character('&'));
                    for c in name_str.chars() {
                        self.tokens.push(Token::Character(c));
                    }
                }
            } else {
                // Try without semicolon (legacy names)
                if let Some(decoded) = decode_named_entity(&name_str) {
                    for c in decoded.chars() {
                        self.tokens.push(Token::Character(c));
                    }
                } else {
                    // No valid named entity - emit & and reconsume
                    self.tokens.push(Token::Character('&'));
                    self.pos = start_pos + 1;
                }
            }
        }
    }

    pub(crate) fn character_reference_state(&mut self, _ch: Option<char>) {
        // Note: pos was incremented after the '&' was processed in data_state,
        // so self.pos now points to the first char after '&'
        let start_pos = self.pos;

        if self.pos >= self.input.len() {
            self.tokens.push(Token::Character('&'));
            self.state = TokenizerState::Data;
            return;
        }

        // Check for numeric reference
        if self.input[self.pos] == '#' {
            let mut pos = self.pos + 1;
            let is_hex = pos < self.input.len() && (self.input[pos] == 'x' || self.input[pos] == 'X');
            if is_hex {
                pos += 1;
            }

            let num_start = pos;
            let radix = if is_hex { 16 } else { 10 };

            // Consume digits
            while pos < self.input.len()
                && ((radix == 10 && self.input[pos].is_ascii_digit())
                    || (radix == 16 && self.input[pos].is_ascii_hexdigit()))
            {
                pos += 1;
            }

            if pos > num_start && pos < self.input.len() && self.input[pos] == ';' {
                // Valid numeric reference with semicolon
                let num_str: String = self.input[num_start..pos].iter().collect();
                // Try to parse; if it overflows or exceeds valid codepoint range, emit replacement character
                let ch = match u64::from_str_radix(&num_str, radix) {
                    Ok(codepoint) if codepoint > 0x10FFFF => '\u{FFFD}',
                    Ok(codepoint) => decode_numeric_entity(codepoint as u32).unwrap_or('\u{FFFD}'),
                    Err(_) => '\u{FFFD}', // Overflow or invalid number - emit replacement character
                };
                self.tokens.push(Token::Character(ch));
                self.pos = pos;
                self.state = TokenizerState::Data;
                return;
            }

            if pos > num_start {
                // Valid numeric reference without semicolon
                let num_str: String = self.input[num_start..pos].iter().collect();
                // Try to parse; if it overflows or exceeds valid codepoint range, emit replacement character
                let ch = match u64::from_str_radix(&num_str, radix) {
                    Ok(codepoint) if codepoint > 0x10FFFF => '\u{FFFD}',
                    Ok(codepoint) => decode_numeric_entity(codepoint as u32).unwrap_or('\u{FFFD}'),
                    Err(_) => '\u{FFFD}', // Overflow or invalid number - emit replacement character
                };
                self.tokens.push(Token::Character(ch));
                self.pos = pos - 1;
                self.state = TokenizerState::Data;
                return;
            }

            // No valid numeric reference - emit & and reposition
            self.tokens.push(Token::Character('&'));
            self.state = TokenizerState::Data;
            self.pos = start_pos - 1; // Back up so main loop reprocesses the # or #x
            return;
        }

        // Named reference - try to match the longest entity
        let mut pos = self.pos;
        let name_start = pos;

        while pos < self.input.len()
            && (self.input[pos].is_ascii_alphanumeric() || self.input[pos] == ';')
        {
            pos += 1;
        }

        if pos == name_start {
            self.tokens.push(Token::Character('&'));
            self.state = TokenizerState::Data;
            self.pos = start_pos - 1; // Back up so main loop reprocesses from start_pos
            return;
        }

        let name_str: String = self.input[name_start..pos].iter().collect();

        if name_str.ends_with(';') {
            // Try to match progressively shorter names with semicolon
            let mut best_match = None;
            let mut best_len = 0;

            // Try from longest to shortest (excluding the semicolon)
            for len in (2..=name_str.len()).rev() {
                let candidate = &name_str[..len];
                if let Some(decoded) = decode_named_entity(candidate) {
                    best_match = Some(decoded);
                    best_len = len; // length of matched entity (will consume semicolon after)
                    break;
                }
            }

            if let Some(decoded) = best_match {
                for ch in decoded.chars() {
                    self.tokens.push(Token::Character(ch));
                }
                // Set pos to point at the last character of the matched entity so main loop can increment past it
                self.pos = name_start + best_len - 1;
                // If name_str had a semicolon, skip over it too
                if best_len < name_str.len() {
                    // We matched without the semicolon, so skip it
                    self.pos += 1;
                }
                self.state = TokenizerState::Data;
                return;
            }

            // No match - emit literal
            self.tokens.push(Token::Character('&'));
            for ch in name_str.chars() {
                self.tokens.push(Token::Character(ch));
            }
            self.pos = pos - 1;
            self.state = TokenizerState::Data;
        } else {
            // Try without semicolon (legacy names) - try progressively shorter names
            let mut best_match = None;
            let mut best_len = 0;

            // Try from longest to shortest
            for len in (1..=name_str.len()).rev() {
                let candidate = &name_str[..len];
                if let Some(decoded) = decode_named_entity(candidate) {
                    // In Data state: no attribute-value exception applies
                    best_match = Some(decoded);
                    best_len = len;
                    break;
                }
            }

            if let Some(decoded) = best_match {
                for ch in decoded.chars() {
                    self.tokens.push(Token::Character(ch));
                }
                self.pos = name_start + best_len - 1;
                self.state = TokenizerState::Data;
                return;
            }

            // No valid named entity - emit & and reconsume
            self.tokens.push(Token::Character('&'));
            self.state = TokenizerState::Data;
            self.pos = start_pos - 1; // Back up one so the main loop reprocesses from start_pos
        }
    }

    /// Parse character reference in attribute value context (for main tokenize loop)
    pub(crate) fn parse_entity_in_attr_value(&mut self) {
        // Position should be at &
        if self.pos >= self.input.len() || self.input[self.pos] != '&' {
            self.current_attr_value.push('&');
            return;
        }

        let start_pos = self.pos;
        let mut pos = self.pos + 1;

        if pos >= self.input.len() {
            self.current_attr_value.push('&');
            return;
        }

        // Check for numeric reference
        if self.input[pos] == '#' {
            pos += 1;
            if pos >= self.input.len() {
                self.current_attr_value.push_str("&#");
                return;
            }

            let is_hex = self.input[pos] == 'x' || self.input[pos] == 'X';
            if is_hex {
                pos += 1;
            }

            let num_start = pos;
            let radix = if is_hex { 16 } else { 10 };

            // Consume digits
            while pos < self.input.len()
                && ((radix == 10 && self.input[pos].is_ascii_digit())
                    || (radix == 16 && self.input[pos].is_ascii_hexdigit()))
            {
                pos += 1;
            }

            if pos == num_start {
                // No digits found
                self.current_attr_value.push('&');
                return;
            }

            // Parse the number
            let num_str: String = self.input[num_start..pos].iter().collect();
            if let Ok(codepoint) = u64::from_str_radix(&num_str, radix) {
                let has_semi = pos < self.input.len() && self.input[pos] == ';';
                if has_semi {
                    pos += 1;
                    self.pos = pos - 1; // Leave pos at ';' for main loop to increment
                } else {
                    self.pos = pos - 1; // Leave pos at last digit for main loop to increment
                }
                let ch = if codepoint > 0x10FFFF {
                    '\u{FFFD}'
                } else {
                    decode_numeric_entity(codepoint as u32).unwrap_or('\u{FFFD}')
                };
                self.current_attr_value.push(ch);
            } else {
                self.current_attr_value.push('&');
            }
        } else {
            // Named reference
            let name_start = pos;
            while pos < self.input.len()
                && (self.input[pos].is_ascii_alphanumeric() || self.input[pos] == ';')
            {
                pos += 1;
            }

            let name_str: String = self.input[name_start..pos].iter().collect();

            if name_str.is_empty() {
                self.current_attr_value.push('&');
                return;
            }

            // Try to match named entity with attribute-value exception
            if let Some(decoded) = decode_named_entity(&name_str) {
                // Check attribute-value exception even if full name matches
                if !name_str.ends_with(';') {
                    let next_char_pos = name_start + name_str.len();
                    if next_char_pos < self.input.len() {
                        let next_ch = self.input[next_char_pos];
                        if next_ch.is_ascii_alphanumeric() || next_ch == '=' {
                            // Attribute-value exception - don't match
                            self.current_attr_value.push('&');
                            self.pos = start_pos;
                            return;
                        }
                    }
                }
                self.current_attr_value.push_str(&decoded);
                self.pos = pos - 1; // Leave pos at last char of entity for main loop to increment
            } else if name_str.ends_with(';') {
                // Try without semicolon - look for progressively shorter matches
                // Still need to apply attribute-value exception
                let mut found = false;
                for len in (1..name_str.len()).rev() {
                    let candidate = &name_str[..len];
                    if let Some(decoded) = decode_named_entity(candidate) {
                        // Check attribute-value exception even with trailing semicolon
                        let next_char_pos = name_start + len;
                        if next_char_pos < self.input.len() {
                            let next_ch = self.input[next_char_pos];
                            if next_ch.is_ascii_alphanumeric() || next_ch == '=' {
                                // Attribute-value exception - don't match (even though we have semicolon)
                                continue;
                            }
                        }
                        self.current_attr_value.push_str(&decoded);
                        self.pos = name_start + len; // Position at the character after the match
                        found = true;
                        break;
                    }
                }
                if !found {
                    self.current_attr_value.push('&');
                    self.current_attr_value.push_str(&name_str);
                    self.pos = pos - 1;
                }
            } else {
                // No semicolon - apply attribute-value exception
                let mut found = false;
                for len in (1..=name_str.len()).rev() {
                    let candidate = &name_str[..len];
                    if let Some(decoded) = decode_named_entity(candidate) {
                        // Check if next char is alphanumeric or '='
                        let next_char_pos = name_start + len;
                        if next_char_pos < self.input.len() {
                            let next_ch = self.input[next_char_pos];
                            if next_ch.is_ascii_alphanumeric() || next_ch == '=' {
                                // Attribute-value exception - don't match
                                continue;
                            }
                        }
                        self.current_attr_value.push_str(&decoded);
                        self.pos = next_char_pos - 1;
                        found = true;
                        break;
                    }
                }
                if !found {
                    self.current_attr_value.push('&');
                    self.pos = start_pos; // Reconsume & for main loop
                }
            }
        }
    }
}

use super::TokenizerState;
