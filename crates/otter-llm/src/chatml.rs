//! ChatML message builder for constructing LLM chat templates.
//! Implements the FORMAT.md chat_template_kind=0 (ChatML) template:
//! `<|im_start|>{role}\n{content}<|im_end|>\n`

use alloc::string::String;
use alloc::vec::Vec;

/// A single message in a chat conversation.
pub struct Message {
    pub role: Role,
    pub content: String,
}

/// Message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

/// Builder for ChatML formatted messages.
pub struct ChatMlBuilder {
    messages: Vec<Message>,
}

impl ChatMlBuilder {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn system(mut self, content: impl Into<String>) -> Self {
        self.messages.push(Message {
            role: Role::System,
            content: content.into(),
        });
        self
    }

    pub fn user(mut self, content: impl Into<String>) -> Self {
        self.messages.push(Message {
            role: Role::User,
            content: content.into(),
        });
        self
    }

    pub fn assistant(mut self, content: impl Into<String>) -> Self {
        self.messages.push(Message {
            role: Role::Assistant,
            content: content.into(),
        });
        self
    }

    /// Build the complete ChatML string.
    /// Format: `<|im_start|>{role}\n{content}<|im_end|>\n`
    pub fn build(&self) -> String {
        let mut result = String::new();

        for msg in &self.messages {
            result.push_str("<|im_start|>");
            result.push_str(msg.role.as_str());
            result.push('\n');
            result.push_str(&msg.content);
            result.push_str("<|im_end|>\n");
        }

        result
    }

    /// Get the list of messages (read-only).
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Clear all messages.
    pub fn clear(&mut self) {
        self.messages.clear();
    }
}

impl Default for ChatMlBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_user_message() {
        let result = ChatMlBuilder::new()
            .user("Hello")
            .build();
        assert_eq!(result, "<|im_start|>user\nHello<|im_end|>\n");
    }

    #[test]
    fn test_system_and_user() {
        let result = ChatMlBuilder::new()
            .system("You are helpful.")
            .user("What is 2+2?")
            .build();
        assert_eq!(
            result,
            "<|im_start|>system\nYou are helpful.<|im_end|>\n<|im_start|>user\nWhat is 2+2?<|im_end|>\n"
        );
    }

    #[test]
    fn test_multiturn() {
        let result = ChatMlBuilder::new()
            .system("Be brief.")
            .user("Hi")
            .assistant("Hello!")
            .user("How are you?")
            .build();
        let expected = "<|im_start|>system\nBe brief.<|im_end|>\n\
                        <|im_start|>user\nHi<|im_end|>\n\
                        <|im_start|>assistant\nHello!<|im_end|>\n\
                        <|im_start|>user\nHow are you?<|im_end|>\n";
        assert_eq!(result, expected);
    }
}
