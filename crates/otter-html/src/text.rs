//! Format blocks to wrapped text with link references

use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use crate::{Block, Span};

/// Convert blocks to text with word wrapping and link references
pub fn to_text(blocks: &[Block], width: usize) -> String {
    let mut out = String::new();
    let mut references: Vec<String> = Vec::new();
    let mut reference_map: BTreeMap<String, usize> = BTreeMap::new();

    for (i, block) in blocks.iter().enumerate() {
        // Add blank line before block i if i > 0 and not both prev and curr are ListItem
        if i > 0 && !(matches!(blocks[i-1], Block::ListItem{..}) && matches!(block, Block::ListItem{..})) {
            out.push('\n');
        }

        // Format the block
        match block {
            Block::Heading { level, spans } => {
                for _ in 0..*level {
                    out.push('#');
                }
                out.push(' ');

                let text = spans_to_text(spans, &mut references, &mut reference_map);
                out.push_str(&text);
                out.push('\n');
            }
            Block::Paragraph { spans } => {
                let text = spans_to_text(spans, &mut references, &mut reference_map);
                wrap_text(&mut out, &text, 0, "", width);
                out.push('\n');
            }
            Block::ListItem { depth, marker, spans } => {
                let indent = (*depth as usize) * 2;
                let prefix = alloc::format!("{}{} ", " ".repeat(indent), marker);
                let continuation_indent = indent + marker.chars().count() + 1;

                let text = spans_to_text(spans, &mut references, &mut reference_map);
                wrap_text(&mut out, &text, continuation_indent, &prefix, width);
                out.push('\n');
            }
            Block::Preformatted { text } => {
                out.push_str(text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
            }
            Block::Rule => {
                for _ in 0..width {
                    out.push('-');
                }
                out.push('\n');
            }
        }
    }

    // Add references
    if !references.is_empty() {
        out.push('\n');
        out.push_str("References:\n");
        for (i, url) in references.iter().enumerate() {
            out.push_str(&alloc::format!("[{}] {}\n", i + 1, url));
        }
    } else if !out.ends_with('\n') {
        out.push('\n');
    }

    out
}

fn spans_to_text(spans: &[Span], references: &mut Vec<String>, reference_map: &mut BTreeMap<String, usize>) -> String {
    let mut text = String::new();

    for span in spans {
        text.push_str(&span.text);

        if let Some(link) = &span.link {
            let ref_num = if let Some(&num) = reference_map.get(link) {
                num
            } else {
                let num = references.len() + 1;
                references.push(link.clone());
                reference_map.insert(link.clone(), num);
                num
            };
            text.push_str(&alloc::format!("[{}]", ref_num));
        }
    }

    text
}

fn wrap_text(out: &mut String, text: &str, continuation_indent: usize, prefix: &str, width: usize) {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut first_line_of_block = true;

    for line_text in lines {
        if !first_line_of_block {
            out.push('\n');
        }

        let words: Vec<&str> = line_text.split_whitespace().collect();
        if words.is_empty() {
            first_line_of_block = false;
            continue;
        }

        let mut output_line = String::new();
        let prefix_len = if first_line_of_block { prefix.chars().count() } else { 0 };

        for word in words {
            let word_len = word.chars().count();

            if output_line.is_empty() {
                if first_line_of_block {
                    out.push_str(prefix);
                } else {
                    out.push_str(&" ".repeat(continuation_indent));
                }
                output_line.push_str(word);
            } else {
                let indent = if first_line_of_block { prefix_len } else { continuation_indent };
                let current_len = indent + output_line.chars().count();

                if current_len + 1 + word_len <= width {
                    output_line.push(' ');
                    output_line.push_str(word);
                } else {
                    out.push_str(&output_line);
                    out.push('\n');
                    out.push_str(&" ".repeat(continuation_indent));
                    output_line.clear();
                    output_line.push_str(word);
                    first_line_of_block = false;
                }
            }
        }

        if !output_line.is_empty() {
            out.push_str(&output_line);
        }
        first_line_of_block = false;
    }
}
