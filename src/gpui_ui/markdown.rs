//! Minimal markdown for the transcript: headings, bullet lists, inline code,
//! fenced code blocks, bold. Everything else renders as plain text.
//!
//! DECISION (see GPUI_NOTES.md): Zed's `markdown` crate at our pin depends on
//! `language` + `theme` + `ui` (tree-sitter and half the editor stack) — far
//! too heavy for a transcript. This ~200-line subset covers what Code Puppy
//! actually emits; upgrade later if real-world transcripts demand more.

use gpui::{AnyElement, FontWeight, IntoElement, ParentElement as _, Styled as _, div, px};

use super::tokens::Tokens;
use super::widgets::alpha;

/// One block-level chunk of a markdown document.
#[derive(Debug, PartialEq, Eq)]
pub enum Block {
    Heading(u8, String),
    Paragraph(String),
    Bullets(Vec<String>),
    Fence(String, String), // (language tag, code)
}

/// Split markdown text into block-level chunks (line-oriented, forgiving).
pub fn parse(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut bullets: Vec<String> = Vec::new();
    let mut fence: Option<(String, Vec<String>)> = None;

    let flush_para = |para: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            blocks.push(Block::Paragraph(para.join(" ")));
            para.clear();
        }
    };
    let flush_bullets = |bullets: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !bullets.is_empty() {
            blocks.push(Block::Bullets(std::mem::take(bullets)));
        }
    };

    for line in text.lines() {
        // Inside a fence: collect until the closing ```.
        if let Some((lang, code)) = &mut fence {
            if line.trim_start().starts_with("```") {
                blocks.push(Block::Fence(lang.clone(), code.join("\n")));
                fence = None;
            } else {
                code.push(line.to_string());
            }
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            flush_para(&mut para, &mut blocks);
            flush_bullets(&mut bullets, &mut blocks);
            fence = Some((rest.trim().to_string(), Vec::new()));
        } else if trimmed.starts_with('#') {
            flush_para(&mut para, &mut blocks);
            flush_bullets(&mut bullets, &mut blocks);
            let level = trimmed.chars().take_while(|&c| c == '#').count().min(6) as u8;
            let body = trimmed.trim_start_matches('#').trim().to_string();
            blocks.push(Block::Heading(level, body));
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            flush_para(&mut para, &mut blocks);
            bullets.push(item.to_string());
        } else if trimmed.is_empty() {
            flush_para(&mut para, &mut blocks);
            flush_bullets(&mut bullets, &mut blocks);
        } else {
            flush_bullets(&mut bullets, &mut blocks);
            para.push(trimmed);
        }
    }
    if let Some((lang, code)) = fence {
        blocks.push(Block::Fence(lang, code.join("\n"))); // unclosed fence
    }
    flush_para(&mut para, &mut blocks);
    flush_bullets(&mut bullets, &mut blocks);
    blocks
}

/// Inline segments: plain text / `code` / **bold**.
#[derive(Debug, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Code(String),
    Bold(String),
}

/// Split a paragraph into inline spans (backticks first, then `**`).
pub fn spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for (i, chunk) in text.split('`').enumerate() {
        if i % 2 == 1 && !chunk.is_empty() {
            out.push(Span::Code(chunk.to_string()));
        } else {
            // Bold inside non-code chunks.
            for (j, sub) in chunk.split("**").enumerate() {
                if sub.is_empty() {
                    continue;
                }
                if j % 2 == 1 {
                    out.push(Span::Bold(sub.to_string()));
                } else {
                    out.push(Span::Text(sub.to_string()));
                }
            }
        }
    }
    out
}

/// Render markdown text into a column of GPUI elements.
pub fn render(t: &Tokens, text: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .children(parse(text).into_iter().map(|b| render_block(t, b)))
        .into_any_element()
}

fn render_block(t: &Tokens, block: Block) -> AnyElement {
    match block {
        Block::Heading(level, body) => div()
            .text_size(px(match level {
                1 => 18.0,
                2 => 16.0,
                _ => 14.0,
            }))
            .font_weight(FontWeight::BOLD)
            .text_color(t.strong)
            .child(body)
            .into_any_element(),
        Block::Paragraph(body) => render_inline(t, &body),
        Block::Bullets(items) => div()
            .flex()
            .flex_col()
            .gap_0p5()
            .children(items.into_iter().map(|item| {
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(div().text_color(t.weak).child("\u{2022}"))
                    .child(div().min_w_0().flex_1().child(render_inline(t, &item)))
            }))
            .into_any_element(),
        Block::Fence(lang, code) => div()
            .flex()
            .flex_col()
            .rounded(px(8.))
            .bg(t.well)
            .border_1()
            .border_color(t.line_soft)
            .overflow_hidden()
            .children((!lang.is_empty()).then(|| {
                div()
                    .px_2p5()
                    .py_0p5()
                    .bg(t.panel)
                    .font_family("JetBrains Mono")
                    .text_size(px(10.))
                    .text_color(t.weak)
                    .child(lang)
            }))
            .child(
                div()
                    .px_2p5()
                    .py_1p5()
                    .font_family("JetBrains Mono")
                    .text_size(px(12.))
                    .text_color(t.text)
                    .whitespace_nowrap()
                    .overflow_x_hidden()
                    .flex()
                    .flex_col()
                    .children(code.split('\n').map(|l| div().child(l.to_string()))),
            )
            .into_any_element(),
    }
}

/// One wrapped row of inline spans (text + `code` + **bold**).
fn render_inline(t: &Tokens, text: &str) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x_1()
        .text_size(px(13.))
        .text_color(t.text)
        .children(spans(text).into_iter().map(|s| {
            match s {
                Span::Text(x) => div().child(x).into_any_element(),
                Span::Bold(x) => div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.strong)
                    .child(x)
                    .into_any_element(),
                Span::Code(x) => div()
                    .px_1()
                    .rounded(px(4.))
                    .bg(alpha(t.accent, 0.10))
                    .font_family("JetBrains Mono")
                    .text_size(px(12.))
                    .text_color(t.accent_2)
                    .child(x)
                    .into_any_element(),
            }
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headings_lists_fences() {
        let md = "# Title\n\nSome text\nmore text\n\n- one\n- two\n\n```rust\nfn x() {}\n```";
        let blocks = parse(md);
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0], Block::Heading(1, "Title".into()));
        assert_eq!(blocks[1], Block::Paragraph("Some text more text".into()));
        assert_eq!(blocks[2], Block::Bullets(vec!["one".into(), "two".into()]));
        assert_eq!(blocks[3], Block::Fence("rust".into(), "fn x() {}".into()));
    }

    #[test]
    fn unclosed_fence_still_renders() {
        let blocks = parse("```\ncode here");
        assert_eq!(
            blocks,
            vec![Block::Fence(String::new(), "code here".into())]
        );
    }

    #[test]
    fn inline_code_and_bold() {
        let s = spans("run `cargo test` and **win**");
        assert_eq!(
            s,
            vec![
                Span::Text("run ".into()),
                Span::Code("cargo test".into()),
                Span::Text(" and ".into()),
                Span::Bold("win".into()),
            ]
        );
    }

    #[test]
    fn plain_text_is_one_paragraph() {
        assert_eq!(parse("hello"), vec![Block::Paragraph("hello".into())]);
    }
}
