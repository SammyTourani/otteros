#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use alloc::collections::BTreeMap;
use core::str;

mod entities;
pub use entities::NAMED_ENTITIES;

mod tokenizer;
pub use tokenizer::{Tokenizer, Token, InitialState};

// Re-export URL types for convenience
pub use otter_http::{Url, UrlError};

/// HTML node
#[derive(Debug, Clone)]
pub enum Node {
    Document { children: Vec<usize> },
    Element { tag: String, attrs: BTreeMap<String, String>, children: Vec<usize>, parent: Option<usize> },
    Text { content: String, parent: Option<usize> },
    Comment { content: String, parent: Option<usize> },
}

/// HTML document tree
pub struct Document {
    nodes: Vec<Node>,
    root: usize,
}

impl Document {
    /// Create a new document
    pub fn new() -> Self {
        let nodes = vec![Node::Document { children: Vec::new() }];
        Document { nodes, root: 0 }
    }

    /// Create an element node
    pub fn create_element(&mut self, tag: &str) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(Node::Element {
            tag: tag.to_string(),
            attrs: BTreeMap::new(),
            children: Vec::new(),
            parent: None,
        });
        idx
    }

    /// Create a text node
    pub fn create_text(&mut self, content: &str) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(Node::Text {
            content: content.to_string(),
            parent: None,
        });
        idx
    }

    /// Append a child to a parent
    pub fn append_child(&mut self, parent: usize, child: usize) {
        if parent < self.nodes.len() && child < self.nodes.len() {
            match &mut self.nodes[parent] {
                Node::Element { children, .. } => {
                    children.push(child);
                }
                Node::Document { children, .. } => {
                    children.push(child);
                }
                _ => {}
            }

            match &mut self.nodes[child] {
                Node::Element { parent: p, .. } | Node::Text { parent: p, .. } => {
                    *p = Some(parent);
                }
                _ => {}
            }
        }
    }

    /// Get the root element
    pub fn root(&self) -> Option<usize> {
        // Find the first html element
        if let Node::Document { children } = &self.nodes[self.root] {
            for &child in children {
                if let Node::Element { tag, .. } = &self.nodes[child]
                    && tag == "html" {
                    return Some(child);
                }
            }
        }
        None
    }

    /// Get all text content of a node and its descendants
    pub fn text_content(&self, node: usize) -> String {
        if node >= self.nodes.len() {
            return String::new();
        }

        match &self.nodes[node] {
            Node::Text { content, .. } => content.clone(),
            Node::Element { children, .. } | Node::Document { children, .. } => {
                let mut result = String::new();
                for &child in children {
                    result.push_str(&self.text_content(child));
                }
                result
            }
            Node::Comment { .. } => String::new(),
        }
    }

    /// Get the document title
    pub fn title(&self) -> String {
        // Find <title> element
        if let Some(root) = self.root()
            && let Some(head) = self.find_element_by_tag(root, "head")
            && let Some(title_elem) = self.find_element_by_tag(head, "title") {
            return self.text_content(title_elem);
        }
        String::new()
    }

    /// Find an element by tag name (first occurrence)
    fn find_element_by_tag(&self, node: usize, tag: &str) -> Option<usize> {
        if node >= self.nodes.len() {
            return None;
        }

        match &self.nodes[node] {
            Node::Element { tag: t, children, .. } => {
                if t == tag {
                    return Some(node);
                }
                for &child in children {
                    if let Some(found) = self.find_element_by_tag(child, tag) {
                        return Some(found);
                    }
                }
                None
            }
            Node::Document { children, .. } => {
                for &child in children {
                    if let Some(found) = self.find_element_by_tag(child, tag) {
                        return Some(found);
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Get all links in the document
    pub fn links(&self) -> Vec<(String, String)> {
        let mut links = Vec::new();
        if let Some(root) = self.root() {
            self.collect_links(root, &mut links);
        }
        links
    }

    fn collect_links(&self, node: usize, links: &mut Vec<(String, String)>) {
        if node >= self.nodes.len() {
            return;
        }

        match &self.nodes[node] {
            Node::Element { tag, attrs, children, .. } => {
                if tag == "a"
                    && let Some(href) = attrs.get("href") {
                    let text = self.text_content(node);
                    links.push((text, href.clone()));
                }
                for &child in children {
                    self.collect_links(child, links);
                }
            }
            Node::Document { children, .. } => {
                for &child in children {
                    self.collect_links(child, links);
                }
            }
            _ => {}
        }
    }

    /// Resolve relative URLs against a base URL
    pub fn resolve_url(&self, base_url: &str, relative: &str) -> Result<String, UrlError> {
        let base = Url::parse(base_url)?;
        let resolved = base.resolve(relative)?;
        Ok(resolved.to_string())
    }

    /// Get the <base> href if present
    pub fn base_href(&self) -> Option<String> {
        if let Some(root) = self.root()
            && let Some(head) = self.find_element_by_tag(root, "head")
            && let Some(base_elem) = self.find_element_by_tag(head, "base")
            && let Node::Element { attrs, .. } = &self.nodes[base_elem] {
            return attrs.get("href").cloned();
        }
        None
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_tokenizer() {
        let mut tokenizer = Tokenizer::new("<div>Hello</div>");
        let tokens = tokenizer.tokenize();
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_document_creation() {
        let mut doc = Document::new();
        let html = doc.create_element("html");
        let body = doc.create_element("body");
        doc.append_child(0, html);
        doc.append_child(html, body);
    }
}
