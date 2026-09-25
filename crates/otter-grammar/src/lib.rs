#![no_std]
#![warn(missing_docs)]

//! Grammar-constrained decoding for tool calls.

extern crate alloc;

mod matcher;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

pub use matcher::{Matcher, CompiledGrammar};

/// Error types for grammar compilation and matching.
#[derive(Debug, Clone)]
pub enum GrammarError {
    /// Failed to parse input JSON.
    ParseError,
    /// Schema uses unsupported keyword.
    UnsupportedKeyword(String),
    /// Invalid schema structure.
    InvalidSchema(String),
}

impl fmt::Display for GrammarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GrammarError::ParseError => write!(f, "parse error"),
            GrammarError::UnsupportedKeyword(k) => write!(f, "unsupported keyword: {}", k),
            GrammarError::InvalidSchema(msg) => write!(f, "invalid schema: {}", msg),
        }
    }
}

/// A JSON schema node.
#[derive(Clone, Debug)]
pub enum SchemaNode {
    /// Object with ordered properties.
    Object {
        #[allow(missing_docs)]
        properties: Vec<(String, Box<SchemaNode>, bool)>,
    },
    /// String with optional max length and enum values.
    String {
        #[allow(missing_docs)]
        max_length: Option<usize>,
        #[allow(missing_docs)]
        enum_values: Option<Vec<String>>,
    },
    /// Integer value.
    Integer,
    /// Boolean value.
    Boolean,
    /// Null value.
    Null,
    /// Array with items and max count.
    Array {
        #[allow(missing_docs)]
        items: Box<SchemaNode>,
        #[allow(missing_docs)]
        max_items: Option<usize>,
    },
}

/// The compiled grammar for tool calls.
pub struct Grammar {
    compiled: Arc<CompiledGrammar>,
}

/// Error from rejecting a byte.
#[derive(Debug, Clone)]
pub struct RejectedByte;

/// Vocabulary wrapper.
pub struct Vocab {
    /// Token bytes.
    pub token_bytes: Vec<Vec<u8>>,
    /// End token ID.
    pub end_token: u32,
    special_tokens: Vec<u32>,
}

/// Token mask for allowed tokens.
pub struct TokenMask {
    allowed: Vec<bool>,
}

impl Grammar {
    /// Build a tool call grammar.
    pub fn tool_call(
        tools: &[(&str, &str)],
        prefix: &[u8],
        suffix: &[u8],
    ) -> Result<Self, GrammarError> {
        let mut tool_names = Vec::new();
        let mut tool_schemas = Vec::new();

        for (name, schema_json) in tools {
            tool_names.push(name.to_string());
            let val = otter_json::parse_str(schema_json).map_err(|_| GrammarError::ParseError)?;

            // Ensure tool input schema is an object type
            let obj = val
                .as_object()
                .ok_or_else(|| GrammarError::InvalidSchema("tool input must be an object".into()))?;
            let schema_type = obj.get("type").and_then(|v| v.as_str());
            if schema_type != Some("object") {
                return Err(GrammarError::InvalidSchema("tool input must be an object".into()));
            }

            let schema = compile_schema(&val)?;
            tool_schemas.push(schema);
        }

        Ok(Grammar {
            compiled: Arc::new(CompiledGrammar {
                prefix: prefix.to_vec(),
                suffix: suffix.to_vec(),
                tool_names,
                tool_schemas,
            }),
        })
    }
}

fn compile_schema(val: &otter_json::Value) -> Result<SchemaNode, GrammarError> {
    let obj = val
        .as_object()
        .ok_or_else(|| GrammarError::InvalidSchema("must be an object".into()))?;

    if let Some(const_val) = obj.get("const") {
        let s = const_val
            .as_str()
            .ok_or_else(|| GrammarError::InvalidSchema("const must be a string".into()))?;
        return Ok(SchemaNode::String {
            max_length: None,
            enum_values: Some(alloc::vec![s.to_string()]),
        });
    }

    let type_str = obj.get("type").and_then(|v| v.as_str());
    match type_str {
        Some("object") => {
            let properties_val = obj
                .get("properties")
                .and_then(|v| v.as_object())
                .ok_or_else(|| GrammarError::InvalidSchema("object must have properties".into()))?;

            let required_set = if let Some(req_val) = obj.get("required").and_then(|v| v.as_array()) {
                req_val.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>()
            } else {
                Vec::new()
            };

            let mut props = Vec::new();
            for (key, schema_val) in properties_val.iter() {
                let schema = compile_schema(schema_val)?;
                let is_required = required_set.iter().any(|k| k == &key);
                props.push((key.to_string(), Box::new(schema), is_required));
            }

            Ok(SchemaNode::Object { properties: props })
        }
        Some("string") => {
            for (key, _) in obj.iter() {
                if matches!(
                    key,
                    "pattern" | "minLength" | "format"
                ) {
                    return Err(GrammarError::UnsupportedKeyword(key.to_string()));
                }
            }
            let max_length = obj.get("maxLength").and_then(|v| v.as_i64()).map(|n| n as usize);
            let enum_values = obj.get("enum").and_then(|v| v.as_array()).map(|arr| {
                arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
            });
            Ok(SchemaNode::String { max_length, enum_values })
        }
        Some("integer") => {
            for (key, _) in obj.iter() {
                if matches!(
                    key,
                    "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf"
                ) {
                    return Err(GrammarError::UnsupportedKeyword(key.to_string()));
                }
            }
            Ok(SchemaNode::Integer)
        }
        Some("boolean") => Ok(SchemaNode::Boolean),
        Some("null") => Ok(SchemaNode::Null),
        Some("array") => {
            for (key, _) in obj.iter() {
                if matches!(
                    key,
                    "minItems" | "uniqueItems"
                ) {
                    return Err(GrammarError::UnsupportedKeyword(key.to_string()));
                }
            }
            let items = obj
                .get("items")
                .ok_or_else(|| GrammarError::InvalidSchema("array must have items".into()))?;
            let items_schema = compile_schema(items)?;
            let max_items = obj.get("maxItems").and_then(|v| v.as_i64()).map(|n| n as usize);
            Ok(SchemaNode::Array {
                items: Box::new(items_schema),
                max_items,
            })
        }
        _ => {
            for (key, _) in obj.iter() {
                if matches!(
                    key,
                    "pattern" | "minLength" | "minimum" | "maximum" | "exclusiveMinimum"
                        | "exclusiveMaximum" | "minItems" | "uniqueItems" | "minProperties"
                        | "maxProperties" | "dependencies" | "allOf" | "oneOf"
                ) {
                    return Err(GrammarError::UnsupportedKeyword(key.to_string()));
                }
            }
            Err(GrammarError::InvalidSchema("unknown type".into()))
        }
    }
}

impl Matcher {
    /// Create a new matcher.
    pub fn new(grammar: &Grammar) -> Self {
        matcher::Matcher::new_internal(grammar.compiled.clone())
    }
}

impl Vocab {
    /// Create a new vocabulary.
    pub fn new(token_bytes: Vec<Vec<u8>>, end_token: u32, special: &[u32]) -> Self {
        Vocab {
            token_bytes,
            end_token,
            special_tokens: special.to_vec(),
        }
    }

    /// Check if a token is special.
    pub fn is_special(&self, token_id: u32) -> bool {
        self.special_tokens.contains(&token_id)
    }
}

impl TokenMask {
    /// Create a new mask.
    pub fn new(vocab_size: usize) -> Self {
        TokenMask {
            allowed: alloc::vec![false; vocab_size],
        }
    }

    /// Mark a token as allowed.
    pub fn allow(&mut self, token_id: u32) {
        if (token_id as usize) < self.allowed.len() {
            self.allowed[token_id as usize] = true;
        }
    }

    /// Check if a token is allowed.
    pub fn is_allowed(&self, token_id: u32) -> bool {
        (token_id as usize) < self.allowed.len() && self.allowed[token_id as usize]
    }

    /// Count allowed tokens.
    pub fn count_allowed(&self) -> usize {
        self.allowed.iter().filter(|&&a| a).count()
    }

    /// Clear the mask.
    pub fn clear(&mut self) {
        self.allowed.fill(false);
    }
}
