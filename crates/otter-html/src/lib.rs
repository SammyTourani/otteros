#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::collections::BTreeMap;
use core::str;

mod entities;
pub use entities::NAMED_ENTITIES;

// Re-export URL types for convenience
pub use otter_http::{Url, UrlError};

/// HTML token types
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Doctype {
        name: Option<String>,
        public_id: Option<String>,
        system_id: Option<String>,
        force_quirks: bool,
    },
    StartTag {
        name: String,
        attrs: BTreeMap<String, String>,
        self_closing: bool,
    },
    EndTag {
        name: String,
    },
    Comment(String),
    Character(char),
    Eof,
}

/// Tokenizer state machine
#[derive(Debug, Clone, Copy, PartialEq)]
enum TokenizerState {
    Data,
    TagOpen,
    EndTagOpen,
    TagName,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValueDoubleQuoted,
    AttributeValueSingleQuoted,
    AttributeValueUnquoted,
    AfterAttributeValueQuoted,
    SelfClosingStartTag,
    MarkupDeclarationOpen,
    CommentStart,
    CommentStartDash,
    Comment,
    CommentEndDash,
    CommentEnd,
    DocType,
    BeforeDocTypeName,
    DocTypeName,
    AfterDocTypeName,
    BogusDocType,
    BogusComment,
    CharacterReference,
}

// Windows-1252 replacement table for 0x80-0x9F
const WINDOWS_1252_TABLE: &[u32] = &[
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x017D, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

/// Decode a numeric character reference (decimal or hex)
fn decode_numeric_entity(codepoint: u32) -> Option<char> {
    // Handle Windows-1252 replacement for 0x80-0x9F
    let final_codepoint = if (0x80..=0x9F).contains(&codepoint) {
        WINDOWS_1252_TABLE[(codepoint - 0x80) as usize]
    } else {
        codepoint
    };

    // Reject surrogates, NULL, and codepoints > 0x10FFFF
    if final_codepoint == 0
        || (0xD800..=0xDFFF).contains(&final_codepoint)
        || final_codepoint > 0x10FFFF
    {
        return Some('\u{FFFD}'); // Replacement character
    }

    char::from_u32(final_codepoint)
}

/// Resolve a named entity reference
fn decode_named_entity(name: &str) -> Option<String> {
    // Try exact match first (entities with semicolon)
    for &(entity_name, entity_char) in NAMED_ENTITIES {
        if entity_name == name {
            return Some(entity_char.to_string());
        }
    }
    None
}

/// HTML tokenizer
pub struct Tokenizer {
    input: Vec<char>,
    pos: usize,
    tokens: Vec<Token>,
    state: TokenizerState,

    // State machine variables
    current_token: Option<Token>,
    temp_buffer: String,
    current_attr_name: String,
    current_attr_value: String,
    doctype_name: Option<String>,
    doctype_public_id: Option<String>,
    doctype_system_id: Option<String>,
    doctype_force_quirks: bool,
}

impl Tokenizer {
    /// Create a new tokenizer for UTF-8 HTML input
    pub fn new(html: &str) -> Self {
        let input: Vec<char> = html.chars().collect();
        Tokenizer {
            input,
            pos: 0,
            tokens: Vec::new(),
            state: TokenizerState::Data,
            current_token: None,
            temp_buffer: String::new(),
            current_attr_name: String::new(),
            current_attr_value: String::new(),
            doctype_name: None,
            doctype_public_id: None,
            doctype_system_id: None,
            doctype_force_quirks: false,
        }
    }

    /// Parse a character reference starting at &
    fn parse_entity(&mut self) {
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
            if let Ok(codepoint) = u32::from_str_radix(&num_str, radix) {
                if self.pos < self.input.len() && self.input[self.pos] == ';' {
                    self.pos += 1;
                }
                if let Some(ch) = decode_numeric_entity(codepoint) {
                    self.current_attr_value.push(ch);
                } else {
                    self.current_attr_value.push('\u{FFFD}');
                }
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

    /// Tokenize the entire input
    pub fn tokenize(&mut self) -> Vec<Token> {
        while self.pos < self.input.len() {
            let ch = self.current_char();
            self.process_state(ch);
            self.pos += 1;
        }

        // Handle EOF
        match self.state {
            TokenizerState::Comment | TokenizerState::CommentStart | TokenizerState::CommentStartDash |
            TokenizerState::CommentEndDash | TokenizerState::CommentEnd => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
            TokenizerState::TagName | TokenizerState::BeforeAttributeName |
            TokenizerState::AttributeName | TokenizerState::AfterAttributeName |
            TokenizerState::BeforeAttributeValue | TokenizerState::AttributeValueDoubleQuoted |
            TokenizerState::AttributeValueSingleQuoted | TokenizerState::AttributeValueUnquoted |
            TokenizerState::AfterAttributeValueQuoted | TokenizerState::SelfClosingStartTag => {
                self.emit_current_tag();
            }
            TokenizerState::DocTypeName | TokenizerState::BeforeDocTypeName |
            TokenizerState::AfterDocTypeName | TokenizerState::BogusDocType => {
                self.emit_doctype();
            }
            _ => {}
        }

        // Add EOF token
        self.tokens.push(Token::Eof);
        self.tokens.clone()
    }

    fn current_char(&self) -> Option<char> {
        if self.pos < self.input.len() {
            Some(self.input[self.pos])
        } else {
            None
        }
    }

    fn peek_char(&self, offset: usize) -> Option<char> {
        if self.pos + offset < self.input.len() {
            Some(self.input[self.pos + offset])
        } else {
            None
        }
    }

    fn process_state(&mut self, ch: Option<char>) {
        match self.state {
            TokenizerState::Data => self.data_state(ch),
            TokenizerState::TagOpen => self.tag_open_state(ch),
            TokenizerState::EndTagOpen => self.end_tag_open_state(ch),
            TokenizerState::TagName => self.tag_name_state(ch),
            TokenizerState::BeforeAttributeName => self.before_attribute_name_state(ch),
            TokenizerState::AttributeName => self.attribute_name_state(ch),
            TokenizerState::AfterAttributeName => self.after_attribute_name_state(ch),
            TokenizerState::BeforeAttributeValue => self.before_attribute_value_state(ch),
            TokenizerState::AttributeValueDoubleQuoted => self.attr_value_double_quoted_state(ch),
            TokenizerState::AttributeValueSingleQuoted => self.attr_value_single_quoted_state(ch),
            TokenizerState::AttributeValueUnquoted => self.attr_value_unquoted_state(ch),
            TokenizerState::AfterAttributeValueQuoted => self.after_attr_value_quoted_state(ch),
            TokenizerState::SelfClosingStartTag => self.self_closing_start_tag_state(ch),
            TokenizerState::MarkupDeclarationOpen => self.markup_declaration_open_state(ch),
            TokenizerState::CommentStart => self.comment_start_state(ch),
            TokenizerState::CommentStartDash => self.comment_start_dash_state(ch),
            TokenizerState::Comment => self.comment_state(ch),
            TokenizerState::CommentEndDash => self.comment_end_dash_state(ch),
            TokenizerState::CommentEnd => self.comment_end_state(ch),
            TokenizerState::DocType => self.doctype_state(ch),
            TokenizerState::BeforeDocTypeName => self.before_doctype_name_state(ch),
            TokenizerState::DocTypeName => self.doctype_name_state(ch),
            TokenizerState::AfterDocTypeName => self.after_doctype_name_state(ch),
            TokenizerState::BogusDocType => self.bogus_doctype_state(ch),
            TokenizerState::BogusComment => self.bogus_comment_state(ch),
            TokenizerState::CharacterReference => self.character_reference_state(ch),
        }
    }

    fn data_state(&mut self, ch: Option<char>) {
        match ch {
            Some('&') => {
                self.temp_buffer.clear();
                self.state = TokenizerState::CharacterReference;
            }
            Some('<') => {
                self.state = TokenizerState::TagOpen;
            }
            None => {} // EOF
            Some(c) => {
                self.tokens.push(Token::Character(c));
            }
        }
    }

    fn tag_open_state(&mut self, ch: Option<char>) {
        match ch {
            Some('/') => {
                self.state = TokenizerState::EndTagOpen;
            }
            Some(c) if c.is_ascii_alphabetic() => {
                self.pos -= 1; // Reconsume
                self.current_token = Some(Token::StartTag {
                    name: String::new(),
                    attrs: BTreeMap::new(),
                    self_closing: false,
                });
                self.state = TokenizerState::TagName;
            }
            Some('!') => {
                self.state = TokenizerState::MarkupDeclarationOpen;
            }
            Some('?') => {
                self.state = TokenizerState::BogusComment;
                self.temp_buffer.clear();
            }
            Some(_) => {
                self.tokens.push(Token::Character('<'));
                self.pos -= 1; // Reconsume
                self.state = TokenizerState::Data;
            }
            None => {
                self.tokens.push(Token::Character('<'));
                self.state = TokenizerState::Data;
            }
        }
    }

    fn end_tag_open_state(&mut self, ch: Option<char>) {
        match ch {
            Some(c) if c.is_ascii_alphabetic() => {
                self.pos -= 1; // Reconsume
                self.current_token = Some(Token::EndTag { name: String::new() });
                self.state = TokenizerState::TagName;
            }
            Some('>') => {
                self.state = TokenizerState::Data;
            }
            None => {
                self.tokens.push(Token::Character('<'));
                self.tokens.push(Token::Character('/'));
            }
            Some(_) => {
                self.temp_buffer.clear();
                self.state = TokenizerState::BogusComment;
            }
        }
    }

    fn tag_name_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {
                self.state = TokenizerState::BeforeAttributeName;
            }
            Some('/') => {
                self.state = TokenizerState::SelfClosingStartTag;
            }
            Some('>') => {
                self.emit_current_tag();
                self.state = TokenizerState::Data;
            }
            Some(c) if c.is_ascii_uppercase() => {
                if let Some(Token::StartTag { name, .. } | Token::EndTag { name, .. }) = &mut self.current_token {
                    name.push(c.to_ascii_lowercase());
                }
            }
            Some(c) => {
                if let Some(Token::StartTag { name, .. } | Token::EndTag { name, .. }) = &mut self.current_token {
                    name.push(c);
                }
            }
            None => {
                self.emit_current_tag();
            }
        }
    }

    fn before_attribute_name_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {} // Skip whitespace
            Some('/') => {
                self.state = TokenizerState::SelfClosingStartTag;
            }
            Some('>') => {
                self.emit_current_tag();
                self.state = TokenizerState::Data;
            }
            Some(_) => {
                self.pos -= 1; // Reconsume
                self.current_attr_name.clear();
                self.current_attr_value.clear();
                self.state = TokenizerState::AttributeName;
            }
            None => {
                self.emit_current_tag();
            }
        }
    }

    fn attribute_name_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {
                self.state = TokenizerState::AfterAttributeName;
            }
            Some('/') => {
                self.state = TokenizerState::SelfClosingStartTag;
            }
            Some('=') => {
                self.state = TokenizerState::BeforeAttributeValue;
            }
            Some('>') => {
                self.finalize_attribute();
                self.emit_current_tag();
                self.state = TokenizerState::Data;
            }
            Some(c) if c.is_ascii_uppercase() => {
                self.current_attr_name.push(c.to_ascii_lowercase());
            }
            Some(c) => {
                self.current_attr_name.push(c);
            }
            None => {
                self.finalize_attribute();
                self.emit_current_tag();
            }
        }
    }

    fn after_attribute_name_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {} // Skip whitespace
            Some('/') => {
                self.state = TokenizerState::SelfClosingStartTag;
            }
            Some('=') => {
                self.state = TokenizerState::BeforeAttributeValue;
            }
            Some('>') => {
                self.finalize_attribute();
                self.emit_current_tag();
                self.state = TokenizerState::Data;
            }
            Some(_) => {
                self.pos -= 1; // Reconsume
                self.finalize_attribute();
                self.current_attr_name.clear();
                self.current_attr_value.clear();
                self.state = TokenizerState::AttributeName;
            }
            None => {
                self.finalize_attribute();
                self.emit_current_tag();
            }
        }
    }

    fn before_attribute_value_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {} // Skip whitespace
            Some('"') => {
                self.state = TokenizerState::AttributeValueDoubleQuoted;
            }
            Some('\'') => {
                self.state = TokenizerState::AttributeValueSingleQuoted;
            }
            Some('>') => {
                self.finalize_attribute();
                self.emit_current_tag();
                self.state = TokenizerState::Data;
            }
            Some(_) => {
                self.pos -= 1; // Reconsume
                self.state = TokenizerState::AttributeValueUnquoted;
            }
            None => {
                self.finalize_attribute();
                self.emit_current_tag();
            }
        }
    }

    fn attr_value_double_quoted_state(&mut self, ch: Option<char>) {
        match ch {
            Some('"') => {
                self.state = TokenizerState::AfterAttributeValueQuoted;
            }
            Some('&') => {
                self.current_attr_value.push('&');
            }
            Some(c) => {
                self.current_attr_value.push(c);
            }
            None => {
                self.finalize_attribute();
                self.emit_current_tag();
            }
        }
    }

    fn attr_value_single_quoted_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\'') => {
                self.state = TokenizerState::AfterAttributeValueQuoted;
            }
            Some('&') => {
                self.current_attr_value.push('&');
            }
            Some(c) => {
                self.current_attr_value.push(c);
            }
            None => {
                self.finalize_attribute();
                self.emit_current_tag();
            }
        }
    }

    fn attr_value_unquoted_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {
                self.state = TokenizerState::BeforeAttributeName;
            }
            Some('&') => {
                self.current_attr_value.push('&');
            }
            Some('>') => {
                self.finalize_attribute();
                self.emit_current_tag();
                self.state = TokenizerState::Data;
            }
            Some(c) => {
                self.current_attr_value.push(c);
            }
            None => {
                self.finalize_attribute();
                self.emit_current_tag();
            }
        }
    }

    fn after_attr_value_quoted_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {
                self.finalize_attribute();
                self.state = TokenizerState::BeforeAttributeName;
            }
            Some('/') => {
                self.finalize_attribute();
                self.state = TokenizerState::SelfClosingStartTag;
            }
            Some('>') => {
                self.finalize_attribute();
                self.emit_current_tag();
                self.state = TokenizerState::Data;
            }
            Some(_) => {
                self.pos -= 1; // Reconsume
                self.finalize_attribute();
                self.state = TokenizerState::BeforeAttributeName;
            }
            None => {
                self.finalize_attribute();
                self.emit_current_tag();
            }
        }
    }

    fn self_closing_start_tag_state(&mut self, ch: Option<char>) {
        match ch {
            Some('>') => {
                if let Some(Token::StartTag { self_closing, .. }) = &mut self.current_token {
                    *self_closing = true;
                }
                self.emit_current_tag();
                self.state = TokenizerState::Data;
            }
            Some(_) => {
                self.pos -= 1; // Reconsume
                self.state = TokenizerState::BeforeAttributeName;
            }
            None => {
                self.emit_current_tag();
            }
        }
    }

    fn finalize_attribute(&mut self) {
        if !self.current_attr_name.is_empty()
            && let Some(Token::StartTag { attrs, .. }) = &mut self.current_token {
            attrs.insert(self.current_attr_name.clone(), self.current_attr_value.clone());
        }
        self.current_attr_name.clear();
        self.current_attr_value.clear();
    }

    fn emit_current_tag(&mut self) {
        if let Some(token) = self.current_token.take() {
            self.tokens.push(token);
        }
    }

    fn markup_declaration_open_state(&mut self, ch: Option<char>) {
        // Check for DOCTYPE
        if self.match_string("DOCTYPE") {
            self.state = TokenizerState::DocType;
            return;
        }

        // Check for comment
        if ch == Some('-') && self.peek_char(1) == Some('-') {
            self.pos += 1; // Skip next -
            self.state = TokenizerState::CommentStart;
            self.temp_buffer.clear();
            return;
        }

        // Bogus comment
        self.temp_buffer.clear();
        self.state = TokenizerState::BogusComment;
    }

    fn match_string(&mut self, s: &str) -> bool {
        let chars: Vec<char> = s.chars().collect();
        if self.pos + chars.len() > self.input.len() {
            return false;
        }

        for (i, &c) in chars.iter().enumerate() {
            if self.input[self.pos + i] != c {
                return false;
            }
        }

        self.pos += chars.len() - 1;
        true
    }

    fn comment_start_state(&mut self, ch: Option<char>) {
        match ch {
            Some('-') => {
                self.state = TokenizerState::CommentStartDash;
            }
            Some('>') => {
                self.state = TokenizerState::Data;
            }
            Some(_) => {
                self.pos -= 1; // Reconsume
                self.state = TokenizerState::Comment;
            }
            None => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
        }
    }

    fn comment_start_dash_state(&mut self, ch: Option<char>) {
        match ch {
            Some('-') => {
                self.state = TokenizerState::CommentEnd;
            }
            Some('>') => {
                self.state = TokenizerState::Data;
            }
            Some(_) => {
                self.pos -= 1; // Reconsume
                self.state = TokenizerState::Comment;
            }
            None => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
        }
    }

    fn comment_state(&mut self, ch: Option<char>) {
        match ch {
            Some('-') => {
                self.state = TokenizerState::CommentEndDash;
            }
            Some(c) => {
                self.temp_buffer.push(c);
            }
            None => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
        }
    }

    fn comment_end_dash_state(&mut self, ch: Option<char>) {
        match ch {
            Some('-') => {
                self.state = TokenizerState::CommentEnd;
            }
            Some(_) => {
                self.temp_buffer.push('-');
                self.pos -= 1; // Reconsume
                self.state = TokenizerState::Comment;
            }
            None => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
        }
    }

    fn comment_end_state(&mut self, ch: Option<char>) {
        match ch {
            Some('>') => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
                self.state = TokenizerState::Data;
            }
            Some('-') => {
                self.temp_buffer.push('-');
            }
            Some(_) => {
                self.temp_buffer.push('-');
                self.temp_buffer.push('-');
                self.pos -= 1; // Reconsume
                self.state = TokenizerState::Comment;
            }
            None => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
        }
    }

    fn doctype_state(&mut self, _ch: Option<char>) {
        self.state = TokenizerState::BeforeDocTypeName;
        self.doctype_force_quirks = true;
        self.pos -= 1; // Reconsume
    }

    fn before_doctype_name_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {} // Skip whitespace
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

    fn doctype_name_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {
                self.state = TokenizerState::AfterDocTypeName;
            }
            Some('>') => {
                self.emit_doctype();
                self.state = TokenizerState::Data;
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

    fn after_doctype_name_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {} // Skip whitespace
            Some('>') => {
                self.emit_doctype();
                self.state = TokenizerState::Data;
            }
            Some(_) => {
                self.state = TokenizerState::BogusDocType;
            }
            None => {
                self.emit_doctype();
            }
        }
    }

    fn bogus_doctype_state(&mut self, ch: Option<char>) {
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

    fn bogus_comment_state(&mut self, ch: Option<char>) {
        match ch {
            Some('>') => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
                self.state = TokenizerState::Data;
            }
            None => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
            Some(c) => {
                self.temp_buffer.push(c);
            }
        }
    }

    fn character_reference_state(&mut self, _ch: Option<char>) {
        // We're at & in data state, try to parse a character reference
        let start_pos = self.pos;

        if self.pos + 1 >= self.input.len() {
            self.tokens.push(Token::Character('&'));
            self.state = TokenizerState::Data;
            return;
        }

        // Check for numeric reference
        if self.input[self.pos + 1] == '#' {
            let mut pos = self.pos + 2;
            let is_hex = pos < self.input.len() && (self.input[pos] == 'x' || self.input[pos] == 'X');
            if is_hex && pos + 1 < self.input.len() {
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
                if let Ok(codepoint) = u32::from_str_radix(&num_str, radix)
                    && let Some(ch) = decode_numeric_entity(codepoint) {
                        self.tokens.push(Token::Character(ch));
                        self.pos = pos;
                        self.state = TokenizerState::Data;
                        return;
                    }
            }

            if pos > num_start {
                // Valid numeric reference without semicolon
                let num_str: String = self.input[num_start..pos].iter().collect();
                if let Ok(codepoint) = u32::from_str_radix(&num_str, radix)
                    && let Some(ch) = decode_numeric_entity(codepoint) {
                        self.tokens.push(Token::Character(ch));
                        self.pos = pos - 1;
                        self.state = TokenizerState::Data;
                        return;
                    }
            }

            // No valid numeric reference
            self.tokens.push(Token::Character('&'));
            self.state = TokenizerState::Data;
            return;
        }

        // Named reference
        let mut pos = self.pos + 1;
        let name_start = pos;

        while pos < self.input.len()
            && (self.input[pos].is_ascii_alphanumeric() || self.input[pos] == ';')
        {
            pos += 1;
        }

        if pos == name_start {
            self.tokens.push(Token::Character('&'));
            self.state = TokenizerState::Data;
            return;
        }

        let name_str: String = self.input[name_start..pos].iter().collect();

        if name_str.ends_with(';') {
            // Try exact match with semicolon
            if let Some(decoded) = decode_named_entity(&name_str) {
                for ch in decoded.chars() {
                    self.tokens.push(Token::Character(ch));
                }
                self.pos = pos - 1;
                self.state = TokenizerState::Data;
                return;
            }

            // Try without semicolon
            let name_no_semi = &name_str[..name_str.len() - 1];
            if let Some(decoded) = decode_named_entity(name_no_semi) {
                for ch in decoded.chars() {
                    self.tokens.push(Token::Character(ch));
                }
                self.pos = pos - 1;
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
            // Try without semicolon (legacy names)
            if let Some(decoded) = decode_named_entity(&name_str) {
                for ch in decoded.chars() {
                    self.tokens.push(Token::Character(ch));
                }
                self.pos = pos - 1;
                self.state = TokenizerState::Data;
                return;
            }

            // No valid named entity
            self.tokens.push(Token::Character('&'));
            self.state = TokenizerState::Data;
            self.pos = start_pos;
        }
    }

    fn emit_doctype(&mut self) {
        self.tokens.push(Token::Doctype {
            name: self.doctype_name.take(),
            public_id: self.doctype_public_id.take(),
            system_id: self.doctype_system_id.take(),
            force_quirks: self.doctype_force_quirks,
        });
        self.doctype_force_quirks = false;
    }
}

/// HTML node
#[derive(Debug, Clone)]
pub enum Node {
    Document { children: Vec<usize> },
    Element { tag: String, attrs: BTreeMap<String, String>, children: Vec<usize>, parent: Option<usize> },
    Text { content: String, parent: Option<usize> },
    Comment { content: String, parent: Option<usize> },
}

/// HTML document tree
pub struct Document {
    nodes: Vec<Node>,
    root: usize,
}

impl Document {
    /// Create a new document
    pub fn new() -> Self {
        let nodes = vec![Node::Document { children: Vec::new() }];
        Document { nodes, root: 0 }
    }

    /// Create an element node
    pub fn create_element(&mut self, tag: &str) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(Node::Element {
            tag: tag.to_string(),
            attrs: BTreeMap::new(),
            children: Vec::new(),
            parent: None,
        });
        idx
    }

    /// Create a text node
    pub fn create_text(&mut self, content: &str) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(Node::Text {
            content: content.to_string(),
            parent: None,
        });
        idx
    }

    /// Append a child to a parent
    pub fn append_child(&mut self, parent: usize, child: usize) {
        if parent < self.nodes.len() && child < self.nodes.len() {
            match &mut self.nodes[parent] {
                Node::Element { children, .. } => {
                    children.push(child);
                }
                Node::Document { children, .. } => {
                    children.push(child);
                }
                _ => {}
            }

            match &mut self.nodes[child] {
                Node::Element { parent: p, .. } | Node::Text { parent: p, .. } => {
                    *p = Some(parent);
                }
                _ => {}
            }
        }
    }

    /// Get the root element
    pub fn root(&self) -> Option<usize> {
        // Find the first html element
        if let Node::Document { children } = &self.nodes[self.root] {
            for &child in children {
                if let Node::Element { tag, .. } = &self.nodes[child]
                    && tag == "html" {
                    return Some(child);
                }
            }
        }
        None
    }

    /// Get all text content of a node and its descendants
    pub fn text_content(&self, node: usize) -> String {
        if node >= self.nodes.len() {
            return String::new();
        }

        match &self.nodes[node] {
            Node::Text { content, .. } => content.clone(),
            Node::Element { children, .. } | Node::Document { children, .. } => {
                let mut result = String::new();
                for &child in children {
                    result.push_str(&self.text_content(child));
                }
                result
            }
            Node::Comment { .. } => String::new(),
        }
    }

    /// Get the document title
    pub fn title(&self) -> String {
        // Find <title> element
        if let Some(root) = self.root()
            && let Some(head) = self.find_element_by_tag(root, "head")
            && let Some(title_elem) = self.find_element_by_tag(head, "title") {
            return self.text_content(title_elem);
        }
        String::new()
    }

    /// Find an element by tag name (first occurrence)
    fn find_element_by_tag(&self, node: usize, tag: &str) -> Option<usize> {
        if node >= self.nodes.len() {
            return None;
        }

        match &self.nodes[node] {
            Node::Element { tag: t, children, .. } => {
                if t == tag {
                    return Some(node);
                }
                for &child in children {
                    if let Some(found) = self.find_element_by_tag(child, tag) {
                        return Some(found);
                    }
                }
                None
            }
            Node::Document { children, .. } => {
                for &child in children {
                    if let Some(found) = self.find_element_by_tag(child, tag) {
                        return Some(found);
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Get all links in the document
    pub fn links(&self) -> Vec<(String, String)> {
        let mut links = Vec::new();
        if let Some(root) = self.root() {
            self.collect_links(root, &mut links);
        }
        links
    }

    fn collect_links(&self, node: usize, links: &mut Vec<(String, String)>) {
        if node >= self.nodes.len() {
            return;
        }

        match &self.nodes[node] {
            Node::Element { tag, attrs, children, .. } => {
                if tag == "a"
                    && let Some(href) = attrs.get("href") {
                    let text = self.text_content(node);
                    links.push((text, href.clone()));
                }
                for &child in children {
                    self.collect_links(child, links);
                }
            }
            Node::Document { children, .. } => {
                for &child in children {
                    self.collect_links(child, links);
                }
            }
            _ => {}
        }
    }

    /// Resolve relative URLs against a base URL
    pub fn resolve_url(&self, base_url: &str, relative: &str) -> Result<String, UrlError> {
        let base = Url::parse(base_url)?;
        let resolved = base.resolve(relative)?;
        Ok(resolved.to_string())
    }

    /// Get the <base> href if present
    pub fn base_href(&self) -> Option<String> {
        if let Some(root) = self.root()
            && let Some(head) = self.find_element_by_tag(root, "head")
            && let Some(base_elem) = self.find_element_by_tag(head, "base")
            && let Node::Element { attrs, .. } = &self.nodes[base_elem] {
            return attrs.get("href").cloned();
        }
        None
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_tokenizer() {
        let mut tokenizer = Tokenizer::new("<div>Hello</div>");
        let tokens = tokenizer.tokenize();
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_document_creation() {
        let mut doc = Document::new();
        let html = doc.create_element("html");
        let body = doc.create_element("body");
        doc.append_child(0, html);
        doc.append_child(html, body);
    }
}
