//! Render Document tree to blocks with styled spans

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use crate::{Document, Node, Span, Block, Url};

/// Render a document to blocks
pub fn render(doc: &Document, base_url: &str) -> Vec<Block> {
    let mut renderer = Renderer::new(base_url.to_string());

    // Extract base href from document
    if let Some(base_href) = doc.base_href() {
        renderer.base_href = Some(base_href);
    }

    if let Some(root) = doc.root() {
        renderer.render_node(doc, root);
    }

    renderer.finalize()
}

struct Renderer {
    base_url: String,
    base_href: Option<String>,
    blocks: Vec<Block>,
    current_spans: Vec<Span>,
    current_inline_style: InlineStyle,
    list_stack: Vec<ListInfo>,
}

#[derive(Clone)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    code: bool,
    link: Option<String>,
}

#[derive(Clone)]
struct ListInfo {
    tag: String,
    depth: u8,
    start: u32,
    counter: u32,
}

impl Renderer {
    fn new(base_url: String) -> Self {
        Renderer {
            base_url,
            base_href: None,
            blocks: Vec::new(),
            current_spans: Vec::new(),
            current_inline_style: InlineStyle {
                bold: false,
                italic: false,
                code: false,
                link: None,
            },
            list_stack: Vec::new(),
        }
    }

    fn resolve_url(&self, href: &str) -> Option<String> {
        // Skip javascript: and mailto: links
        if href.starts_with("javascript:") || href.starts_with("mailto:") {
            return None;
        }

        let base = if let Some(ref base_href) = self.base_href {
            base_href.clone()
        } else {
            self.base_url.clone()
        };

        match Url::parse(&base) {
            Ok(base_url) => match base_url.resolve(href) {
                Ok(resolved) => Some(resolved.to_string()),
                Err(_) => None,
            },
            Err(_) => None,
        }
    }

    fn render_node(&mut self, doc: &Document, node_idx: usize) {
        if node_idx >= doc.nodes.len() {
            return;
        }

        match &doc.nodes[node_idx] {
            Node::Document { children } => {
                for &child in children {
                    self.render_node(doc, child);
                }
            }
            Node::Element { tag, attrs, children, .. } => {
                let tag_lower = tag.to_lowercase();

                // Handle base href
                if tag_lower == "base" {
                    if let Some(href) = attrs.get("href") {
                        self.base_href = Some(href.clone());
                    }
                    return;
                }

                // Handle hidden elements
                if is_hidden(&tag_lower) {
                    return;
                }

                // Handle heading
                if tag_lower.starts_with('h') && tag_lower.len() == 2
                    && let Some(level_char) = tag_lower.chars().nth(1)
                    && let Some(level) = level_char.to_digit(10) {
                    self.flush_current_block();
                    let mut heading_spans = Vec::new();
                    self.render_inline_children(doc, children, &mut heading_spans);
                    if !heading_spans.is_empty() {
                        self.blocks.push(Block::Heading {
                            level: level as u8,
                            spans: heading_spans,
                        });
                    }
                    return;
                }

                // Handle paragraph
                if tag_lower == "p" || tag_lower == "div" || tag_lower == "section"
                    || tag_lower == "article" || tag_lower == "header" || tag_lower == "footer"
                    || tag_lower == "main" || tag_lower == "nav" || tag_lower == "aside"
                    || tag_lower == "figure" || tag_lower == "figcaption" || tag_lower == "address"
                    || tag_lower == "blockquote" || tag_lower == "dt" || tag_lower == "dd" {
                    self.flush_current_block();
                    let mut para_spans = Vec::new();
                    self.render_inline_children(doc, children, &mut para_spans);
                    if !para_spans.is_empty() {
                        self.blocks.push(Block::Paragraph { spans: para_spans });
                    }
                    return;
                }

                // Handle list items
                if tag_lower == "li" {
                    self.flush_current_block();

                    // Separate inline and block children
                    let mut inline_children = Vec::new();
                    let mut block_children = Vec::new();

                    for &child_idx in children {
                        if child_idx < doc.nodes.len() {
                            match &doc.nodes[child_idx] {
                                Node::Element { tag: child_tag, .. } => {
                                    let child_tag_lower = child_tag.to_lowercase();
                                    if is_block_element(&child_tag_lower) {
                                        block_children.push(child_idx);
                                    } else {
                                        inline_children.push(child_idx);
                                    }
                                }
                                Node::Text { .. } => {
                                    inline_children.push(child_idx);
                                }
                                _ => {}
                            }
                        }
                    }

                    // Render inline content as spans for the list item
                    let mut item_spans = Vec::new();
                    for &child_idx in &inline_children {
                        self.render_node(doc, child_idx);
                    }
                    let inline_spans = core::mem::take(&mut self.current_spans);
                    item_spans.extend(inline_spans);

                    if !item_spans.is_empty() || !block_children.is_empty() {
                        let marker = if let Some(list) = self.list_stack.last_mut() {
                            let mark = if list.tag == "ul" {
                                "•".to_string()
                            } else {
                                format!("{}.", list.counter)
                            };
                            list.counter += 1;
                            mark
                        } else {
                            "•".to_string()
                        };
                        self.blocks.push(Block::ListItem {
                            depth: (self.list_stack.len().saturating_sub(1)) as u8,
                            marker,
                            spans: item_spans,
                        });
                    }

                    // Render block children (nested lists) separately
                    for &child_idx in &block_children {
                        self.render_node(doc, child_idx);
                    }
                    return;
                }

                // Handle lists
                if tag_lower == "ul" || tag_lower == "ol" {
                    self.flush_current_block();
                    let start = if tag_lower == "ol" {
                        attrs.get("start")
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(1)
                    } else {
                        1
                    };
                    self.list_stack.push(ListInfo {
                        tag: tag_lower.clone(),
                        depth: self.list_stack.len() as u8,
                        start,
                        counter: start,
                    });
                    for &child in children {
                        self.render_node(doc, child);
                    }
                    self.list_stack.pop();
                    return;
                }

                // Handle pre
                if tag_lower == "pre" {
                    self.flush_current_block();
                    let text = self.extract_pre_text(doc, children);
                    if !text.is_empty() {
                        self.blocks.push(Block::Preformatted { text });
                    }
                    return;
                }

                // Handle hr
                if tag_lower == "hr" {
                    self.flush_current_block();
                    self.blocks.push(Block::Rule);
                    return;
                }

                // Handle table
                if tag_lower == "table" {
                    self.flush_current_block();
                    self.render_table(doc, children);
                    return;
                }

                // Handle br - use a placeholder that won't be collapsed
                if tag_lower == "br" {
                    self.add_text_to_spans("\u{FFFE}");  // Use replacement character as placeholder for newline
                    return;
                }

                // Handle img
                if tag_lower == "img" {
                    if let Some(alt) = attrs.get("alt")
                        && !alt.is_empty() {
                        self.add_text_to_spans(&format!("[{}]", alt));
                    }
                    return;
                }

                // Handle link
                if tag_lower == "a" {
                    let saved_link = self.current_inline_style.link.clone();
                    if let Some(href) = attrs.get("href") {
                        self.current_inline_style.link = self.resolve_url(href);
                    }
                    for &child in children {
                        self.render_node(doc, child);
                    }
                    self.current_inline_style.link = saved_link;
                    return;
                }

                // Handle bold/strong
                if tag_lower == "b" || tag_lower == "strong" {
                    let saved = self.current_inline_style.bold;
                    self.current_inline_style.bold = true;
                    for &child in children {
                        self.render_node(doc, child);
                    }
                    self.current_inline_style.bold = saved;
                    return;
                }

                // Handle italic/em
                if tag_lower == "i" || tag_lower == "em" {
                    let saved = self.current_inline_style.italic;
                    self.current_inline_style.italic = true;
                    for &child in children {
                        self.render_node(doc, child);
                    }
                    self.current_inline_style.italic = saved;
                    return;
                }

                // Handle code
                if tag_lower == "code" || tag_lower == "kbd" || tag_lower == "samp" || tag_lower == "tt" {
                    let saved = self.current_inline_style.code;
                    self.current_inline_style.code = true;
                    for &child in children {
                        self.render_node(doc, child);
                    }
                    self.current_inline_style.code = saved;
                    return;
                }

                // Default: render children
                for &child in children {
                    self.render_node(doc, child);
                }
            }
            Node::Text { content, .. } => {
                // Don't collapse here; let render_inline_children handle it
                if !content.is_empty() {
                    self.add_text_to_spans(content);
                }
            }
            Node::Comment { .. } => {
                // Ignore comments
            }
        }
    }

    fn render_inline_children(
        &mut self,
        doc: &Document,
        children: &[usize],
        output: &mut Vec<Span>,
    ) {
        let saved_spans = core::mem::take(&mut self.current_spans);
        let mut last_char_whitespace = true; // Start as true for block beginning

        for &child in children {
            self.render_node(doc, child);
        }

        let inline_spans = core::mem::take(&mut self.current_spans);
        self.current_spans = saved_spans;

        // Apply whitespace collapsing per rule 1: collapse runs to single space,
        // never trim at span boundaries
        for span in inline_spans {
            // Collapse whitespace runs within this span
            let mut collapsed = String::new();
            let mut in_whitespace = false;

            for ch in span.text.chars() {
                if ch == '\u{FFFE}' {
                    // Preserve <br> placeholders (will convert back to newlines later)
                    collapsed.push(ch);
                    in_whitespace = false;
                    last_char_whitespace = false;
                } else if ch.is_ascii_whitespace() {
                    if !in_whitespace {
                        // Only emit space if last char wasn't whitespace
                        if !last_char_whitespace {
                            collapsed.push(' ');
                        }
                        in_whitespace = true;
                        last_char_whitespace = true;
                    }
                } else {
                    collapsed.push(ch);
                    in_whitespace = false;
                    last_char_whitespace = false;
                }
            }

            if !collapsed.is_empty() {
                if let Some(last) = output.last_mut()
                    && last.bold == span.bold && last.italic == span.italic
                    && last.code == span.code && last.link == span.link {
                    last.text.push_str(&collapsed);
                } else {
                    output.push(Span { text: collapsed, ..span });
                }
            }
        }

        // Trim trailing whitespace from last span only at block edges
        if let Some(last) = output.last_mut()
            && !last.text.is_empty() {
            // Count trailing spaces
            let trim_len = last.text.len() - last.text.trim_end().len();
            if trim_len > 0 && trim_len == last.text.len() {
                // Entire span is whitespace
                last.text.clear();
                output.pop();
            } else if trim_len > 0 {
                last.text.truncate(last.text.len() - trim_len);
            }
        }

        // Remove empty spans
        output.retain(|s| !s.text.is_empty());

        // Convert <br> placeholders back to newlines
        for span in output.iter_mut() {
            span.text = span.text.replace('\u{FFFE}', "\n");
        }
    }

    fn render_table(&mut self, doc: &Document, children: &[usize]) {
        for &child in children {
            if child >= doc.nodes.len() {
                continue;
            }
            if let Node::Element { tag, children: row_children, .. } = &doc.nodes[child] {
                if tag == "tr" {
                    let mut row_spans = Vec::new();
                    let mut cell_texts = Vec::new();

                    for &cell in row_children {
                        if cell >= doc.nodes.len() {
                            continue;
                        }
                        if let Node::Element { tag: cell_tag, children: cell_children, .. } = &doc.nodes[cell]
                            && (cell_tag == "td" || cell_tag == "th") {
                            let mut cell_spans = Vec::new();
                            self.render_inline_children(doc, cell_children, &mut cell_spans);
                            let cell_text = cell_spans.iter()
                                .map(|s| s.text.clone())
                                .collect::<Vec<_>>()
                                .join("");
                            if !cell_text.is_empty() {
                                cell_texts.push(cell_text);
                            }
                        }
                    }

                    if !cell_texts.is_empty() {
                        row_spans.push(Span {
                            text: cell_texts.join(" | "),
                            link: None,
                            bold: false,
                            italic: false,
                            code: false,
                        });
                        if !row_spans.is_empty() {
                            self.blocks.push(Block::Paragraph { spans: row_spans });
                        }
                    }
                } else {
                    self.render_table(doc, row_children);
                }
            }
        }
    }

    fn extract_pre_text(&self, doc: &Document, children: &[usize]) -> String {
        let mut text = String::new();
        let mut first = true;

        for &child in children {
            if child >= doc.nodes.len() {
                continue;
            }
            match &doc.nodes[child] {
                Node::Text { content, .. } => {
                    if first && content.starts_with('\n') {
                        text.push_str(&content[1..]);
                        first = false;
                    } else {
                        text.push_str(content);
                    }
                }
                Node::Element { tag, .. } if tag == "br" => {
                    text.push('\n');
                }
                _ => {}
            }
        }

        text
    }

    fn add_text_to_spans(&mut self, text: &str) {
        if let Some(last) = self.current_spans.last_mut()
            && last.bold == self.current_inline_style.bold
            && last.italic == self.current_inline_style.italic
            && last.code == self.current_inline_style.code
            && last.link == self.current_inline_style.link {
            last.text.push_str(text);
            return;
        }

        self.current_spans.push(Span {
            text: text.to_string(),
            link: self.current_inline_style.link.clone(),
            bold: self.current_inline_style.bold,
            italic: self.current_inline_style.italic,
            code: self.current_inline_style.code,
        });
    }

    fn flush_current_block(&mut self) {
        if !self.current_spans.is_empty() {
            // Check if there's actual content (not just whitespace)
            let has_content = self.current_spans.iter().any(|s| !s.text.trim().is_empty());
            if has_content {
                // Add text that was outside any block
                self.blocks.push(Block::Paragraph {
                    spans: core::mem::take(&mut self.current_spans),
                });
            } else {
                // Discard whitespace-only spans
                self.current_spans.clear();
            }
        }
    }

    fn finalize(mut self) -> Vec<Block> {
        self.flush_current_block();
        self.blocks
    }
}

fn is_hidden(tag: &str) -> bool {
    matches!(
        tag,
        "head" | "script" | "style" | "template" | "svg" | "math"
            | "noscript" | "select" | "textarea" | "button" | "input"
    )
}

fn is_block_element(tag: &str) -> bool {
    matches!(
        tag,
        "address" | "article" | "aside" | "blockquote" | "div" | "dl"
            | "fieldset" | "figure" | "footer" | "form" | "h1" | "h2" | "h3"
            | "h4" | "h5" | "h6" | "header" | "hr" | "main" | "nav" | "ol" | "p"
            | "pre" | "section" | "table" | "ul"
    )
}
