use super::{Token, TokenizerState};

impl super::Tokenizer {
    pub(crate) fn comment_start_state(&mut self, ch: Option<char>) {
        match ch {
            Some('-') => {
                self.state = TokenizerState::CommentStartDash;
            }
            Some('>') => {
                // Parse error: <!--> is not allowed, but emit the empty comment
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
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

    pub(crate) fn comment_start_dash_state(&mut self, ch: Option<char>) {
        match ch {
            Some('-') => {
                self.state = TokenizerState::CommentEnd;
            }
            Some('>') => {
                // Parse error: <!---> is not allowed, but emit the empty comment
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
                self.state = TokenizerState::Data;
            }
            Some('\0') => {
                // NULL character must be replaced with U+FFFD
                self.temp_buffer.push('-'); // Append the dash we've been tracking
                self.temp_buffer.push('\u{FFFD}');
                self.state = TokenizerState::Comment;
            }
            Some(_c) => {
                self.temp_buffer.push('-'); // Append the dash we've been tracking
                self.pos -= 1; // Reconsume
                self.state = TokenizerState::Comment;
            }
            None => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
        }
    }

    pub(crate) fn comment_state(&mut self, ch: Option<char>) {
        match ch {
            Some('-') => {
                self.state = TokenizerState::CommentEndDash;
            }
            Some('\0') => {
                // NULL character must be replaced with U+FFFD
                self.temp_buffer.push('\u{FFFD}');
            }
            Some(c) => {
                self.temp_buffer.push(c);
            }
            None => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
        }
    }

    pub(crate) fn comment_end_dash_state(&mut self, ch: Option<char>) {
        match ch {
            Some('-') => {
                self.state = TokenizerState::CommentEnd;
            }
            Some('\0') => {
                // NULL character must be replaced with U+FFFD
                self.temp_buffer.push('-');
                self.temp_buffer.push('\u{FFFD}');
                self.state = TokenizerState::Comment;
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

    pub(crate) fn comment_end_state(&mut self, ch: Option<char>) {
        match ch {
            Some('>') => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
                self.state = TokenizerState::Data;
            }
            Some('-') => {
                self.temp_buffer.push('-');
            }
            Some('!') => {
                self.state = TokenizerState::CommentEndBang;
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

    pub(crate) fn comment_end_bang_state(&mut self, ch: Option<char>) {
        match ch {
            Some('-') => {
                self.temp_buffer.push('-');
                self.temp_buffer.push('-');
                self.temp_buffer.push('!');
                self.state = TokenizerState::CommentEndDash;
            }
            Some('>') => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
                self.state = TokenizerState::Data;
            }
            Some(_) => {
                self.temp_buffer.push('-');
                self.temp_buffer.push('-');
                self.temp_buffer.push('!');
                self.pos -= 1; // Reconsume
                self.state = TokenizerState::Comment;
            }
            None => {
                self.tokens.push(Token::Comment(self.temp_buffer.clone()));
            }
        }
    }
}
