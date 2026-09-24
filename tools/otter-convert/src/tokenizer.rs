//! A Hugging Face "fast" (byte-level BPE) tokenizer: parses `tokenizer.json`
//! and re-implements its encode path from scratch (brief M7-T1: "a
//! straightforward reference BPE ... this BPE becomes the spec for the
//! runtime's tokenizer"). Matches the specific pipeline SmolLM2/Llama3-style
//! tokenizers use: an optional `Digits` pre-split (every digit isolated into
//! its own piece, a hard boundary later stages can't merge across), then the
//! classic GPT-2 byte-level regex, then BPE merges, all after literal
//! special/added-token splitting.

use std::collections::HashMap;

use otter_json::Value;

/// GPT-2's byte<->"visible" Unicode character mapping (Radford et al.'s
/// `bytes_to_unicode`): printable bytes (`!`..`~`, `\u{a1}`..`\u{ac}`,
/// `\u{ae}`..`\u{ff}`) map to themselves; the rest (controls, space, DEL,
/// and a couple of Latin-1 gaps) map to consecutive code points starting at
/// 256, in ascending byte order. This exists purely so every byte value has
/// a printable stand-in character to build BPE merge strings out of; the
/// `.otm` format reverses it back to raw bytes before storing the vocab
/// (brief M7-T1's ".otm ... vocab as byte strings after GPT-2
/// byte-to-unicode reversal").
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

#[allow(dead_code)]
pub struct AddedToken {
    pub id: u32,
    pub content: String,
}

#[allow(dead_code)]
pub struct Tokenizer {
    /// Byte-level-mapped token string -> id.
    pub vocab: HashMap<String, u32>,
    /// id -> raw bytes (after reversing the byte-level mapping), indexed
    /// contiguously by id. This is what `.otm` actually stores.
    pub id_to_bytes: Vec<Vec<u8>>,
    /// (left, right) mapped-strings -> merge priority (lower merges first).
    pub merge_rank: HashMap<(String, String), u32>,
    /// In the exact order `.otm` should record them; also used (longest
    /// content first) to greedily match literal special-token text.
    pub added_tokens: Vec<AddedToken>,
    /// Whether the pre-tokenizer isolates every digit into its own piece
    /// (SmolLM2/Llama3-style `Sequence[Digits(individual_digits=true), ByteLevel]`).
    isolate_digits: bool,
    byte_to_unicode: [char; 256],
}

/// Parses an already-JSON-parsed `tokenizer.json`. Only the byte-level BPE
/// shape this brief's models use is supported; anything else is a clear
/// error rather than a silent wrong answer.
pub fn parse_tokenizer(root: &Value) -> Result<Tokenizer, String> {
    let model = root.get("model").ok_or("tokenizer.json: missing \"model\"")?;
    let model_type = model.get("type").and_then(Value::as_str).unwrap_or("");
    if model_type != "BPE" {
        return Err(format!("tokenizer.json: unsupported model type \"{model_type}\" (only \"BPE\")"));
    }

    let vocab_obj = model.get("vocab").and_then(Value::as_object).ok_or("tokenizer.json: model.vocab missing")?;
    let mut vocab = HashMap::new();
    let mut max_id: i64 = -1;
    for (tok, id_val) in vocab_obj.iter() {
        let id = id_val.as_u64().ok_or("tokenizer.json: a vocab id is not an integer")? as u32;
        vocab.insert(tok.to_string(), id);
        max_id = max_id.max(i64::from(id));
    }
    if max_id < 0 {
        return Err("tokenizer.json: empty vocab".to_string());
    }
    let byte_to_unicode = byte_to_unicode_table();
    let mut unicode_to_byte = HashMap::with_capacity(256);
    for (b, &c) in byte_to_unicode.iter().enumerate() {
        unicode_to_byte.insert(c, b as u8);
    }
    let mut id_to_bytes = vec![None; (max_id + 1) as usize];
    for (mapped, &id) in vocab.iter() {
        let mut raw = Vec::with_capacity(mapped.chars().count());
        for c in mapped.chars() {
            let b = *unicode_to_byte
                .get(&c)
                .ok_or_else(|| format!("tokenizer.json: vocab entry {mapped:?} has a char outside the byte-level alphabet"))?;
            raw.push(b);
        }
        id_to_bytes[id as usize] = Some(raw);
    }
    let id_to_bytes = id_to_bytes
        .into_iter()
        .enumerate()
        .map(|(id, v)| v.ok_or_else(|| format!("tokenizer.json: vocab id {id} has no entry (ids must be contiguous 0..vocab_size)")))
        .collect::<Result<Vec<_>, _>>()?;

    let merges_arr = model.get("merges").and_then(Value::as_array).ok_or("tokenizer.json: model.merges missing")?;
    let mut merge_rank = HashMap::with_capacity(merges_arr.len());
    for (rank, m) in merges_arr.iter().enumerate() {
        let s = m.as_str().ok_or("tokenizer.json: a merges entry is not a string")?;
        let (l, r) = s.split_once(' ').ok_or_else(|| format!("tokenizer.json: merges entry {s:?} has no separating space"))?;
        merge_rank.insert((l.to_string(), r.to_string()), rank as u32);
    }

    let mut added_tokens = Vec::new();
    if let Some(arr) = root.get("added_tokens").and_then(Value::as_array) {
        for t in arr {
            let id = t.get("id").and_then(Value::as_u64).ok_or("tokenizer.json: an added token is missing \"id\"")? as u32;
            let content = t
                .get("content")
                .and_then(Value::as_str)
                .ok_or("tokenizer.json: an added token is missing \"content\"")?
                .to_string();
            added_tokens.push(AddedToken { id, content });
        }
    }
    // Longest content first: greedy literal matching must prefer a longer
    // added token over a shorter one that happens to be its prefix.
    added_tokens.sort_by_key(|a| std::cmp::Reverse(a.content.len()));

    let isolate_digits = pre_tokenizer_isolates_digits(root.get("pre_tokenizer"));

    Ok(Tokenizer { vocab, id_to_bytes, merge_rank, added_tokens, isolate_digits, byte_to_unicode })
}

fn pre_tokenizer_isolates_digits(pre_tokenizer: Option<&Value>) -> bool {
    fn walk(v: &Value) -> bool {
        if v.get("type").and_then(Value::as_str) == Some("Digits") {
            return v.get("individual_digits").and_then(Value::as_bool).unwrap_or(false);
        }
        v.get("pretokenizers").and_then(Value::as_array).is_some_and(|subs| subs.iter().any(walk))
    }
    pre_tokenizer.is_some_and(walk)
}

/// Splits on digit/non-digit boundaries, isolating every individual digit
/// into its own piece when `isolate_digits` is set. This runs *before*, and
/// produces hard boundaries for, [`gpt2_split`] -- e.g. a trailing space
/// before a number ends up in the non-digit piece, not attached to the
/// digit, because by the time the space-attaching regex logic runs, the
/// digit is already in a separate piece it can't see across.
#[allow(dead_code)]
fn split_digit_segments(text: &str, isolate_digits: bool) -> Vec<&str> {
    if !isolate_digits {
        return vec![text];
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

#[allow(dead_code)]
fn is_other(c: char) -> bool {
    !c.is_whitespace() && !c.is_alphabetic() && !c.is_numeric()
}

/// One alternative of `'s|'t|'re|'ve|'m|'ll|'d`; every pattern has a
/// distinct second character so there's no ambiguity to resolve by trying
/// longest-first.
#[allow(dead_code)]
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

/// Matches `" ?" + is_cat+` at the start of `chars`: an optional single
/// literal space (only `' '`, not any Unicode whitespace, matching the
/// regex's literal `" ?"`) followed by one or more characters in category
/// `is_cat`. Returns the number of `chars` consumed.
#[allow(dead_code)]
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
/// implemented directly as a scanner rather than run through a general regex
/// engine (DECISIONS.md D27: zero dependencies). `\p{L}`/`\p{N}` are
/// approximated with `char::is_alphabetic`/`is_numeric` (built into `core`,
/// not a crate); every prompt this brief's tests use stays within territory
/// where that distinction from a strict Unicode-category regex engine can't
/// matter (letters, digits, ASCII/Latin-1 punctuation, common whitespace,
/// and symbols/emoji that are unambiguously in neither category).
#[allow(dead_code)]
pub fn gpt2_split(segment: &str) -> Vec<String> {
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
        // Only whitespace can remain (letter/digit/other all failed), so
        // `run >= 1`. The `\s+(?!\S)` alternative only succeeds when the run
        // reaches the end of this segment; otherwise it backs off by one
        // char, leaving that last whitespace char for the next iteration
        // (where, if it's a literal space followed by a word, it becomes
        // that word's leading-space prefix via `match_category_run` above).
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

/// Byte-level-maps `piece`'s UTF-8 bytes to single-char symbol strings, then
/// repeatedly merges the adjacent pair with the lowest rank until none of
/// the remaining adjacent pairs has one -- the standard BPE encode loop.
#[allow(dead_code)]
fn bpe_encode_piece(tok: &Tokenizer, piece: &str, out: &mut Vec<u32>) -> Result<(), String> {
    let mut symbols: Vec<String> = piece.bytes().map(|b| tok.byte_to_unicode[b as usize].to_string()).collect();
    loop {
        let mut best: Option<(usize, u32)> = None;
        for i in 0..symbols.len().saturating_sub(1) {
            if let Some(&rank) = tok.merge_rank.get(&(symbols[i].clone(), symbols[i + 1].clone())) {
                if best.is_none_or(|(_, best_rank)| rank < best_rank) {
                    best = Some((i, rank));
                }
            }
        }
        match best {
            Some((i, _)) => {
                let merged = symbols[i].clone() + &symbols[i + 1];
                symbols.splice(i..=i + 1, [merged]);
            }
            None => break,
        }
    }
    for s in &symbols {
        let id = tok.vocab.get(s).ok_or_else(|| format!("BPE symbol {s:?} (from piece {piece:?}) is not in the vocab"))?;
        out.push(*id);
    }
    Ok(())
}

#[allow(dead_code)]
fn encode_normal_chunk(tok: &Tokenizer, chunk: &str, out: &mut Vec<u32>) -> Result<(), String> {
    for segment in split_digit_segments(chunk, tok.isolate_digits) {
        for piece in gpt2_split(segment) {
            bpe_encode_piece(tok, &piece, out)?;
        }
    }
    Ok(())
}

/// Encodes `text` with no automatic BOS/EOS handling, matching Hugging
/// Face's `tokenizer.encode(text, add_special_tokens=False)`: literal
/// added-token text (e.g. `<|im_start|>`) is matched and emitted directly
/// wherever it occurs, and everything between such matches goes through the
/// normal digit-split + GPT-2 regex + BPE pipeline above.
#[allow(dead_code)]
pub fn encode(tok: &Tokenizer, text: &str) -> Result<Vec<u32>, String> {
    let mut ids = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(at) = tok.added_tokens.iter().find(|at| rest.starts_with(at.content.as_str())) {
            ids.push(at.id);
            rest = &rest[at.content.len()..];
            continue;
        }
        let mut split_at = rest.len();
        for (i, _) in rest.char_indices().skip(1) {
            if tok.added_tokens.iter().any(|at| rest[i..].starts_with(at.content.as_str())) {
                split_at = i;
                break;
            }
        }
        encode_normal_chunk(tok, &rest[..split_at], &mut ids)?;
        rest = &rest[split_at..];
    }
    Ok(ids)
}
