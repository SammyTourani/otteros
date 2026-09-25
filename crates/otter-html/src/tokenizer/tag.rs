use alloc::collections::BTreeMap;

use super::{Token, TokenizerState};

impl super::Tokenizer {
    pub(crate) fn tag_open_state(&mut self, ch: Option<char>) {
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
                self.temp_buffer.push('?'); // Include the '?' in the bogus comment
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

    pub(crate) fn end_tag_open_state(&mut self, ch: Option<char>) {
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
            Some(c) => {
                self.temp_buffer.clear();
                self.temp_buffer.push(c); // Include the invalid character in the comment
                self.state = TokenizerState::BogusComment;
            }
        }
    }

    pub(crate) fn tag_name_state(&mut self, ch: Option<char>) {
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

    pub(crate) fn before_attribute_name_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {} // Skip whitespace
            Some('/') => {
                self.state = TokenizerState::SelfClosingStartTag;
            }
            Some('=') => {
                // '=' as attribute name (parse error but valid)
                // Start attribute with name "=" and empty value, then go to before-attribute-value
                // to collect the value (if any)
                self.current_attr_name.clear();
                self.current_attr_name.push('=');
                self.current_attr_value.clear();
                self.state = TokenizerState::BeforeAttributeValue;
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

    pub(crate) fn attribute_name_state(&mut self, ch: Option<char>) {
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

    pub(crate) fn after_attribute_name_state(&mut self, ch: Option<char>) {
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

    pub(crate) fn before_attribute_value_state(&mut self, ch: Option<char>) {
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

    pub(crate) fn attr_value_double_quoted_state(&mut self, ch: Option<char>) {
        match ch {
            Some('"') => {
                self.state = TokenizerState::AfterAttributeValueQuoted;
            }
            Some('&') => {
                self.parse_entity_in_attr_value();
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

    pub(crate) fn attr_value_single_quoted_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\'') => {
                self.state = TokenizerState::AfterAttributeValueQuoted;
            }
            Some('&') => {
                self.parse_entity_in_attr_value();
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

    pub(crate) fn attr_value_unquoted_state(&mut self, ch: Option<char>) {
        match ch {
            Some('\t') | Some('\n') | Some('\r') | Some(' ') => {
                self.state = TokenizerState::BeforeAttributeName;
            }
            Some('&') => {
                self.parse_entity_in_attr_value();
            }
            Some('>') => {
                self.finalize_attribute();
                self.emit_current_tag();
                self.state = TokenizerState::Data;
            }
            Some('=') if self.current_attr_name == "=" && self.current_attr_value.is_empty() => {
                // Special case: when attribute name is "=" and value is still empty,
                // the first "=" should not be appended as a value character.
                // Instead, treat it as starting a new attribute by staying in before-attribute-name.
                // But DON'T finalize yet - just go back to process this "=" as a new attribute.
                self.finalize_attribute();
                self.pos -= 1; // Reconsume to reprocess this "=" as a potential new attribute
                self.state = TokenizerState::BeforeAttributeName;
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

    pub(crate) fn after_attr_value_quoted_state(&mut self, ch: Option<char>) {
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

    pub(crate) fn self_closing_start_tag_state(&mut self, ch: Option<char>) {
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

    pub(crate) fn finalize_attribute(&mut self) {
        if !self.current_attr_name.is_empty()
            && let Some(Token::StartTag { attrs, .. }) = &mut self.current_token {
            // Only insert if the attribute doesn't already exist (keep first value)
            attrs.entry(self.current_attr_name.clone())
                .or_insert_with(|| self.current_attr_value.clone());
        }
        self.current_attr_name.clear();
        self.current_attr_value.clear();
    }

    pub(crate) fn emit_current_tag(&mut self) {
        if let Some(token) = self.current_token.take() {
            if let Token::StartTag { name, .. } = &token {
                self.last_start_tag = Some(name.clone());
            }
            self.tokens.push(token);
        }
    }
}

use alloc::string::String;
