//! `otter-claude`: OtterOS's Claude Messages API client.
//!
//! A library for building Claude Messages API requests, parsing streaming responses
//! event by event, running the tool-use loop, and handling refusals and limits.
//! Tested entirely offline with recorded streams and a local mock server; no test
//! contacts the real API (DECISIONS.md D23).
//!
//! DECISIONS.md D2, D23, D27: pure no_std + alloc crate, zero external dependencies,
//! path dependencies on otter-json and otter-http.

#![no_std]
#![warn(missing_docs)]

extern crate alloc;

mod client;
/// Request building for the Claude Messages API.
pub mod request;
/// Response parsing and event handling for streaming Claude API responses.
pub mod response;
/// Tool input validation against JSON schemas.
pub mod validation;
/// Tool use loop and message API integration.
pub mod tool_loop;

pub use client::{Client, ApiKey, ClientConfig};
pub use request::{Conversation, Message, ContentBlock, Tool};
pub use response::{TurnEvent, TurnResult, StopReason, StreamParser};
pub use validation::ValidationError;
pub use tool_loop::{Transport, ToolHost, LoopOutcome, LoopError, run_tool_loop};
