//! A Hugging Face "fast" (byte-level BPE) tokenizer matching the reference in
//! tools/otter-convert/src/tokenizer.rs exactly: GPT-2 byte-to-unicode mapping,
//! digit pre-splitting, GPT-2 regex split, BPE merges, and special token matching.
//! This is a faithful translation to no_std+alloc using BTreeMap and (u32, u32) merge pairs
//! from the .otm format instead of (String, String) pairs from tokenizer.json.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

/// Tokenization errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenizationError {
    VocabNotFound(String),
    InvalidUtf8,
    MissingVocab,
    DecodingError,
}

/// GPT-2's byte<->"visible" Unicode character mapping (Radford et al.'s `bytes_to_unicode`).
/// Printable bytes (0x21..0x7E, 0xA1..0xAC, 0xAE..0xFF) map to themselves.
/// The rest (controls, space, DEL, Latin-1 gaps) map to consecutive code points starting at 256.
pub fn byte_to_unicode_table() -> [char; 256] {
    let is_printable = |b: u32| (0x21..=0x7E).contains(&b) || (0xA1..=0xAC).contains(&b) || (0xAE..=0xFF).contains(&b);
    let mut table = ['\0'; 256];
    for (b, slot) in table.iter_mut().enumerate() {
        if is_printable(b as u32) {
            *slot = char::from_u32(b as u32).expect("printable byte range is valid char");
        }
    }
    let mut n = 0u32;
    for (b, slot) in table.iter_mut().enumerate() {
        if !is_printable(b as u32) {
            *slot = char::from_u32(256 + n).expect("256.. is a valid BMP range");
            n += 1;
        }
    }
    table
}

pub struct AddedToken {
    pub id: u32,
    pub content: String,
}

pub struct Tokenizer {
    /// Byte-mapped token string -> id.
    pub vocab: BTreeMap<String, u32>,
    /// id -> raw bytes (after reversing the byte-level mapping).
    pub id_to_bytes: Vec<Vec<u8>>,
    /// (left_str, right_str) -> merge rank (lower merges first).
    pub merge_rank: BTreeMap<(String, String), u32>,
    /// In exact order from .otm; used (longest first) to greedily match literal special-token text.
    pub added_tokens: Vec<AddedToken>,
    /// Whether the pre-tokenizer isolates every digit into its own piece.
    pub isolate_digits: bool,
    pub byte_to_unicode: [char; 256],
}

impl Tokenizer {
    /// Builds a tokenizer from the data loaded by .otm loader.
    /// Takes: id_to_bytes (the vocab), merge_pairs as (u32, u32) in rank order,
    /// added_tokens as (id, content), and isolate_digits flag.
    pub fn from_loader_data(
        id_to_bytes: &[Vec<u8>],
        merge_pairs: &[(u32, u32)],
        added_tokens: &[(u32, &str)],
        isolate_digits: bool,
    ) -> Result<Self, TokenizationError> {
        let byte_to_unicode = byte_to_unicode_table();

        // Build vocab: byte-mapped string -> id.
        let mut vocab = BTreeMap::new();
        for (id, bytes) in id_to_bytes.iter().enumerate() {
            let mut mapped = String::new();
            for &b in bytes {
                mapped.push(byte_to_unicode[b as usize]);
            }
            vocab.insert(mapped, id as u32);
        }

        // Build merge_rank from (u32, u32) pairs by converting to string pairs.
        // merge_pairs are in rank order (0, 1, 2, ...).
        let mut merge_rank = BTreeMap::new();
        for (rank, &(left_id, right_id)) in merge_pairs.iter().enumerate() {
            // Convert IDs to byte-mapped strings
            if (left_id as usize) < id_to_bytes.len() && (right_id as usize) < id_to_bytes.len() {
                let left_bytes = &id_to_bytes[left_id as usize];
                let right_bytes = &id_to_bytes[right_id as usize];

                let mut left_str = String::new();
                for &b in left_bytes {
                    left_str.push(byte_to_unicode[b as usize]);
                }

                let mut right_str = String::new();
                for &b in right_bytes {
                    right_str.push(byte_to_unicode[b as usize]);
                }

                merge_rank.insert((left_str, right_str), rank as u32);
            }
        }

        // Build added tokens, sorted by length descending (greedy matching).
        let mut added = Vec::new();
        for (id, content) in added_tokens {
            added.push(AddedToken {
                id: *id,
                content: content.to_string(),
            });
        }
        added.sort_by_key(|a| core::cmp::Reverse(a.content.len()));

        Ok(Tokenizer {
            vocab,
            id_to_bytes: id_to_bytes.to_vec(),
            merge_rank,
            added_tokens: added,
            isolate_digits,
            byte_to_unicode,
        })
    }

    /// Encodes `text` with no automatic BOS/EOS handling, matching Hugging Face's
    /// `tokenizer.encode(text, add_special_tokens=False)`: literal added-token text
    /// is matched and emitted directly wherever it occurs, and everything between
    /// such matches goes through the normal digit-split + GPT-2 regex + BPE pipeline.
    pub fn encode(&self, text: &str) -> Result<Vec<u32>, TokenizationError> {
        let mut ids = Vec::new();
        let mut rest = text;

        while !rest.is_empty() {
            // Try to match an added token (longest first due to sort).
            if let Some(at) = self.added_tokens.iter().find(|at| rest.starts_with(&at.content)) {
                ids.push(at.id);
                rest = &rest[at.content.len()..];
                continue;
            }

            // Find the next position where an added token could start.
            let mut split_at = rest.len();
            for (i, _) in rest.char_indices().skip(1) {
                if self.added_tokens.iter().any(|at| rest[i..].starts_with(&at.content)) {
                    split_at = i;
                    break;
                }
            }

            // Encode the normal chunk.
            self.encode_normal_chunk(&rest[..split_at], &mut ids)?;
            rest = &rest[split_at..];
        }

        Ok(ids)
    }

    fn encode_normal_chunk(&self, chunk: &str, out: &mut Vec<u32>) -> Result<(), TokenizationError> {
        for segment in split_digit_segments(chunk, self.isolate_digits) {
            for piece in gpt2_split(segment) {
                self.bpe_encode_piece(&piece, out)?;
            }
        }
        Ok(())
    }

    fn bpe_encode_piece(&self, piece: &str, out: &mut Vec<u32>) -> Result<(), TokenizationError> {
        // Map bytes to characters using the GPT-2 byte-to-unicode table.
        let mut symbols: Vec<String> = piece
            .bytes()
            .map(|b| self.byte_to_unicode[b as usize].to_string())
            .collect();

        // Repeatedly merge the adjacent pair with the lowest rank until none remain.
        loop {
            let mut best: Option<(usize, u32)> = None;
            for i in 0..symbols.len().saturating_sub(1) {
                if let Some(&rank) = self.merge_rank.get(&(symbols[i].clone(), symbols[i + 1].clone()))
                    && best.is_none_or(|(_, best_rank)| rank < best_rank)
                {
                    best = Some((i, rank));
                }
            }
            match best {
                Some((i, _)) => {
                    let merged = alloc::format!("{}{}", symbols[i], symbols[i + 1]);
                    symbols.splice(i..=i + 1, [merged]);
                }
                None => break,
            }
        }

        // Convert merged symbols to token IDs.
        for s in symbols {
            let id = self.vocab.get(&s)
                .ok_or_else(|| TokenizationError::VocabNotFound(s.clone()))?;
            out.push(*id);
        }

        Ok(())
    }

    /// Decode token IDs back to bytes.
    pub fn decode(&self, ids: &[u32]) -> Result<Vec<u8>, TokenizationError> {
        let mut bytes = Vec::new();

        for &id in ids {
            let idx = id as usize;
            if idx >= self.id_to_bytes.len() {
                return Err(TokenizationError::DecodingError);
            }
            bytes.extend_from_slice(&self.id_to_bytes[idx]);
        }

        Ok(bytes)
    }

    /// Streaming decoder that reconstructs UTF-8 text from token IDs.
    pub fn decode_streaming(&self) -> StreamingDecoder<'_> {
        StreamingDecoder::new(self)
    }
}

pub struct StreamingDecoder<'a> {
    tokenizer: &'a Tokenizer,
    buffer: Vec<u8>,
    incomplete_char_start: usize,
}

impl<'a> StreamingDecoder<'a> {
    pub fn new(tokenizer: &'a Tokenizer) -> Self {
        Self {
            tokenizer,
            buffer: Vec::new(),
            incomplete_char_start: 0,
        }
    }

    /// Feed a single token and get valid UTF-8 output, holding back incomplete sequences.
    pub fn feed(&mut self, id: u32) -> Result<Vec<u8>, TokenizationError> {
        let idx = id as usize;
        if idx >= self.tokenizer.id_to_bytes.len() {
            return Err(TokenizationError::DecodingError);
        }

        self.buffer.extend_from_slice(&self.tokenizer.id_to_bytes[idx]);

        // Find the longest valid UTF-8 prefix we can emit
        let mut valid_until = 0;
        let mut pos = 0;

        while pos < self.buffer.len() {
            let lead = self.buffer[pos];

            let char_len = if (lead & 0x80) == 0 {
                1
            } else if (lead & 0xE0) == 0xC0 {
                2
            } else if (lead & 0xF0) == 0xE0 {
                3
            } else if (lead & 0xF8) == 0xF0 {
                4
            } else {
                // Invalid UTF-8 byte
                return Err(TokenizationError::InvalidUtf8);
            };

            // Check if we have enough bytes for this character
            if pos + char_len <= self.buffer.len() {
                // Validate continuation bytes
                let mut valid_char = true;
                for i in 1..char_len {
                    if (self.buffer[pos + i] & 0xC0) != 0x80 {
                        valid_char = false;
                        break;
                    }
                }

                if valid_char {
                    valid_until = pos + char_len;
                    pos += char_len;
                } else {
                    return Err(TokenizationError::InvalidUtf8);
                }
            } else {
                // Incomplete character at the end, hold it back
                break;
            }
        }

        let result = self.buffer[..valid_until].to_vec();
        self.buffer = self.buffer[valid_until..].to_vec();

        Ok(result)
    }

    /// Flush any remaining bytes.
    pub fn finish(self) -> Result<Vec<u8>, TokenizationError> {
        if self.buffer.is_empty() {
            Ok(Vec::new())
        } else {
            // Try to parse remaining as UTF-8
            if core::str::from_utf8(&self.buffer).is_ok() {
                Ok(self.buffer)
            } else {
                Err(TokenizationError::InvalidUtf8)
            }
        }
    }
}

// ==================== Digit isolation and GPT-2 regex split ====================

/// Splits on digit/non-digit boundaries, isolating every individual digit into its own piece
/// when `isolate_digits` is set. This runs *before*, and produces hard boundaries for, `gpt2_split`.
fn split_digit_segments(text: &str, isolate_digits: bool) -> Vec<&str> {
    if !isolate_digits {
        return alloc::vec![text];
    }
    let mut pieces = Vec::new();
    let mut start = 0;
    for (idx, ch) in text.char_indices() {
        if ch.is_ascii_digit() {
            if idx > start {
                pieces.push(&text[start..idx]);
            }
            let end = idx + ch.len_utf8();
            pieces.push(&text[idx..end]);
            start = end;
        }
    }
    if start < text.len() {
        pieces.push(&text[start..]);
    }
    pieces
}

fn is_other(c: char) -> bool {
    !c.is_whitespace() && !c.is_alphabetic() && !c.is_numeric()
}

/// One alternative of `'s|'t|'re|'ve|'m|'ll|'d`; distinct second character means no ambiguity.
fn match_contraction(chars: &[char]) -> Option<usize> {
    if chars.first() != Some(&'\'') {
        return None;
    }
    match chars.get(1) {
        Some('s' | 't' | 'm' | 'd') => Some(2),
        Some('r') if chars.get(2) == Some(&'e') => Some(3),
        Some('v') if chars.get(2) == Some(&'e') => Some(3),
        Some('l') if chars.get(2) == Some(&'l') => Some(3),
        _ => None,
    }
}

/// Matches ` ?" + is_cat+` at the start of `chars`: an optional single literal space
/// followed by one or more characters in category `is_cat`. Returns the number of chars consumed.
fn match_category_run(chars: &[char], is_cat: fn(char) -> bool) -> Option<usize> {
    let (offset, first) = if chars.first() == Some(&' ') && chars.get(1).is_some_and(|&c| is_cat(c)) {
        (1, 1)
    } else if chars.first().is_some_and(|&c| is_cat(c)) {
        (0, 1)
    } else {
        return None;
    };
    let mut len = offset + first;
    while chars.get(len).is_some_and(|&c| is_cat(c)) {
        len += 1;
    }
    Some(len)
}

/// The GPT-2 byte-level pre-tokenizer regex
/// (`'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+`),
/// implemented as a scanner rather than a regex engine (DECISIONS.md D27: zero dependencies).
fn gpt2_split(segment: &str) -> Vec<String> {
    let chars: Vec<char> = segment.chars().collect();
    let n = chars.len();
    let mut pieces = Vec::new();
    let mut i = 0;
    while i < n {
        if let Some(len) = match_contraction(&chars[i..]) {
            pieces.push(chars[i..i + len].iter().collect());
            i += len;
            continue;
        }
        if let Some(len) = match_category_run(&chars[i..], char::is_alphabetic) {
            pieces.push(chars[i..i + len].iter().collect());
            i += len;
            continue;
        }
        if let Some(len) = match_category_run(&chars[i..], char::is_numeric) {
            pieces.push(chars[i..i + len].iter().collect());
            i += len;
            continue;
        }
        if let Some(len) = match_category_run(&chars[i..], is_other) {
            pieces.push(chars[i..i + len].iter().collect());
            i += len;
            continue;
        }
        // Only whitespace can remain; run >= 1. The \s+(?!\S) alternative only succeeds
        // when the run reaches the end; otherwise it backs off by one char, leaving that
        // last whitespace char for the next iteration (where it becomes a word's leading space).
        let mut run = 0;
        while i + run < n && chars[i + run].is_whitespace() {
            run += 1;
        }
        let at_end = i + run == n;
        let take = if at_end || run == 1 { run } else { run - 1 };
        pieces.push(chars[i..i + take].iter().collect());
        i += take;
    }
    pieces
}
