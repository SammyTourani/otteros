#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::mem;
use otter_llm::{Model, Tokenizer, TokenizationError, chatml::{ChatMlBuilder, Role}, Session};
use otter_sample::{Sampler, Params};

/// Sampling and generation configuration.
#[derive(Clone, Debug)]
pub struct ChatConfig {
    /// Temperature for sampling; 0.0 = greedy (argmax).
    pub temperature: f32,
    /// Top-k filtering; 0 = off.
    pub top_k: usize,
    /// Top-p (nucleus) filtering; 1.0 = off.
    pub top_p: f32,
    /// Repetition penalty for recent tokens; 1.0 = off.
    pub repetition_penalty: f32,
    /// RNG seed for the sampler.
    pub seed: u64,
    /// Maximum number of tokens to generate.
    pub max_new_tokens: usize,
    /// Token IDs that end the turn without being emitted.
    pub stop_ids: Vec<u32>,
}

impl Default for ChatConfig {
    fn default() -> Self {
        // The default stop_ids will be set to contain <|im_end|> by ChatEngine::new.
        ChatConfig {
            temperature: 0.0,
            top_k: 0,
            top_p: 1.0,
            repetition_penalty: 1.0,
            seed: 0,
            max_new_tokens: 256,
            stop_ids: Vec::new(), // Will be populated by the engine
        }
    }
}

/// Clock trait for measuring timing.
pub trait Clock {
    fn now_ms(&self) -> u64;
}

/// Reason the generation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoneReason {
    EndOfTurn,
    MaxTokens,
    Cancelled,
}

/// Result of a generation step.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Text(String),
    Done(DoneReason),
}

/// Generation statistics.
#[derive(Debug, Clone, Copy)]
pub struct Stats {
    pub prompt_tokens: usize,
    pub generated_tokens: usize,
    pub prefill_ms: u64,
    pub generate_ms: u64,
}

/// Error type for ChatEngine.
#[derive(Debug, Clone)]
pub enum ChatError {
    TokenizationError(TokenizationError),
    ForwardPassError(&'static str),
    InvalidTokenId,
}

impl From<TokenizationError> for ChatError {
    fn from(e: TokenizationError) -> Self {
        ChatError::TokenizationError(e)
    }
}

/// The main chat engine for offline inference.
pub struct ChatEngine<'a> {
    #[allow(dead_code)]
    model: &'a Model<'a>,
    tokenizer: Tokenizer,
    session: Session<'a>,
    sampler: Sampler,
    config: ChatConfig,
    clock: Box<dyn Clock>,

    // State
    prompt_ids: Vec<u32>,
    generated_ids: Vec<u32>,
    pending_logits: Vec<f32>,
    cancelled: bool,
    decoder_buffer: Vec<u8>,

    // Timing
    prefill_start_ms: u64,
    generate_start_ms: u64,
    prefill_ms: u64,
    generate_ms: u64,
}

impl<'a> ChatEngine<'a> {
    /// Create a new ChatEngine.
    pub fn new(
        model: &'a Model<'a>,
        mut config: ChatConfig,
        clock: Box<dyn Clock>,
    ) -> Result<Self, ChatError> {
        let tokenizer = model.tokenizer()?;

        // Find <|im_end|> token ID
        let im_end_id = tokenizer.encode("<|im_end|>")?
            .first()
            .copied()
            .ok_or(ChatError::InvalidTokenId)?;

        // Set default stop_ids to contain <|im_end|>
        if config.stop_ids.is_empty() {
            config.stop_ids.push(im_end_id);
        }

        Ok(ChatEngine {
            model,
            tokenizer,
            session: Session::new(model),
            sampler: Sampler::new(config.seed),
            config,
            clock,
            prompt_ids: Vec::new(),
            generated_ids: Vec::new(),
            pending_logits: Vec::new(),
            cancelled: false,
            decoder_buffer: Vec::new(),
            prefill_start_ms: 0,
            generate_start_ms: 0,
            prefill_ms: 0,
            generate_ms: 0,
        })
    }

    /// Start a chat turn with the given messages. Messages should end with an open assistant turn.
    pub fn start(&mut self, messages: &[(Role, &str)]) -> Result<(), ChatError> {
        self.prefill_start_ms = self.clock.now_ms();

        let mut builder = ChatMlBuilder::new();
        for (role, content) in messages {
            match role {
                Role::System => builder = builder.system(content.to_string()),
                Role::User => builder = builder.user(content.to_string()),
                Role::Assistant => builder = builder.assistant(content.to_string()),
            }
        }

        let mut prompt_text = builder.build();
        // prompt_text is now: <|im_start|>system\n...<|im_end|>\n<|im_start|>user\n...<|im_end|>\n
        // Append the open assistant turn
        prompt_text.push_str("<|im_start|>assistant\n");

        // Tokenize the prompt
        self.prompt_ids = self.tokenizer.encode(&prompt_text)?;

        // Feed all prompt tokens to the session to get logits at the last position
        for &token_id in &self.prompt_ids {
            self.pending_logits = self.session.feed(token_id)
                .map_err(ChatError::ForwardPassError)?;
        }

        let prefill_end_ms = self.clock.now_ms();
        self.prefill_ms = prefill_end_ms.saturating_sub(self.prefill_start_ms);
        self.generate_start_ms = prefill_end_ms;

        Ok(())
    }

    /// Generate up to `budget` new tokens and return decoded text or Done.
    pub fn step(&mut self, budget: usize) -> Step {
        if self.cancelled {
            self.generate_ms = self.clock.now_ms().saturating_sub(self.generate_start_ms);
            return Step::Done(DoneReason::Cancelled);
        }

        if self.generated_ids.len() >= self.config.max_new_tokens {
            self.generate_ms = self.clock.now_ms().saturating_sub(self.generate_start_ms);
            return Step::Done(DoneReason::MaxTokens);
        }

        let mut accumulated_text = String::new();

        for _ in 0..budget {
            // Stop if we've already hit max tokens
            if self.generated_ids.len() >= self.config.max_new_tokens {
                let end_ms = self.clock.now_ms();
                self.generate_ms = end_ms.saturating_sub(self.generate_start_ms);
                return Step::Done(DoneReason::MaxTokens);
            }

            // Sample a token
            let sampled = self.sampler.sample(
                &self.pending_logits,
                None,
                &self.generated_ids,
                &Params {
                    temperature: self.config.temperature,
                    top_k: self.config.top_k,
                    top_p: self.config.top_p,
                    repetition_penalty: self.config.repetition_penalty,
                },
            );

            let token_id = match sampled {
                Some(id) => id,
                None => break, // No valid token; end generation
            };

            // Check for stop tokens (not counted as generated)
            if self.config.stop_ids.contains(&token_id) {
                // Flush decoder buffer, trying to extract complete UTF-8
                let buffer = mem::take(&mut self.decoder_buffer);
                if let Ok(text) = String::from_utf8(buffer) {
                    accumulated_text.push_str(&text);
                }
                let end_ms = self.clock.now_ms();
                self.generate_ms = end_ms.saturating_sub(self.generate_start_ms);
                // Return accumulated text if available, otherwise Done
                if !accumulated_text.is_empty() {
                    return Step::Text(accumulated_text);
                }
                return Step::Done(DoneReason::EndOfTurn);
            }

            // Feed the token and get new logits
            self.pending_logits = match self.session.feed(token_id) {
                Ok(logits) => logits,
                Err(_) => break, // Session error; end generation
            };

            // Add token to generated list
            self.generated_ids.push(token_id);

            // Decode token bytes and feed to buffer
            let idx = token_id as usize;
            if idx >= self.tokenizer.id_to_bytes.len() {
                break; // Invalid token
            }
            self.decoder_buffer.extend_from_slice(&self.tokenizer.id_to_bytes[idx]);

            // Extract valid UTF-8 from buffer
            let mut valid_until = 0;
            let mut pos = 0;
            while pos < self.decoder_buffer.len() {
                let lead = self.decoder_buffer[pos];
                let char_len = if (lead & 0x80) == 0 {
                    1
                } else if (lead & 0xE0) == 0xC0 {
                    2
                } else if (lead & 0xF0) == 0xE0 {
                    3
                } else if (lead & 0xF8) == 0xF0 {
                    4
                } else {
                    break; // Invalid UTF-8 byte
                };

                if pos + char_len <= self.decoder_buffer.len() {
                    let mut valid_char = true;
                    for i in 1..char_len {
                        if (self.decoder_buffer[pos + i] & 0xC0) != 0x80 {
                            valid_char = false;
                            break;
                        }
                    }
                    if valid_char {
                        valid_until = pos + char_len;
                        pos += char_len;
                    } else {
                        break; // Invalid continuation byte
                    }
                } else {
                    break; // Incomplete character
                }
            }

            // Emit valid UTF-8 and keep incomplete bytes
            if valid_until > 0 {
                if let Ok(text) = alloc::string::String::from_utf8(self.decoder_buffer.drain(..valid_until).collect()) {
                    accumulated_text.push_str(&text);
                }
            }

            // Check if we just reached max tokens
            if self.generated_ids.len() >= self.config.max_new_tokens {
                // Try to flush remaining complete UTF-8
                if core::str::from_utf8(&self.decoder_buffer).is_ok() {
                    let buffer = mem::take(&mut self.decoder_buffer);
                    if let Ok(text) = String::from_utf8(buffer) {
                        accumulated_text.push_str(&text);
                    }
                }
                let end_ms = self.clock.now_ms();
                self.generate_ms = end_ms.saturating_sub(self.generate_start_ms);
                // Return accumulated text if available, otherwise Done
                if !accumulated_text.is_empty() {
                    return Step::Text(accumulated_text);
                }
                return Step::Done(DoneReason::MaxTokens);
            }
        }

        // Exhausted budget without reaching stop condition; return accumulated text
        let end_ms = self.clock.now_ms();
        self.generate_ms = end_ms.saturating_sub(self.generate_start_ms);
        Step::Text(accumulated_text)
    }

    /// Cancel generation; the next step will return Done.
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// Get the prompt token IDs.
    pub fn prompt_ids(&self) -> &[u32] {
        &self.prompt_ids
    }

    /// Get the generated token IDs.
    pub fn generated_ids(&self) -> &[u32] {
        &self.generated_ids
    }

    /// Get generation statistics.
    pub fn stats(&self) -> Stats {
        Stats {
            prompt_tokens: self.prompt_ids.len(),
            generated_tokens: self.generated_ids.len(),
            prefill_ms: self.prefill_ms,
            generate_ms: self.generate_ms,
        }
    }
}
