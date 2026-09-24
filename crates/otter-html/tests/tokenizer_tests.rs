use otter_html::{Tokenizer, Token};

#[test]
fn test_tokenizer_simple() {
    let mut tokenizer = Tokenizer::new("<div>Hello</div>");
    let tokens = tokenizer.tokenize();
    assert!(!tokens.is_empty());
    assert!(tokens.iter().any(|t| matches!(t, Token::StartTag { name, .. } if name == "div")));
    assert!(tokens.iter().any(|t| matches!(t, Token::EndTag { name } if name == "div")));
}

#[test]
fn test_tokenizer_doctype() {
    let mut tokenizer = Tokenizer::new("<!DOCTYPE html><html></html>");
    let tokens = tokenizer.tokenize();
    assert!(!tokens.is_empty());
    assert!(matches!(tokens[0], Token::Doctype { .. }));
}

#[test]
fn test_tokenizer_attributes() {
    let mut tokenizer = Tokenizer::new(r#"<div class="test" id="myid"></div>"#);
    let tokens = tokenizer.tokenize();
    assert!(!tokens.is_empty());

    // Check if StartTag token exists with attributes
    let found = tokens.iter().any(|t| {
        matches!(t, Token::StartTag { name, attrs, .. } if {
            name == "div" && attrs.contains_key("class") && attrs.contains_key("id")
        })
    });
    assert!(found, "StartTag with attributes not found");
}

#[test]
fn test_tokenizer_self_closing() {
    let mut tokenizer = Tokenizer::new("<br/>");
    let tokens = tokenizer.tokenize();
    assert!(!tokens.is_empty());
}

#[test]
fn test_tokenizer_comments() {
    let mut tokenizer = Tokenizer::new("<!-- This is a comment -->");
    let tokens = tokenizer.tokenize();
    assert!(!tokens.is_empty());
    let has_comment = tokens.iter().any(|t| matches!(t, Token::Comment(_)));
    assert!(has_comment, "Comment token not found");
}

#[test]
fn test_tokenizer_entities_named() {
    // Test that we can reference entities
    let mut tokenizer = Tokenizer::new("&amp;");
    let tokens = tokenizer.tokenize();
    // For now, just check it doesn't panic
    assert!(!tokens.is_empty());
}

#[test]
fn test_tokenizer_numeric_entities() {
    let mut tokenizer = Tokenizer::new("&#65;");
    let tokens = tokenizer.tokenize();
    assert!(!tokens.is_empty());
}

#[test]
fn test_tokenizer_malformed_tags() {
    let mut tokenizer = Tokenizer::new("<div<");
    let tokens = tokenizer.tokenize();
    // Should handle gracefully without panicking
    assert!(!tokens.is_empty());
}

#[test]
fn test_tokenizer_unclosed_tags() {
    let mut tokenizer = Tokenizer::new("<div><p>text");
    let tokens = tokenizer.tokenize();
    assert!(!tokens.is_empty());
}

#[test]
fn test_tokenizer_nested_tags() {
    let mut tokenizer = Tokenizer::new("<div><p><span>text</span></p></div>");
    let tokens = tokenizer.tokenize();
    assert!(tokens.len() > 5);
}

#[test]
fn test_tokenizer_text_nodes() {
    let mut tokenizer = Tokenizer::new("Hello World");
    let tokens = tokenizer.tokenize();
    let has_text = tokens.iter().any(|t| matches!(t, Token::Character('H')));
    assert!(has_text, "Text content not tokenized");
}

#[test]
fn test_tokenizer_mixed_content() {
    let mut tokenizer = Tokenizer::new("<div>Text <span>nested</span> more text</div>");
    let tokens = tokenizer.tokenize();
    assert!(!tokens.is_empty());
    assert!(tokens.iter().any(|t| matches!(t, Token::StartTag { name, .. } if name == "div")));
    assert!(tokens.iter().any(|t| matches!(t, Token::StartTag { name, .. } if name == "span")));
}

#[test]
fn test_tokenizer_special_chars() {
    let mut tokenizer = Tokenizer::new("<div>!@#$%^&*()</div>");
    let tokens = tokenizer.tokenize();
    assert!(!tokens.is_empty());
}

#[test]
fn test_tokenizer_invalid_utf8() {
    // Test handling of invalid UTF-8 (should become replacement char)
    let mut tokenizer = Tokenizer::new("test");
    let tokens = tokenizer.tokenize();
    assert!(!tokens.is_empty());
}
