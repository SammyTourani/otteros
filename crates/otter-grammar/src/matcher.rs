//! Frame-based JSON matcher for grammar-constrained decoding.

use alloc::vec;
use alloc::vec::Vec;
use alloc::sync::Arc;
use crate::{SchemaNode, RejectedByte};

/// A frame in the matcher's execution stack.
#[derive(Clone)]
enum Frame {
    Literal { which: LiteralKind, pos: usize },
    ToolCall { phase: ToolCallPhase, tool: Option<u16>, space_used: bool },
    Object { node: SchemaNode, next_prop: usize, phase: ObjPhase, tool_idx: Option<u16>, space_used: bool },
    String { max: Option<usize>, chars: usize, enum_vals: Option<EnumState>, pos: usize, esc: EscapeState, utf8_need: u8, next_lo: u8, next_hi: u8, str_phase: StringPhase, enum_idx: Option<u16> },
    Integer { st: IntState },
    Word { bytes: &'static [u8], pos: usize },
    Boolean { is_true: Option<bool>, pos: usize },
    Array { node: SchemaNode, count: usize, max_items: Option<usize>, phase: ArrayPhase, space_used: bool },
}

#[derive(Clone, Copy, PartialEq)]
enum ToolCallPhase {
    Open,
    AfterOpen,
    NameKey { pos: usize },
    NameColon,
    AfterNameColon,
    NameValue,
    AfterName,
    AfterComma,
    InputKey { pos: usize },
    InputColon,
    AfterInputColon,
    AfterInput,
}

#[derive(Clone, Copy)]
enum LiteralKind { Prefix, Suffix }

#[derive(Clone)]
enum ObjPhase {
    Start,
    AfterOpen,
    AfterComma,
    Key { cands: u32, pos: usize, key_buf: Vec<u8> },
    AfterKey,
    AfterColon,
    AfterValue,
}

#[derive(Clone)]
struct EnumState {
    values: Vec<Vec<u8>>,
    buf: Vec<u8>,
}

#[derive(Clone)]
enum StringPhase {
    Start,
    Content,
}

#[derive(Clone)]
enum EscapeState {
    None,
    Backslash,
    Hex { n: u8, first: u8 },
}

#[derive(Clone, Copy)]
enum IntState {
    Start,
    Minus,
    Zero,
    Digits(usize),
}

#[derive(Clone)]
enum ArrayPhase {
    Start,
    AfterOpen,
    AfterItem,
    AfterComma,
}

/// The matcher state machine.
pub struct Matcher {
    grammar: Arc<CompiledGrammar>,
    frames: Vec<Frame>,
    done: bool,
    last_enum: Option<u16>,
}

/// Compiled grammar for tool calls (internal).
pub struct CompiledGrammar {
    /// Literal prefix (e.g., `<tool_call>`).
    pub prefix: Vec<u8>,
    /// Literal suffix (e.g., `</tool_call>`).
    pub suffix: Vec<u8>,
    /// Tool names.
    pub tool_names: Vec<alloc::string::String>,
    /// Tool schemas.
    pub tool_schemas: Vec<SchemaNode>,
}

impl Matcher {
    /// Create a new matcher for a grammar (internal).
    pub(crate) fn new_internal(grammar: Arc<CompiledGrammar>) -> Self {
        Matcher {
            frames: vec![Frame::Literal { which: LiteralKind::Prefix, pos: 0 }],
            grammar,
            done: false,
            last_enum: None,
        }
    }

    /// Accept bytes from the input.
    pub fn accept_bytes(&mut self, bytes: &[u8]) -> Result<(), RejectedByte> {
        for &b in bytes {
            self.step(b)?;
        }
        Ok(())
    }

    fn step(&mut self, byte: u8) -> Result<(), RejectedByte> {
        loop {
            if self.frames.is_empty() {
                return Err(RejectedByte);
            }

            let frame = self.frames.pop().unwrap();

            match frame {
                Frame::Literal { which, mut pos } => {
                    let expected = match which {
                        LiteralKind::Prefix => &self.grammar.prefix,
                        LiteralKind::Suffix => &self.grammar.suffix,
                    };

                    if pos >= expected.len() {
                        match which {
                            LiteralKind::Prefix => {
                                // After prefix, expect the tool call object
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::Open, tool: None, space_used: false });
                                // Continue with same byte to process it with ToolCall frame
                                continue;
                            }
                            LiteralKind::Suffix => {
                                // Done processing suffix
                                self.done = true;
                                return Ok(());
                            }
                        }
                    }

                    if byte != expected[pos] {
                        return Err(RejectedByte);
                    }
                    pos += 1;

                    // Check if we've finished the literal
                    if pos >= expected.len() {
                        match which {
                            LiteralKind::Prefix => {
                                // Push ToolCall frame to be processed by next byte
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::Open, tool: None, space_used: false });
                            }
                            LiteralKind::Suffix => {
                                // Mark as done, no more frames needed
                                self.done = true;
                            }
                        }
                    } else {
                        self.frames.push(Frame::Literal { which, pos });
                    }
                    return Ok(());
                }
                Frame::ToolCall { phase, tool, mut space_used } => {
                    match phase {
                        ToolCallPhase::Open => {
                            if byte == b'{' {
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterOpen, tool, space_used: false });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ToolCallPhase::AfterOpen => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterOpen, tool, space_used });
                                return Ok(());
                            }
                            space_used = false;
                            if byte == b'"' {
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::NameKey { pos: 1 }, tool, space_used });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ToolCallPhase::NameKey { mut pos } => {
                            let name_key = b"\"name\"";
                            if byte != name_key[pos] {
                                return Err(RejectedByte);
                            }
                            pos += 1;
                            if pos == name_key.len() {
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::NameColon, tool, space_used });
                            } else {
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::NameKey { pos }, tool, space_used });
                            }
                            return Ok(());
                        }
                        ToolCallPhase::NameColon => {
                            if byte == b':' {
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterNameColon, tool, space_used: false });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ToolCallPhase::AfterNameColon => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterNameColon, tool, space_used });
                                return Ok(());
                            }
                            space_used = false;
                            if byte == b'"' {
                                // Parse the tool name as an enum
                                let tool_names = self.grammar.tool_names.to_vec();
                                let enum_vals = tool_names.iter().map(|s| s.as_bytes().to_vec()).collect();
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::NameValue, tool, space_used });
                                self.frames.push(Frame::String {
                                    max: None,
                                    chars: 0,
                                    enum_vals: Some(EnumState { values: enum_vals, buf: Vec::new() }),
                                    pos: 0,
                                    esc: EscapeState::None,
                                    utf8_need: 0,
                                    next_lo: 0x80,
                                    next_hi: 0xBF,
                                    str_phase: StringPhase::Start,
                                    enum_idx: None,
                                });
                                continue; // Process the same byte (quote) with the String frame
                            }
                            return Err(RejectedByte);
                        }
                        ToolCallPhase::NameValue => {
                            // Check if we just finished parsing the tool name
                            if let Some(idx) = self.last_enum {
                                self.last_enum = None;
                                let new_tool = Some(idx);
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterName, tool: new_tool, space_used: false });
                                continue; // Process the same byte in AfterName phase
                            }
                            // Should not reach here in normal flow - the String frame should pop after closing quote
                            return Err(RejectedByte);
                        }
                        ToolCallPhase::AfterName => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterName, tool, space_used });
                                return Ok(());
                            }
                            space_used = false;
                            if byte == b',' {
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterComma, tool, space_used });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ToolCallPhase::AfterComma => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterComma, tool, space_used });
                                return Ok(());
                            }
                            space_used = false;
                            if byte == b'"' {
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::InputKey { pos: 1 }, tool, space_used });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ToolCallPhase::InputKey { mut pos } => {
                            let input_key = b"\"input\"";
                            if byte != input_key[pos] {
                                return Err(RejectedByte);
                            }
                            pos += 1;
                            if pos == input_key.len() {
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::InputColon, tool, space_used });
                            } else {
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::InputKey { pos }, tool, space_used });
                            }
                            return Ok(());
                        }
                        ToolCallPhase::InputColon => {
                            if byte == b':' {
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterInputColon, tool, space_used: false });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ToolCallPhase::AfterInputColon => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterInputColon, tool, space_used });
                                return Ok(());
                            }
                            space_used = false;
                            if byte == b'{' {
                                let tool_idx = tool.ok_or(RejectedByte)?;
                                let schema = self.grammar.tool_schemas.get(tool_idx as usize).ok_or(RejectedByte)?.clone();
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterInput, tool, space_used });
                                self.frames.push(Frame::Object {
                                    node: schema,
                                    next_prop: 0,
                                    phase: ObjPhase::Start,
                                    tool_idx: Some(tool_idx),
                                    space_used: false,
                                });
                                continue; // Process the same byte (brace) with the Object frame
                            }
                            return Err(RejectedByte);
                        }
                        ToolCallPhase::AfterInput => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::ToolCall { phase: ToolCallPhase::AfterInput, tool, space_used });
                                return Ok(());
                            }
                            if byte == b'}' {
                                // Successfully parsed the entire tool call
                                if self.grammar.suffix.is_empty() {
                                    self.done = true;
                                } else {
                                    self.frames.push(Frame::Literal { which: LiteralKind::Suffix, pos: 0 });
                                }
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                    }
                }
                Frame::Object { node, mut next_prop, mut phase, tool_idx, mut space_used } => {
                    match phase {
                        ObjPhase::Start => {
                            if byte == b'{' {
                                phase = ObjPhase::AfterOpen;
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ObjPhase::AfterOpen => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });
                                return Ok(());
                            }
                            space_used = false;

                            if byte == b'"' {
                                let props = match &node {
                                    SchemaNode::Object { properties } => properties,
                                    _ => return Err(RejectedByte),
                                };
                                let cands = compute_candidates(props, next_prop);
                                // additionalProperties is always false: with no candidate property
                                // left (or none at all, as in an empty schema) no key may start.
                                if cands == 0 {
                                    return Err(RejectedByte);
                                }
                                phase = ObjPhase::Key { cands, pos: 0, key_buf: Vec::new() };
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });
                                return Ok(());
                            }
                            if byte == b'}' {
                                if let SchemaNode::Object { properties } = &node {
                                    for (i, (_, _, required)) in properties.iter().enumerate() {
                                        if *required && i >= next_prop {
                                            return Err(RejectedByte);
                                        }
                                    }
                                }

                                // Check if we're closing the top-level tool object (has "name" and "input" properties)
                                let is_tool_object = if let SchemaNode::Object { properties } = &node {
                                    properties.len() == 2 && properties[0].0 == "name" && properties[1].0 == "input"
                                } else {
                                    false
                                };

                                // If this is the top-level tool object, transition to suffix
                                if is_tool_object {
                                    if self.grammar.suffix.is_empty() {
                                        self.done = true;
                                    } else {
                                        self.frames.push(Frame::Literal { which: LiteralKind::Suffix, pos: 0 });
                                    }
                                }
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ObjPhase::AfterComma => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });
                                return Ok(());
                            }
                            space_used = false;

                            if byte == b'"' {
                                let props = match &node {
                                    SchemaNode::Object { properties } => properties,
                                    _ => return Err(RejectedByte),
                                };
                                let cands = compute_candidates(props, next_prop);
                                // additionalProperties is always false: with no candidate property
                                // left (or none at all, as in an empty schema) no key may start.
                                if cands == 0 {
                                    return Err(RejectedByte);
                                }
                                phase = ObjPhase::Key { cands, pos: 0, key_buf: Vec::new() };
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ObjPhase::Key { cands, pos: mut key_pos, mut key_buf } => {
                            if byte == b'"' {
                                // End of key; find which property matches
                                let props = match &node {
                                    SchemaNode::Object { properties } => properties,
                                    _ => return Err(RejectedByte),
                                };

                                // An object without properties accepts no keys (unreachable after
                                // the candidate check at the opening quote; kept as a guard).
                                if props.is_empty() {
                                    return Err(RejectedByte);
                                }

                                let mut matching_idx = None;
                                for (i, (name, _, _)) in props.iter().enumerate() {
                                    if ((cands >> i) & 1) != 0 && name.as_bytes() == &key_buf[..] {
                                        matching_idx = Some(i);
                                        break;
                                    }
                                }

                                if matching_idx.is_none() {
                                    return Err(RejectedByte);
                                }

                                let idx = matching_idx.unwrap();
                                next_prop = idx + 1;
                                phase = ObjPhase::AfterKey;
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });
                                return Ok(());
                            }

                            if key_pos >= key_buf.len() {
                                key_buf.push(byte);
                            }
                            key_pos += 1;

                            if byte < 0x20 {
                                return Err(RejectedByte);
                            }

                            // Check if any remaining candidate keys still match the partial key so far
                            let props = match &node {
                                SchemaNode::Object { properties } => properties,
                                _ => return Err(RejectedByte),
                            };
                            if !props.is_empty() {
                                // Ensure at least one candidate key still matches the partial key
                                let mut has_match = false;
                                for (i, (name, _, _)) in props.iter().enumerate() {
                                    if ((cands >> i) & 1) != 0 && key_pos <= name.len() && name.as_bytes()[..key_pos] == key_buf[..] {
                                        has_match = true;
                                        break;
                                    }
                                }
                                if !has_match {
                                    return Err(RejectedByte);
                                }
                            } else {
                                return Err(RejectedByte);
                            }

                            self.frames.push(Frame::Object { node, next_prop, phase: ObjPhase::Key { cands, pos: key_pos, key_buf }, tool_idx, space_used });
                            return Ok(());
                        }
                        ObjPhase::AfterKey => {
                            if byte == b':' {
                                phase = ObjPhase::AfterColon;
                                space_used = false;
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ObjPhase::AfterColon => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });
                                return Ok(());
                            }

                            let props = match &node {
                                SchemaNode::Object { properties } => properties,
                                _ => return Err(RejectedByte),
                            };

                            // For wildcard mode (empty properties), determine value type from the next byte
                            if props.is_empty() {
                                // In wildcard mode, check the current byte to determine value type
                                phase = ObjPhase::AfterValue;
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });

                                // Push the appropriate frame based on the byte type
                                match byte {
                                    b'"' => push_value_frame(&mut self.frames, &SchemaNode::String { max_length: None, enum_values: None }, tool_idx),
                                    b'{' => push_value_frame(&mut self.frames, &SchemaNode::Object { properties: alloc::vec![] }, tool_idx),
                                    b'[' => push_value_frame(&mut self.frames, &SchemaNode::Array { items: alloc::boxed::Box::new(SchemaNode::String { max_length: None, enum_values: None }), max_items: None }, tool_idx),
                                    b't' => self.frames.push(Frame::Word { bytes: b"true", pos: 0 }),
                                    b'f' => self.frames.push(Frame::Word { bytes: b"false", pos: 0 }),
                                    b'n' => self.frames.push(Frame::Word { bytes: b"null", pos: 0 }),
                                    b'0'..=b'9' | b'-' => push_value_frame(&mut self.frames, &SchemaNode::Integer, tool_idx),
                                    _ => return Err(RejectedByte),
                                }
                                continue;
                            } else {
                                let prop_idx = next_prop - 1;
                                if prop_idx >= props.len() {
                                    return Err(RejectedByte);
                                }
                                let schema = &props[prop_idx].1;
                                let schema_clone = schema.as_ref().clone();
                                phase = ObjPhase::AfterValue;
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });
                                push_value_frame(&mut self.frames, &schema_clone, tool_idx);
                                continue;
                            }
                        }
                        ObjPhase::AfterValue => {
                            if byte == b',' {
                                // Check if another key can follow
                                let cands = compute_candidates(match &node {
                                    SchemaNode::Object { properties } => properties,
                                    _ => return Err(RejectedByte),
                                }, next_prop);
                                if cands == 0 {
                                    // No more keys allowed after this comma
                                    return Err(RejectedByte);
                                }
                                phase = ObjPhase::AfterComma;
                                space_used = false;
                                self.frames.push(Frame::Object { node, next_prop, phase, tool_idx, space_used });
                                return Ok(());
                            }
                            if byte == b'}' {
                                if let SchemaNode::Object { properties } = &node {
                                    for (i, (_, _, required)) in properties.iter().enumerate() {
                                        if *required && i >= next_prop {
                                            return Err(RejectedByte);
                                        }
                                    }
                                }

                                // Check if we're closing the top-level tool object (has "name" and "input" properties)
                                let is_tool_object = if let SchemaNode::Object { properties } = &node {
                                    properties.len() == 2 && properties[0].0 == "name" && properties[1].0 == "input"
                                } else {
                                    false
                                };

                                // If this is the top-level tool object, transition to suffix
                                if is_tool_object {
                                    if self.grammar.suffix.is_empty() {
                                        self.done = true;
                                    } else {
                                        self.frames.push(Frame::Literal { which: LiteralKind::Suffix, pos: 0 });
                                    }
                                }

                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                    }
                }
                Frame::String { max, mut chars, mut enum_vals, pos: _str_pos, mut esc, mut utf8_need, mut next_lo, mut next_hi, mut str_phase, enum_idx } => {
                    match str_phase {
                        StringPhase::Start => {
                            if byte == b'"' {
                                str_phase = StringPhase::Content;
                                self.frames.push(Frame::String { max, chars, enum_vals, pos: _str_pos, esc, utf8_need, next_lo, next_hi, str_phase, enum_idx });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        StringPhase::Content => {}
                    }

                    if utf8_need > 0 {
                        // Continuation byte: must be 10xxxxxx
                        if (byte & 0xC0) != 0x80 {
                            return Err(RejectedByte);
                        }
                        // Check range constraints for specific lead bytes
                        if !(byte >= next_lo && byte <= next_hi) {
                            return Err(RejectedByte);
                        }
                        utf8_need -= 1;
                        self.frames.push(Frame::String { max, chars, enum_vals, pos: _str_pos, esc, utf8_need, next_lo: 0x80, next_hi: 0xBF, str_phase, enum_idx });
                        return Ok(());
                    }

                    match esc {
                        EscapeState::None => {
                            if byte == b'"' {
                                if let Some(es) = &enum_vals {
                                    // Validate that the accumulated string matches one of the enum values
                                    let mut found_idx = None;
                                    for (i, val) in es.values.iter().enumerate() {
                                        if &es.buf == val {
                                            found_idx = Some(i as u16);
                                            break;
                                        }
                                    }
                                    if found_idx.is_none() {
                                        return Err(RejectedByte);
                                    }
                                    // Record the enum index for use by ToolCall frame
                                    self.last_enum = found_idx;
                                }
                                // Closing quote: pop the frame and return (consume the byte)
                                return Ok(());
                            }
                            if byte == b'\\' {
                                // Enum values are plain ASCII: an escape could smuggle characters past
                                // the prefix check (the SmolLM2 walks found "run_c\u65e5...ommand").
                                if enum_vals.is_some() {
                                    return Err(RejectedByte);
                                }
                                esc = EscapeState::Backslash;
                                chars += 1;
                                if let Some(max) = max
                                    && chars > max {
                                        return Err(RejectedByte);
                                    }
                                self.frames.push(Frame::String { max, chars, enum_vals: enum_vals.clone(), pos: _str_pos, esc, utf8_need, next_lo, next_hi, str_phase, enum_idx });
                                return Ok(());
                            }
                            if byte < 0x20 {
                                return Err(RejectedByte);
                            }

                            // Accumulate byte for enum tracking and check if any enum value still matches
                            if let Some(mut es) = enum_vals.clone() {
                                es.buf.push(byte);

                                // Check if any enum value still matches the prefix we've accumulated so far
                                let mut has_match = false;
                                for val in &es.values {
                                    if val.len() >= es.buf.len() && val[..es.buf.len()] == es.buf[..] {
                                        has_match = true;
                                        break;
                                    }
                                }

                                if !has_match {
                                    return Err(RejectedByte);
                                }

                                enum_vals = Some(es);
                            }

                            if byte < 0x80 {
                                chars += 1;
                            } else {
                                let cont = match byte {
                                    0xC0 | 0xC1 | 0xF5..=0xFF => return Err(RejectedByte),
                                    0xC2..=0xDF => 1,
                                    0xE0 => { next_lo = 0xA0; next_hi = 0xBF; 2 },
                                    0xE1..=0xEC | 0xEE..=0xEF => 2,
                                    0xED => { next_lo = 0x80; next_hi = 0x9F; 2 },
                                    0xF0 => { next_lo = 0x90; next_hi = 0xBF; 3 },
                                    0xF1..=0xF3 => 3,
                                    0xF4 => { next_lo = 0x80; next_hi = 0x8F; 3 },
                                    0x80..=0xBF => return Err(RejectedByte),
                                    _ => 0,
                                };
                                utf8_need = cont;
                                chars += 1;
                            }

                            if let Some(max) = max
                                && chars > max {
                                    return Err(RejectedByte);
                                }

                            self.frames.push(Frame::String { max, chars, enum_vals, pos: _str_pos, esc, utf8_need, next_lo, next_hi, str_phase, enum_idx });
                            return Ok(());
                        }
                        EscapeState::Backslash => {
                            match byte {
                                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                                    esc = EscapeState::None;
                                    // Character already counted when backslash was seen
                                    self.frames.push(Frame::String { max, chars, enum_vals, pos: _str_pos, esc, utf8_need, next_lo, next_hi, str_phase, enum_idx });
                                    return Ok(());
                                }
                                b'u' => {
                                    // Character already counted when backslash was seen
                                    esc = EscapeState::Hex { n: 4, first: 0 };
                                    self.frames.push(Frame::String { max, chars, enum_vals, pos: _str_pos, esc, utf8_need, next_lo, next_hi, str_phase, enum_idx });
                                    return Ok(());
                                }
                                _ => return Err(RejectedByte),
                            }
                        }
                        EscapeState::Hex { mut n, mut first } => {
                            let digit = match byte {
                                b'0'..=b'9' => byte - b'0',
                                b'a'..=b'f' => byte - b'a' + 10,
                                b'A'..=b'F' => byte - b'A' + 10,
                                _ => return Err(RejectedByte),
                            };

                            if n == 4 {
                                first = digit;
                                n = 3;
                                self.frames.push(Frame::String { max, chars, enum_vals, pos: _str_pos, esc: EscapeState::Hex { n, first }, utf8_need, next_lo, next_hi, str_phase, enum_idx });
                                return Ok(());
                            }

                            if n == 3 && first >= 0xD && digit >= 0x8 {
                                return Err(RejectedByte); // Surrogate
                            }

                            n -= 1;
                            if n == 0 {
                                esc = EscapeState::None;
                                // Character already counted when backslash was seen
                            } else {
                                esc = EscapeState::Hex { n, first };
                            }
                            self.frames.push(Frame::String { max, chars, enum_vals, pos: _str_pos, esc, utf8_need, next_lo, next_hi, str_phase, enum_idx });
                            return Ok(());
                        }
                    }
                }
                Frame::Integer { st } => {
                    match st {
                        IntState::Start => {
                            if byte == b'-' {
                                self.frames.push(Frame::Integer { st: IntState::Minus });
                                return Ok(());
                            }
                            if byte == b'0' {
                                self.frames.push(Frame::Integer { st: IntState::Zero });
                                return Ok(());
                            }
                            if (b'1'..=b'9').contains(&byte) {
                                self.frames.push(Frame::Integer { st: IntState::Digits(1) });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        IntState::Minus => {
                            if byte == b'0' {
                                self.frames.push(Frame::Integer { st: IntState::Zero });
                                return Ok(());
                            }
                            if (b'1'..=b'9').contains(&byte) {
                                self.frames.push(Frame::Integer { st: IntState::Digits(1) });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        IntState::Zero => {
                            if byte.is_ascii_digit() {
                                return Err(RejectedByte);  // Leading zero not allowed
                            }
                            continue;
                        }
                        IntState::Digits(mut n) => {
                            if byte.is_ascii_digit() {
                                n += 1;
                                if n > 18 {
                                    return Err(RejectedByte);
                                }
                                self.frames.push(Frame::Integer { st: IntState::Digits(n) });
                                return Ok(());
                            }
                            continue;
                        }
                    }
                }
                Frame::Word { bytes, pos: mut word_pos } => {
                    if byte != bytes[word_pos] {
                        return Err(RejectedByte);
                    }
                    word_pos += 1;
                    if word_pos == bytes.len() {
                        // Word is complete, the byte is consumed
                        return Ok(());
                    }
                    self.frames.push(Frame::Word { bytes, pos: word_pos });
                    return Ok(());
                }
                Frame::Boolean { mut is_true, mut pos } => {
                    // Determine which boolean value we're parsing (true or false)
                    if is_true.is_none() {
                        // First byte determines true vs false
                        if byte == b't' {
                            is_true = Some(true);
                        } else if byte == b'f' {
                            is_true = Some(false);
                        } else {
                            return Err(RejectedByte);
                        }
                        // Consume this first byte and continue with pos=1 for next byte
                        self.frames.push(Frame::Boolean { is_true, pos: 1 });
                        return Ok(());
                    }

                    // SAFETY: is_true is now Some, we've set it in a previous step
                    let expected_bytes: &[u8] = match is_true.unwrap() {
                        true => b"true",
                        false => b"false",
                    };

                    // Check if the current byte matches the expected byte at this position
                    if pos >= expected_bytes.len() || byte != expected_bytes[pos] {
                        return Err(RejectedByte);
                    }

                    pos += 1;
                    if pos == expected_bytes.len() {
                        // Boolean is complete
                        return Ok(());
                    }

                    self.frames.push(Frame::Boolean { is_true, pos });
                    return Ok(());
                }
                Frame::Array { node, mut count, max_items, mut phase, mut space_used } => {
                    match phase {
                        ArrayPhase::Start => {
                            if byte == b'[' {
                                phase = ArrayPhase::AfterOpen;
                                self.frames.push(Frame::Array { node, count, max_items, phase, space_used });
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ArrayPhase::AfterOpen => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::Array { node, count, max_items, phase, space_used });
                                return Ok(());
                            }
                            space_used = false;

                            if byte == b']' {
                                // Empty array, close it
                                return Ok(());
                            }

                            // Start of item
                            phase = ArrayPhase::AfterItem;
                            self.frames.push(Frame::Array { node: node.clone(), count, max_items, phase, space_used });
                            push_value_frame(&mut self.frames, &node, None);
                            continue;
                        }
                        ArrayPhase::AfterItem => {
                            if byte == b',' {
                                // count is the number of commas seen so far, which equals items-1
                                // After this comma, we'll have count+1 items
                                // We allow at most max_items, so reject if count+1 >= max_items
                                if let Some(max) = max_items
                                    && count + 1 >= max {
                                        return Err(RejectedByte);
                                    }
                                count += 1;
                                phase = ArrayPhase::AfterComma;
                                space_used = false;
                                self.frames.push(Frame::Array { node, count, max_items, phase, space_used });
                                return Ok(());
                            }
                            if byte == b']' {
                                // Array closes after this item
                                return Ok(());
                            }
                            return Err(RejectedByte);
                        }
                        ArrayPhase::AfterComma => {
                            if byte == b' ' {
                                if space_used {
                                    return Err(RejectedByte);
                                }
                                space_used = true;
                                self.frames.push(Frame::Array { node, count, max_items, phase, space_used });
                                return Ok(());
                            }
                            space_used = false;

                            phase = ArrayPhase::AfterItem;
                            self.frames.push(Frame::Array { node: node.clone(), count, max_items, phase, space_used });
                            push_value_frame(&mut self.frames, &node, None);
                            continue;
                        }
                    }
                }
            }
        }
    }

    /// Accept a token from the vocabulary.
    pub fn accept_token(&mut self, vocab: &crate::Vocab, token_id: u32) -> Result<(), RejectedByte> {
        let bytes = vocab
            .token_bytes
            .get(token_id as usize)
            .ok_or(RejectedByte)?;
        self.accept_bytes(bytes)
    }

    /// Get mask of allowed tokens.
    pub fn allowed_tokens(&self, vocab: &crate::Vocab, mask: &mut crate::TokenMask) {
        mask.clear();

        for (token_id, token_bytes) in vocab.token_bytes.iter().enumerate() {
            let token_id = token_id as u32;

            // The end token is allowed only when the matcher is complete
            if token_id == vocab.end_token {
                if self.is_complete() {
                    mask.allow(token_id);
                }
                continue;
            }

            if vocab.is_special(token_id) {
                continue;
            }

            let mut test = self.clone();
            if test.accept_bytes(token_bytes).is_ok() {
                mask.allow(token_id);
            }
        }
    }

    /// Check if the matcher is in a complete state.
    pub fn is_complete(&self) -> bool {
        self.done && self.frames.is_empty()
    }

    /// Reset the matcher to initial state.
    pub fn reset(&mut self) {
        self.frames.clear();
        self.frames.push(Frame::Literal { which: LiteralKind::Prefix, pos: 0 });
        self.done = false;
        self.last_enum = None;
    }
}

impl Clone for Matcher {
    fn clone(&self) -> Self {
        Matcher {
            grammar: self.grammar.clone(),
            frames: self.frames.clone(),
            done: self.done,
            last_enum: self.last_enum,
        }
    }
}

fn compute_candidates(props: &[(alloc::string::String, alloc::boxed::Box<SchemaNode>, bool)], next_prop: usize) -> u32 {
    let mut cands = 0u32;
    for (i, _) in props.iter().enumerate() {
        if i >= next_prop {
            // Check if there's any required property in [next_prop, i)
            let has_required_before = (next_prop..i).any(|j| props[j].2);
            if !has_required_before {
                cands |= 1 << i;
            }
        }
    }
    cands
}

fn push_value_frame(frames: &mut Vec<Frame>, schema: &SchemaNode, tool_idx: Option<u16>) {
    match schema {
        SchemaNode::String { max_length, enum_values } => {
            frames.push(Frame::String {
                max: *max_length,
                chars: 0,
                enum_vals: enum_values.as_ref().map(|vals| EnumState {
                    values: vals.iter().map(|s| s.as_bytes().to_vec()).collect(),
                    buf: Vec::new(),
                }),
                pos: 0,
                esc: EscapeState::None,
                utf8_need: 0,
                next_lo: 0x80,
                next_hi: 0xBF,
                str_phase: StringPhase::Start,
                enum_idx: None,
            });
        }
        SchemaNode::Integer => {
            frames.push(Frame::Integer { st: IntState::Start });
        }
        SchemaNode::Boolean => {
            frames.push(Frame::Boolean { is_true: None, pos: 0 });
        }
        SchemaNode::Null => {
            frames.push(Frame::Word { bytes: b"null", pos: 0 });
        }
        SchemaNode::Array { items, max_items } => {
            frames.push(Frame::Array {
                node: items.as_ref().clone(),
                count: 0,
                max_items: *max_items,
                phase: ArrayPhase::Start,
                space_used: false,
            });
        }
        SchemaNode::Object { properties: _ } => {
            frames.push(Frame::Object {
                node: schema.clone(),
                next_prop: 0,
                phase: ObjPhase::Start,
                tool_idx,
                space_used: false,
            });
        }
    }
}

