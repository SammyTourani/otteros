use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

mod data;
mod char_ref;
mod tag;
mod comment;
mod doctype;
mod text_states;

/// Initial tokenizer state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InitialState {
    Data,
    Plaintext,
    Rcdata,
    Rawtext,
    ScriptData,
    CdataSection,
}

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
    CommentEndBang,
    DocType,
    BeforeDocTypeName,
    DocTypeName,
    AfterDocTypeName,
    BogusDocType,
    BogusComment,
    CharacterReference,
}

/// HTML tokenizer
pub struct Tokenizer {
    input: Vec<char>,
    pos: usize,
    tokens: Vec<Token>,
    state: TokenizerState,
    initial_state: InitialState,
    last_start_tag: Option<String>,

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
        Self::with_state(html, InitialState::Data, None)
    }

    /// Create a tokenizer with a specific initial state and optional last start tag
    pub fn with_state(html: &str, initial_state: InitialState, last_start_tag: Option<&str>) -> Self {
        // Preprocess input: normalize CR/CRLF to LF per WHATWG spec
        let mut input: Vec<char> = html.chars().collect();
        let mut i = 0;
        while i < input.len() {
            if input[i] == '\r' {
                if i + 1 < input.len() && input[i + 1] == '\n' {
                    // CRLF -> LF
                    input.remove(i);
                } else {
                    // CR -> LF
                    input[i] = '\n';
                }
            }
            i += 1;
        }

        let state = match initial_state {
            InitialState::Data => TokenizerState::Data,
            InitialState::Plaintext => TokenizerState::Data, // Will be handled specially
            InitialState::Rcdata => TokenizerState::Data, // Will be handled specially
            InitialState::Rawtext => TokenizerState::Data, // Will be handled specially
            InitialState::ScriptData => TokenizerState::Data, // Will be handled specially
            InitialState::CdataSection => TokenizerState::Data, // Will be handled specially
        };
        Tokenizer {
            input,
            pos: 0,
            tokens: Vec::new(),
            state,
            initial_state,
            last_start_tag: last_start_tag.map(|s| s.to_string()),
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

    /// Tokenize the entire input
    pub fn tokenize(&mut self) -> Vec<Token> {
        // Handle special initial states
        match self.initial_state {
            InitialState::Plaintext => return self.tokenize_plaintext(),
            InitialState::Rcdata => return self.tokenize_rcdata(),
            InitialState::Rawtext => return self.tokenize_rawtext(),
            InitialState::ScriptData => return self.tokenize_scriptdata(),
            InitialState::CdataSection => return self.tokenize_cdata(),
            InitialState::Data => {} // Normal tokenization
        }

        while self.pos < self.input.len() {
            let ch = self.current_char();
            self.process_state(ch);
            self.pos += 1;
        }

        // Handle EOF - special handling for CharacterReference state
        if self.state == TokenizerState::CharacterReference {
            self.process_state(None);
        }

        // Handle EOF for other states
        match self.state {
            TokenizerState::Comment | TokenizerState::CommentStart | TokenizerState::CommentStartDash |
            TokenizerState::CommentEndDash | TokenizerState::CommentEnd | TokenizerState::CommentEndBang | TokenizerState::BogusComment => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
            TokenizerState::TagName | TokenizerState::BeforeAttributeName |
            TokenizerState::AttributeName | TokenizerState::AfterAttributeName |
            TokenizerState::BeforeAttributeValue | TokenizerState::AttributeValueDoubleQuoted |
            TokenizerState::AttributeValueSingleQuoted | TokenizerState::AttributeValueUnquoted |
            TokenizerState::AfterAttributeValueQuoted | TokenizerState::SelfClosingStartTag => {
                // EOF during tag parsing: discard the incomplete tag, emit nothing
            }
            TokenizerState::DocType | TokenizerState::BeforeDocTypeName |
            TokenizerState::DocTypeName | TokenizerState::AfterDocTypeName |
            TokenizerState::BogusDocType => {
                // EOF during doctype parsing: emit the incomplete doctype
                self.emit_doctype();
            }
            TokenizerState::TagOpen => {
                // EOF after <
                self.tokens.push(Token::Character('<'));
            }
            TokenizerState::EndTagOpen => {
                // EOF after </
                self.tokens.push(Token::Character('<'));
                self.tokens.push(Token::Character('/'));
            }
            TokenizerState::MarkupDeclarationOpen => {
                // EOF after <! - treat as bogus comment
                self.tokens.push(Token::Comment(String::new()));
            }
            TokenizerState::CharacterReference => {
                // This shouldn't happen if process_state(None) was called above
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
            TokenizerState::CommentEndBang => self.comment_end_bang_state(ch),
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

    fn markup_declaration_open_state(&mut self, ch: Option<char>) {
        // Check for DOCTYPE
        if self.match_string("DOCTYPE") {
            self.state = TokenizerState::DocType;
            self.doctype_force_quirks = true;  // Incomplete DOCTYPE starts in quirks mode
            return;
        }

        // Check for comment
        if ch == Some('-') && self.peek_char(1) == Some('-') {
            self.pos += 1; // Skip next -
            self.state = TokenizerState::CommentStart;
            self.temp_buffer.clear();
            return;
        }

        // Bogus comment - reconsume the current character
        self.pos -= 1; // Reconsume
        self.temp_buffer.clear();
        self.state = TokenizerState::BogusComment;
    }

    fn match_string(&mut self, s: &str) -> bool {
        let chars: Vec<char> = s.chars().collect();
        if self.pos + chars.len() > self.input.len() {
            return false;
        }

        for (i, &c) in chars.iter().enumerate() {
            let input_char = self.input[self.pos + i];
            // Case-insensitive match for ASCII characters (for DOCTYPE and PUBLIC/SYSTEM keywords)
            if !c.eq_ignore_ascii_case(&input_char) {
                return false;
            }
        }

        self.pos += chars.len() - 1;
        true
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
            Some('\0') => {
                // NULL character must be replaced with U+FFFD
                self.temp_buffer.push('\u{FFFD}');
            }
            Some(c) => {
                self.temp_buffer.push(c);
            }
        }
    }
}
