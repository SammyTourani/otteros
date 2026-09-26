//! Acceptance oracle for brief M5-T2a (parse simple HTML into a tree, render it into blocks for
//! the page viewer, and into wrapped text for the terminal's `fetch`), written by the orchestrator.
//! The crate must pass this file unchanged.
//!
//! API this file relies on:
//!   otter_html::parse(&str) -> Document; Document::title() (existing)
//!   otter_html::render(&Document, base_url: &str) -> Vec<Block>
//!   Block::{Heading { level: u8, spans: Vec<Span> }, Paragraph { spans }, ListItem { depth: u8,
//!     marker: String, spans }, Preformatted { text: String }, Rule}
//!   Span { text: String, link: Option<String>, bold: bool, italic: bool, code: bool }
//!   otter_html::to_text(&[Block], width: usize) -> String
//!   (Block and Span: Debug, Clone, PartialEq, Eq)
//!
//! Rules:
//! - parse: a forgiving tree builder over the tokenizer. Void elements (area base br col embed hr
//!   img input link meta source track wbr) never get children; `<p>` is closed by a block start
//!   tag (address article aside blockquote div dl fieldset figure footer form h1-h6 header hr main
//!   nav ol p pre section table ul) and by `</p>`; `<li>` closes an open `<li>` of the same list,
//!   `<dt>`/`<dd>` close each other; an end tag closes up to the nearest matching open element and
//!   is ignored when there is none; html, head and body are implied when missing (title, base,
//!   meta, link, style and script before any body content go into head), so `Document::title()`
//!   (whose text is stripped and whitespace-collapsed like `document.title`) and `base_href()` work
//!   on fragments too. In document mode the tokenizer
//!   switches to script data after `<script>`, RAWTEXT after style/xmp/iframe/noembed/noframes and
//!   RCDATA after title/textarea (the html5lib-conformant `Tokenizer::new`/`with_state` stay as
//!   they are).
//! - render: blocks in document order. Headings h1-h6; p, div, section, article, header, footer,
//!   main, nav, aside, figure, figcaption, address, blockquote, dt, dd and table rows make
//!   paragraphs (table cells joined by " | "); li makes a ListItem (depth 0 for a top-level list,
//!   "•" in ul, "N." in ol honouring `start`); pre keeps its text verbatim (one leading newline
//!   dropped); hr is a Rule; text outside any block becomes a paragraph of its own. Whitespace runs
//!   collapse to one space and are trimmed at block edges; `<br>` is a "\n" inside the span text
//!   (not collapsed, and it merges like any text). Links: `a[href]`
//!   resolved against the base URL (and `<base href>` if present); `javascript:`/`mailto:` hrefs and
//!   hrefs that fail to resolve give no link. b/strong bold, i/em italic, code/kbd/samp/tt code.
//!   img shows `[alt]` when alt is non-empty. Nothing from head (other than title), script, style,
//!   template, svg, math, noscript, select, textarea, button or input is shown. Adjacent spans with
//!   identical styles and link merge; empty blocks are dropped.
//! - to_text: headings as "#"×level + " " + text; paragraphs and list items word-wrapped at
//!   `width` characters (not bytes; words longer than a line stand alone; "\n" forces a break); list items indented 2×depth spaces, marker, one
//!   space, continuation lines aligned after the marker; pre verbatim; Rule as `width` dashes;
//!   linked text followed by "[n]", numbered in order of first appearance, with "[n] url" lines
//!   under a final "References:" line. One blank line between blocks, none between consecutive list
//!   items; no trailing spaces; the output ends with exactly one newline.

use otter_html::{parse, render, to_text, Block, Span};

fn plain(text: &str) -> Span {
    Span { text: text.into(), link: None, bold: false, italic: false, code: false }
}

fn link(text: &str, url: &str) -> Span {
    Span { link: Some(url.into()), ..plain(text) }
}

fn para(spans: Vec<Span>) -> Block {
    Block::Paragraph { spans }
}

const BASE: &str = "https://example.com/docs/index.html";

#[test]
fn headings_paragraphs_and_whitespace() {
    let doc = parse("<html><head><title>Otter  News</title><style>p { color: red }</style></head>\n<body>\n<h1>Hello,\n   world</h1>\n<p>  One   two\tthree  </p><p>Second<br>line</p></body></html>");
    assert_eq!(doc.title(), "Otter News", "stripped and collapsed like document.title");
    assert_eq!(
        render(&doc, BASE),
        [
            Block::Heading { level: 1, spans: vec![plain("Hello, world")] },
            para(vec![plain("One two three")]),
            para(vec![plain("Second\nline")]),
        ]
    );
}

#[test]
fn inline_styles_links_and_images() {
    let doc = parse(r#"<p>Read <a href="../about.html">about <b>us</b></a>, <em>now</em>. <code>x = 1</code> <img alt="otter" src="o.png"><img src="x.png"> <a href="javascript:void(0)">js</a> <a href="https://otter.test/">abs</a></p>"#);
    assert_eq!(
        render(&doc, BASE),
        [para(vec![
            plain("Read "),
            link("about ", "https://example.com/about.html"),
            Span { bold: true, ..link("us", "https://example.com/about.html") },
            plain(", "),
            Span { italic: true, ..plain("now") },
            plain(". "),
            Span { code: true, ..plain("x = 1") },
            plain(" [otter] js "),
            link("abs", "https://otter.test/"),
        ])]
    );
}

#[test]
fn base_href_changes_link_resolution() {
    let doc = parse(r#"<head><base href="https://cdn.example.org/a/"></head><p><a href="b.html">b</a></p>"#);
    assert_eq!(render(&doc, BASE), [para(vec![link("b", "https://cdn.example.org/a/b.html")])]);
}

#[test]
fn lists_nest_and_number() {
    let doc = parse("<ul><li>one<li>two<ul><li>two-a</li></ul></li></ul><ol start=\"3\"><li>three</li><li>four</ol>");
    let items: Vec<(u8, String, String)> = render(&doc, BASE)
        .into_iter()
        .map(|b| match b {
            Block::ListItem { depth, marker, spans } => (depth, marker, spans.iter().map(|s| s.text.clone()).collect()),
            other => panic!("expected list items, got {other:?}"),
        })
        .collect();
    assert_eq!(
        items,
        [
            (0, "•".into(), "one".into()),
            (0, "•".into(), "two".into()),
            (1, "•".into(), "two-a".into()),
            (0, "3.".into(), "three".into()),
            (0, "4.".into(), "four".into()),
        ]
    );
}

#[test]
fn forgiving_tree_building() {
    // Unclosed p before a div, a stray </span>, a p closed by h2, and text outside any block.
    let doc = parse("loose text<p>first<div>inside div</div></span><p>second<h2>Title</h2>tail");
    assert_eq!(
        render(&doc, BASE),
        [
            para(vec![plain("loose text")]),
            para(vec![plain("first")]),
            para(vec![plain("inside div")]),
            para(vec![plain("second")]),
            Block::Heading { level: 2, spans: vec![plain("Title")] },
            para(vec![plain("tail")]),
        ]
    );
}

#[test]
fn raw_text_elements_hide_markup() {
    let doc = parse("<p>a</p><script>if (x < 1) { document.write('<p>no</p>') }</script><style>div > p { }</style><textarea><p>no</p></textarea><p>b</p>");
    assert_eq!(render(&doc, BASE), [para(vec![plain("a")]), para(vec![plain("b")])]);
    let doc = parse("<title>A <b>bold</b> title</title><p>x</p>");
    assert_eq!(doc.title(), "A <b>bold</b> title", "title is RCDATA");
}

#[test]
fn pre_tables_rules_and_hidden_things() {
    let doc = parse("<pre>\n  fn main() {\n      println!(\"&lt;hi&gt;\");\n  }\n</pre><hr><table><tr><td>a</td><td> b </td></tr><tr><th>c</th></tr></table><noscript>enable js</noscript><button>Go</button><p><input value=x>ok</p>");
    assert_eq!(
        render(&doc, BASE),
        [
            Block::Preformatted { text: "  fn main() {\n      println!(\"<hi>\");\n  }\n".into() },
            Block::Rule,
            para(vec![plain("a | b")]),
            para(vec![plain("c")]),
            para(vec![plain("ok")]),
        ]
    );
}

#[test]
fn text_output_wraps_and_references_links() {
    let doc = parse(r#"<h1>OtterOS</h1><p>An operating system written by an AI, with an AI inside it. See <a href="/source">the source</a> and <a href="https://otter.test/demo">the demo</a>.</p><ul><li>Boots on a real laptop and keeps going</li><li>Runs a local model</li></ul><pre>  $ fetch
  done</pre><hr><p>Again: <a href="/source">source</a>.</p>"#);
    let text = to_text(&render(&doc, BASE), 30);
    let expected = "\
# OtterOS

An operating system written by
an AI, with an AI inside it.
See the source[1] and the
demo[2].

• Boots on a real laptop and
  keeps going
• Runs a local model

  $ fetch
  done

------------------------------

Again: source[1].

References:
[1] https://example.com/source
[2] https://otter.test/demo
";
    assert_eq!(text, expected);
}

#[test]
fn long_words_and_narrow_widths() {
    let doc = parse("<p>tiny supercalifragilisticexpialidocious end</p><ol><li>alpha beta gamma</li></ol>");
    let text = to_text(&render(&doc, BASE), 10);
    assert_eq!(text, "tiny\nsupercalifragilisticexpialidocious\nend\n\n1. alpha\n   beta\n   gamma\n");
}

#[test]
fn garbage_never_panics() {
    let pieces = ["<p>", "</p>", "<li>", "<ul>", "</ul>", "<a href='x'>", "</a>", "<pre>", "</pre>", "<br>", "&amp;", "&bogus", "<", ">", "</", "<script>", "</script>", "<table><tr><td>", "text ", "\u{0}", "é", "<h3>", "</h7>", "<!--", "-->", "<b><i>", "</b></i>"];
    let mut x = 42u64;
    for _ in 0..2000 {
        let mut html = String::new();
        for _ in 0..(x % 40) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            html.push_str(pieces[(x % pieces.len() as u64) as usize]);
        }
        x = x.wrapping_add(1);
        let blocks = render(&parse(&html), BASE);
        let _ = to_text(&blocks, 20);
    }
}
