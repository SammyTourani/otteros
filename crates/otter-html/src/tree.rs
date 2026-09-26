//! HTML5-style tree builder with forgiving error handling

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::collections::BTreeMap;
use crate::tokenizer::{Tokenizer, Token};
use crate::Document;

/// A styled text span
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// The text content
    pub text: String,
    /// Optional link URL (None for non-links)
    pub link: Option<String>,
    /// Bold styling
    pub bold: bool,
    /// Italic styling
    pub italic: bool,
    /// Code styling
    pub code: bool,
}

/// A rendered block element
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Heading (h1-h6)
    Heading { level: u8, spans: Vec<Span> },
    /// Paragraph
    Paragraph { spans: Vec<Span> },
    /// List item
    ListItem { depth: u8, marker: String, spans: Vec<Span> },
    /// Preformatted text
    Preformatted { text: String },
    /// Horizontal rule
    Rule,
}

/// Void elements that never have children
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input",
    "link", "meta", "source", "track", "wbr",
];

/// Block-level elements that close `<p>`
const BLOCK_ELEMENTS: &[&str] = &[
    "address", "article", "aside", "blockquote", "div", "dl",
    "fieldset", "figure", "footer", "form", "h1", "h2", "h3",
    "h4", "h5", "h6", "header", "hr", "main", "nav", "ol", "p",
    "pre", "section", "table", "ul",
];

/// Elements that contain only text (RCDATA/RAWTEXT)
const TEXT_ELEMENTS: &[&str] = &["title"];
const RAWTEXT_ELEMENTS: &[&str] = &["style", "xmp", "iframe", "noembed", "noframes"];
const SCRIPT_ELEMENTS: &[&str] = &["script"];

/// Elements that should not be rendered
const HIDDEN_ELEMENTS: &[&str] = &[
    "head", "script", "style", "template", "svg", "math", "noscript",
    "select", "textarea", "button", "input",
];

fn is_void_element(tag: &str) -> bool {
    VOID_ELEMENTS.contains(&tag.to_lowercase().as_str())
}

fn is_block_element(tag: &str) -> bool {
    BLOCK_ELEMENTS.contains(&tag.to_lowercase().as_str())
}

fn is_text_element(tag: &str) -> bool {
    TEXT_ELEMENTS.contains(&tag.to_lowercase().as_str())
}

fn is_rawtext_element(tag: &str) -> bool {
    RAWTEXT_ELEMENTS.contains(&tag.to_lowercase().as_str())
}

fn is_script_element(tag: &str) -> bool {
    SCRIPT_ELEMENTS.contains(&tag.to_lowercase().as_str())
}

fn is_hidden_element(tag: &str) -> bool {
    HIDDEN_ELEMENTS.contains(&tag.to_lowercase().as_str())
}

/// Parse HTML into a Document
pub fn parse(html: &str) -> Document {
    let mut builder = TreeBuilder::new();
    builder.build(html);
    builder.document
}

struct OpenElement {
    tag: String,
    node_idx: usize,
}

struct TreeBuilder {
    document: Document,
    stack: Vec<OpenElement>,
    last_start_tag: Option<String>,
    html: String,
}

impl TreeBuilder {
    fn new() -> Self {
        let mut doc = Document::new();
        let html = doc.create_element("html");
        doc.append_child(0, html);
        TreeBuilder {
            document: doc,
            stack: vec![OpenElement { tag: "html".to_string(), node_idx: html }],
            last_start_tag: None,
            html: String::new(),
        }
    }

    fn build(&mut self, html: &str) {
        self.html = html.to_string();
        let mut tokenizer = Tokenizer::new(html);
        let tokens = tokenizer.tokenize();

        let mut in_head = true;
        let mut body_created = false;
        let mut accumulated_text = String::new();
        let mut i = 0;

        while i < tokens.len() {
            let token = tokens[i].clone();

            // Flush accumulated text before processing non-text tokens
            if !matches!(token, Token::Character(_)) && !accumulated_text.is_empty() {
                self.add_text(&accumulated_text);
                accumulated_text.clear();
            }
            match token {
                Token::StartTag { name, attrs, self_closing } => {
                    let lowercase_name = name.to_lowercase();

                    // Handle text elements - collect content until closing tag
                    if !self_closing && (is_text_element(&lowercase_name) || is_rawtext_element(&lowercase_name) || is_script_element(&lowercase_name)) {
                        // Ensure head exists for head-related elements
                        if in_head && (is_text_element(&lowercase_name) || lowercase_name == "base" || lowercase_name == "meta" || lowercase_name == "link" || lowercase_name == "style") {
                            self.ensure_head();
                        }

                        self.insert_element(&lowercase_name, attrs.clone());
                        let elem_idx = self.stack.last().unwrap().node_idx;

                        // Extract raw content from original HTML string
                        let closing_tag_str = alloc::format!("</{}>", lowercase_name);
                        let closing_tag_pos = self.html.find(&closing_tag_str);

                        if let Some(close_pos) = closing_tag_pos {
                            // Find opening tag in HTML
                            let opening_search = alloc::format!("<{}", lowercase_name);
                            if let Some(open_pos) = self.html.find(&opening_search) {
                                // Find end of opening tag
                                if let Some(tag_end_offset) = self.html[open_pos..].find('>') {
                                    let content_start = open_pos + tag_end_offset + 1;
                                    if content_start <= close_pos {
                                        let mut raw_content = self.html[content_start..close_pos].to_string();
                                        if !raw_content.is_empty() {
                                            // Decode common HTML entities
                                            raw_content = raw_content.replace("&lt;", "<");
                                            raw_content = raw_content.replace("&gt;", ">");
                                            raw_content = raw_content.replace("&amp;", "&");
                                            raw_content = raw_content.replace("&quot;", "\"");
                                            raw_content = raw_content.replace("&apos;", "'");

                                            let text_idx = self.document.create_text(&raw_content);
                                            self.document.append_child(elem_idx, text_idx);
                                        }
                                        // Remove this element from self.html
                                        let end_pos = close_pos + closing_tag_str.len();
                                        self.html = self.html[..open_pos].to_string() + &self.html[end_pos..];
                                    }
                                }
                            }
                        }

                        // Skip tokens until matching end tag
                        let mut depth = 1;
                        i += 1;
                        while i < tokens.len() && depth > 0 {
                            match &tokens[i] {
                                Token::StartTag { name: inner_name, .. } if inner_name.to_lowercase() == lowercase_name => {
                                    depth += 1;
                                }
                                Token::EndTag { name: inner_name } if inner_name.to_lowercase() == lowercase_name => {
                                    depth -= 1;
                                }
                                _ => {}
                            }
                            i += 1;
                        }
                        i -= 1;
                        continue;
                    }

                    // Switch from head to body on body content
                    if in_head && !is_text_element(&lowercase_name) && lowercase_name != "base"
                        && lowercase_name != "meta" && lowercase_name != "link"
                        && lowercase_name != "style" && !body_created {
                        self.ensure_body();
                        body_created = true;
                        in_head = false;
                    }

                    // Handle void elements
                    if is_void_element(&lowercase_name) {
                        self.insert_element(&lowercase_name, attrs);
                        i += 1;
                        continue;
                    }

                    // Close <p> if block element starts
                    if is_block_element(&lowercase_name) && lowercase_name != "p" {
                        self.close_p();
                    }

                    // Close <li> if another <li> starts (same list scope)
                    if lowercase_name == "li" {
                        self.close_li();
                    }

                    // Close <dt>/<dd> if another one starts
                    if lowercase_name == "dt" || lowercase_name == "dd" {
                        self.close_dt_dd();
                    }

                    self.insert_element(&lowercase_name, attrs);
                    self.last_start_tag = Some(lowercase_name);
                }
                Token::EndTag { name } => {
                    let lowercase_name = name.to_lowercase();

                    // Close up to the matching element
                    self.close_to(&lowercase_name);
                }
                Token::Character(ch) => {
                    accumulated_text.push(ch);
                }
                Token::Comment(_) => {
                    // Ignore comments for rendering
                }
                Token::Doctype { .. } => {
                    // Ignore doctype
                }
                Token::Eof => {
                    break;
                }
            }
            i += 1;
        }

        // Flush any remaining accumulated text
        if !accumulated_text.is_empty() {
            self.add_text(&accumulated_text);
        }

        // Ensure body exists
        if !body_created {
            self.ensure_body();
        }
    }

    fn ensure_head(&mut self) {
        let html_idx = self.stack[0].node_idx;
        let has_head = self.find_child_element(html_idx, "head").is_some();

        if !has_head {
            let head = self.document.create_element("head");
            self.document.append_child(html_idx, head);
            // Update the stack to have head as the current open element
            if self.stack.len() == 1 {
                self.stack.push(OpenElement { tag: "head".to_string(), node_idx: head });
            }
        } else if self.stack.len() == 1 || (self.stack.len() > 1 && self.stack[1].tag != "head") {
            // head exists but not on stack, add it
            if let Some(head_idx) = self.find_child_element(html_idx, "head") {
                self.stack.push(OpenElement { tag: "head".to_string(), node_idx: head_idx });
            }
        }
    }

    fn ensure_body(&mut self) {
        // Find if body already exists
        let html_idx = self.stack[0].node_idx;
        let has_body = self.find_child_element(html_idx, "body").is_some();

        if !has_body {
            let body = self.document.create_element("body");
            self.document.append_child(html_idx, body);
            self.stack.push(OpenElement { tag: "body".to_string(), node_idx: body });
        }
    }

    fn find_child_element(&self, parent_idx: usize, tag: &str) -> Option<usize> {
        use crate::Node;
        if parent_idx >= self.document.nodes.len() {
            return None;
        }
        match &self.document.nodes[parent_idx] {
            Node::Element { children, .. } => {
                for &child_idx in children {
                    if let Node::Element { tag: t, .. } = &self.document.nodes[child_idx]
                        && t == tag {
                        return Some(child_idx);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn insert_element(&mut self, tag: &str, attrs: BTreeMap<String, String>) {
        let idx = self.document.create_element(tag);

        // Set attributes
        use crate::Node;
        if let Node::Element { attrs: elem_attrs, .. } = &mut self.document.nodes[idx] {
            *elem_attrs = attrs;
        }

        // Find the current parent (last open element)
        if let Some(parent) = self.stack.last() {
            self.document.append_child(parent.node_idx, idx);
        }

        // Push to stack (except void elements)
        if !is_void_element(tag) {
            self.stack.push(OpenElement { tag: tag.to_string(), node_idx: idx });
        }
    }

    fn add_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        if let Some(parent) = self.stack.last() {
            let idx = self.document.create_text(text);
            self.document.append_child(parent.node_idx, idx);
        }
    }

    fn close_p(&mut self) {
        // Close <p> if it's open
        for i in (0..self.stack.len()).rev() {
            if self.stack[i].tag == "p" {
                self.stack.remove(i);
                break;
            }
        }
    }

    fn close_li(&mut self) {
        // Close <li> if it's open
        for i in (0..self.stack.len()).rev() {
            if self.stack[i].tag == "li" {
                self.stack.remove(i);
                break;
            }
        }
    }

    fn close_dt_dd(&mut self) {
        // Close <dt> or <dd> if either is open
        for i in (0..self.stack.len()).rev() {
            if self.stack[i].tag == "dt" || self.stack[i].tag == "dd" {
                self.stack.remove(i);
                break;
            }
        }
    }

    fn close_to(&mut self, tag: &str) {
        let tag_lower = tag.to_lowercase();

        // Find the matching open element
        for i in (0..self.stack.len()).rev() {
            if self.stack[i].tag == tag_lower {
                // Close from top to this element (inclusive)
                self.stack.truncate(i);
                return;
            }
        }
        // If no match found, do nothing (forgiving)
    }
}
