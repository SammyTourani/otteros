use alloc::string::String;
use alloc::vec::Vec;

use super::Token;

impl super::Tokenizer {
    pub(crate) fn tokenize_plaintext(&mut self) -> Vec<Token> {
        // PLAINTEXT mode: everything is character data until EOF
        // NULL characters are replaced with U+FFFD per WHATWG §13.2.5
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch == '\0' {
                self.tokens.push(Token::Character('\u{FFFD}'));
            } else {
                self.tokens.push(Token::Character(ch));
            }
            self.pos += 1;
        }
        self.tokens.push(Token::Eof);
        self.tokens.clone()
    }

    pub(crate) fn tokenize_rcdata(&mut self) -> Vec<Token> {
        // RCDATA mode: character data with entity decoding and end tag recognition
        let mut current_content = String::new();

        while self.pos < self.input.len() {
            let ch = self.input[self.pos];

            if ch == '&' {
                // Emit accumulated content first
                if !current_content.is_empty() {
                    for c in current_content.chars() {
                        self.tokens.push(Token::Character(c));
                    }
                    current_content.clear();
                }
                // Parse entity reference
                self.parse_entity_in_rcdata();
                continue;
            } else if ch == '<' && self.peek_char(1) == Some('/') {
                // Possible end tag
                let mut tag_pos = self.pos + 2;
                let mut tag_name = String::new();

                // Collect alphabetic characters for tag name
                while tag_pos < self.input.len() {
                    let tag_ch = self.input[tag_pos];
                    if tag_ch.is_ascii_alphabetic() {
                        tag_name.push(tag_ch.to_ascii_lowercase());
                        tag_pos += 1;
                    } else {
                        break;
                    }
                }

                // Check if followed by >, whitespace, or / (proper end tag start)
                if self.last_start_tag.as_ref() == Some(&tag_name) 
                    && tag_pos < self.input.len() && (self.input[tag_pos] == '>' || self.input[tag_pos].is_whitespace() || self.input[tag_pos] == '/') {
                    // Need to find the closing >
                    let mut close_pos = tag_pos;
                    // Skip whitespace and attributes if any
                    while close_pos < self.input.len() && self.input[close_pos] != '>' {
                        close_pos += 1;
                    }

                    // Only emit end tag if we found a closing >
                    if close_pos < self.input.len() && self.input[close_pos] == '>' {
                        // Valid end tag
                        if !current_content.is_empty() {
                            for c in current_content.chars() {
                                self.tokens.push(Token::Character(c));
                            }
                            current_content.clear();
                        }
                        self.tokens.push(Token::EndTag { name: tag_name });
                        self.pos = close_pos + 1;
                        // Exit RCDATA and continue with normal tokenization
                        self.state = super::TokenizerState::Data;
                        while self.pos < self.input.len() {
                            let ch = self.current_char();
                            self.process_state(ch);
                            self.pos += 1;
                        }
                        // Handle EOF
                        match self.state {
                            super::TokenizerState::Comment | super::TokenizerState::CommentStart | super::TokenizerState::CommentStartDash |
                            super::TokenizerState::CommentEndDash | super::TokenizerState::CommentEnd | super::TokenizerState::BogusComment => {
                                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
                            }
                            super::TokenizerState::TagName | super::TokenizerState::BeforeAttributeName |
                            super::TokenizerState::AttributeName | super::TokenizerState::AfterAttributeName |
                            super::TokenizerState::BeforeAttributeValue | super::TokenizerState::AttributeValueDoubleQuoted |
                            super::TokenizerState::AttributeValueSingleQuoted | super::TokenizerState::AttributeValueUnquoted |
                            super::TokenizerState::AfterAttributeValueQuoted | super::TokenizerState::SelfClosingStartTag => {
                                self.emit_current_tag();
                            }
                            super::TokenizerState::DocTypeName | super::TokenizerState::BeforeDocTypeName |
                            super::TokenizerState::AfterDocTypeName | super::TokenizerState::BogusDocType => {
                                self.emit_doctype();
                            }
                            super::TokenizerState::TagOpen => {
                                self.tokens.push(Token::Character('<'));
                            }
                            super::TokenizerState::EndTagOpen => {
                                self.tokens.push(Token::Character('<'));
                                self.tokens.push(Token::Character('/'));
                            }
                            super::TokenizerState::MarkupDeclarationOpen => {
                                self.tokens.push(Token::Comment(String::new()));
                            }
                            _ => {}
                        }
                        self.tokens.push(Token::Eof);
                        return self.tokens.clone();
                    }
                    // If incomplete end tag, skip it and continue from close_pos
                    self.pos = close_pos;
                    continue;
                }

                // Not a valid end tag start - output character
                current_content.push(ch);
            } else if ch == '\0' {
                // NULL character replaced with U+FFFD per WHATWG §13.2.5
                current_content.push('\u{FFFD}');
            } else {
                current_content.push(ch);
            }

            self.pos += 1;
        }

        if !current_content.is_empty() {
            for c in current_content.chars() {
                self.tokens.push(Token::Character(c));
            }
        }
        self.tokens.push(Token::Eof);
        self.tokens.clone()
    }

    pub(crate) fn tokenize_rawtext(&mut self) -> Vec<Token> {
        // RAWTEXT mode: like RCDATA but NO entity decoding
        let mut current_content = String::new();

        while self.pos < self.input.len() {
            let ch = self.input[self.pos];

            if ch == '<' && self.peek_char(1) == Some('/') {
                // Possible end tag
                let mut tag_pos = self.pos + 2;
                let mut tag_name = String::new();

                // Collect alphabetic characters for tag name
                while tag_pos < self.input.len() {
                    let tag_ch = self.input[tag_pos];
                    if tag_ch.is_ascii_alphabetic() {
                        tag_name.push(tag_ch.to_ascii_lowercase());
                        tag_pos += 1;
                    } else {
                        break;
                    }
                }

                // Check if followed by >, whitespace, or / (proper end tag start)
                if self.last_start_tag.as_ref() == Some(&tag_name) 
                    && tag_pos < self.input.len() && (self.input[tag_pos] == '>' || self.input[tag_pos].is_whitespace() || self.input[tag_pos] == '/') {
                    // Need to find the closing >
                    let mut close_pos = tag_pos;
                    // Skip whitespace and attributes if any
                    while close_pos < self.input.len() && self.input[close_pos] != '>' {
                        close_pos += 1;
                    }

                    // Only emit end tag if we found a closing >
                    if close_pos < self.input.len() && self.input[close_pos] == '>' {
                        // Valid end tag
                        if !current_content.is_empty() {
                            for c in current_content.chars() {
                                self.tokens.push(Token::Character(c));
                            }
                            current_content.clear();
                        }
                        self.tokens.push(Token::EndTag { name: tag_name });
                        self.pos = close_pos + 1;
                        // Exit RAWTEXT and continue with normal tokenization
                        self.state = super::TokenizerState::Data;
                        while self.pos < self.input.len() {
                            let ch = self.current_char();
                            self.process_state(ch);
                            self.pos += 1;
                        }
                        // Handle EOF
                        match self.state {
                            super::TokenizerState::Comment | super::TokenizerState::CommentStart | super::TokenizerState::CommentStartDash |
                            super::TokenizerState::CommentEndDash | super::TokenizerState::CommentEnd | super::TokenizerState::BogusComment => {
                                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
                            }
                            super::TokenizerState::TagName | super::TokenizerState::BeforeAttributeName |
                            super::TokenizerState::AttributeName | super::TokenizerState::AfterAttributeName |
                            super::TokenizerState::BeforeAttributeValue | super::TokenizerState::AttributeValueDoubleQuoted |
                            super::TokenizerState::AttributeValueSingleQuoted | super::TokenizerState::AttributeValueUnquoted |
                            super::TokenizerState::AfterAttributeValueQuoted | super::TokenizerState::SelfClosingStartTag => {
                                self.emit_current_tag();
                            }
                            super::TokenizerState::DocTypeName | super::TokenizerState::BeforeDocTypeName |
                            super::TokenizerState::AfterDocTypeName | super::TokenizerState::BogusDocType => {
                                self.emit_doctype();
                            }
                            super::TokenizerState::TagOpen => {
                                self.tokens.push(Token::Character('<'));
                            }
                            super::TokenizerState::EndTagOpen => {
                                self.tokens.push(Token::Character('<'));
                                self.tokens.push(Token::Character('/'));
                            }
                            super::TokenizerState::MarkupDeclarationOpen => {
                                self.tokens.push(Token::Comment(String::new()));
                            }
                            _ => {}
                        }
                        self.tokens.push(Token::Eof);
                        return self.tokens.clone();
                    }
                    // If incomplete end tag, skip it and continue from close_pos
                    self.pos = close_pos;
                    continue;
                }

                // Not a valid end tag start - output character
                current_content.push(ch);
            } else if ch == '\0' {
                // NULL character replaced with U+FFFD per WHATWG §13.2.5
                current_content.push('\u{FFFD}');
            } else {
                current_content.push(ch);
            }

            self.pos += 1;
        }

        if !current_content.is_empty() {
            for c in current_content.chars() {
                self.tokens.push(Token::Character(c));
            }
        }
        self.tokens.push(Token::Eof);
        self.tokens.clone()
    }

    pub(crate) fn tokenize_scriptdata(&mut self) -> Vec<Token> {
        // Script data state: output character data until we see the proper end tag
        // In script data, HTML comments are NOT recognized - they're just character data
        let mut current_content = String::new();

        while self.pos < self.input.len() {
            let ch = self.input[self.pos];

            if ch == '<' && self.peek_char(1) == Some('/') {
                // Possible end tag
                let mut tag_pos = self.pos + 2;
                let mut tag_name = String::new();

                // Collect alphabetic characters for tag name
                while tag_pos < self.input.len() {
                    let tag_ch = self.input[tag_pos];
                    if tag_ch.is_ascii_alphabetic() {
                        tag_name.push(tag_ch.to_ascii_lowercase());
                        tag_pos += 1;
                    } else {
                        break;
                    }
                }

                // Check if followed by >, whitespace, or / AND matches last_start_tag
                if self.last_start_tag.as_ref() == Some(&tag_name) 
                    && tag_pos < self.input.len() && (self.input[tag_pos] == '>' || self.input[tag_pos].is_whitespace() || self.input[tag_pos] == '/')
                    && self.last_start_tag.as_ref().is_some_and(|t| t == &tag_name) {
                    // Valid end tag for script - emit accumulated content first
                    if !current_content.is_empty() {
                        for c in current_content.chars() {
                            self.tokens.push(Token::Character(c));
                        }
                        current_content.clear();
                    }
                    self.tokens.push(Token::EndTag { name: tag_name });
                    self.pos = tag_pos;
                    // Skip to >
                    while self.pos < self.input.len() && self.input[self.pos] != '>' {
                        self.pos += 1;
                    }
                    if self.pos < self.input.len() && self.input[self.pos] == '>' {
                        self.pos += 1;
                    }
                    continue;
                }

                // Not a valid end tag - treat < as regular character
                current_content.push(ch);
            } else if ch == '\0' {
                // NULL character replaced with U+FFFD per WHATWG §13.2.5
                current_content.push('\u{FFFD}');
            } else {
                current_content.push(ch);
            }

            self.pos += 1;
        }

        if !current_content.is_empty() {
            for c in current_content.chars() {
                self.tokens.push(Token::Character(c));
            }
        }
        self.tokens.push(Token::Eof);
        self.tokens.clone()
    }

    pub(crate) fn tokenize_cdata(&mut self) -> Vec<Token> {
        // CDATA section: output as character data, NOT replacing NULL
        // Recognize ]]> as the closing sequence
        while self.pos < self.input.len() {
            // Check for CDATA closing sequence
            if self.pos + 2 < self.input.len()
                && self.input[self.pos] == ']'
                && self.input[self.pos + 1] == ']'
                && self.input[self.pos + 2] == '>' {
                // End of CDATA section - switch to normal data state
                self.pos += 3;
                self.state = super::TokenizerState::Data;
                // Continue processing in data state
                while self.pos < self.input.len() {
                    let ch = self.current_char();
                    self.process_state(ch);
                    self.pos += 1;
                }
                // Handle EOF
                match self.state {
                    super::TokenizerState::Comment | super::TokenizerState::CommentStart | super::TokenizerState::CommentStartDash |
                    super::TokenizerState::CommentEndDash | super::TokenizerState::CommentEnd | super::TokenizerState::BogusComment => {
                        self.tokens.push(Token::Comment(self.temp_buffer.clone()));
                    }
                    super::TokenizerState::TagName | super::TokenizerState::BeforeAttributeName |
                    super::TokenizerState::AttributeName | super::TokenizerState::AfterAttributeName |
                    super::TokenizerState::BeforeAttributeValue | super::TokenizerState::AttributeValueDoubleQuoted |
                    super::TokenizerState::AttributeValueSingleQuoted | super::TokenizerState::AttributeValueUnquoted |
                    super::TokenizerState::AfterAttributeValueQuoted | super::TokenizerState::SelfClosingStartTag => {
                        self.emit_current_tag();
                    }
                    super::TokenizerState::DocType | super::TokenizerState::BeforeDocTypeName |
                    super::TokenizerState::DocTypeName | super::TokenizerState::AfterDocTypeName |
                    super::TokenizerState::BogusDocType => {
                        self.emit_doctype();
                    }
                    super::TokenizerState::TagOpen => {
                        self.tokens.push(Token::Character('<'));
                    }
                    super::TokenizerState::EndTagOpen => {
                        self.tokens.push(Token::Character('<'));
                        self.tokens.push(Token::Character('/'));
                    }
                    super::TokenizerState::MarkupDeclarationOpen => {
                        self.tokens.push(Token::Comment(String::new()));
                    }
                    _ => {}
                }
                self.tokens.push(Token::Eof);
                return self.tokens.clone();
            }
            let ch = self.input[self.pos];
            self.tokens.push(Token::Character(ch));
            self.pos += 1;
        }
        self.tokens.push(Token::Eof);
        self.tokens.clone()
    }
}
