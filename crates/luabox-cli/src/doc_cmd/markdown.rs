//! Minimal markdown renderer for doc text (SPEC.md §13).
//!
//! Deliberately tiny (no dependency, predictable output). Supported
//! constructs:
//!
//! - paragraphs (blank-line separated),
//! - `` `inline code` `` spans,
//! - fenced code blocks (```` ``` ````),
//! - flat unordered (`- `, `* `) and ordered (`1. `) lists.
//!
//! Everything else — headings, emphasis, links, images, tables,
//! blockquotes, nested lists — is *not* interpreted and renders as literal
//! text (HTML-escaped). That is a documented limitation, not an oversight.

/// Escape text for safe interpolation into HTML content or attributes.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Render doc text to HTML. Never fails; unknown constructs pass through
/// as escaped literal text.
pub(crate) fn to_html(md: &str) -> String {
    let mut out = String::new();
    let mut para: Vec<String> = Vec::new();
    let mut ul: Vec<String> = Vec::new();
    let mut ol: Vec<String> = Vec::new();
    let mut fence: Option<Vec<String>> = None;

    for line in md.lines() {
        if let Some(buf) = fence.as_mut() {
            if line.trim_start().starts_with("```") {
                flush_code(&mut out, buf);
                fence = None;
            } else {
                buf.push(line.to_string());
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            flush_all(&mut out, &mut para, &mut ul, &mut ol);
            fence = Some(Vec::new());
        } else if trimmed.is_empty() {
            flush_all(&mut out, &mut para, &mut ul, &mut ol);
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            flush_para(&mut out, &mut para);
            flush_list(&mut out, &mut ol, "ol");
            ul.push(item.to_string());
        } else if let Some(item) = ordered_item(trimmed) {
            flush_para(&mut out, &mut para);
            flush_list(&mut out, &mut ul, "ul");
            ol.push(item.to_string());
        } else {
            flush_list(&mut out, &mut ul, "ul");
            flush_list(&mut out, &mut ol, "ol");
            para.push(trimmed.to_string());
        }
    }
    if let Some(mut buf) = fence.take() {
        // Unclosed fence: still render what was collected as code.
        flush_code(&mut out, &mut buf);
    }
    flush_all(&mut out, &mut para, &mut ul, &mut ol);
    out
}

/// The item text of an `N. item` ordered-list line, if it is one.
fn ordered_item(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() == line.len() {
        // No leading digits.
        return None;
    }
    rest.strip_prefix(". ")
}

fn flush_all(out: &mut String, para: &mut Vec<String>, ul: &mut Vec<String>, ol: &mut Vec<String>) {
    flush_para(out, para);
    flush_list(out, ul, "ul");
    flush_list(out, ol, "ol");
}

fn flush_para(out: &mut String, para: &mut Vec<String>) {
    if para.is_empty() {
        return;
    }
    out.push_str("<p>");
    out.push_str(&inline(&para.join(" ")));
    out.push_str("</p>\n");
    para.clear();
}

fn flush_list(out: &mut String, items: &mut Vec<String>, tag: &str) {
    if items.is_empty() {
        return;
    }
    out.push('<');
    out.push_str(tag);
    out.push_str(">\n");
    for item in items.iter() {
        out.push_str("<li>");
        out.push_str(&inline(item));
        out.push_str("</li>\n");
    }
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
    items.clear();
}

fn flush_code(out: &mut String, lines: &mut Vec<String>) {
    out.push_str("<pre><code>");
    for line in lines.iter() {
        out.push_str(&escape(line));
        out.push('\n');
    }
    out.push_str("</code></pre>\n");
    lines.clear();
}

/// Escape a line and convert `` `code` `` spans. An unmatched backtick is
/// left literal.
fn inline(text: &str) -> String {
    let escaped = escape(text);
    let mut out = String::with_capacity(escaped.len());
    let mut rest = escaped.as_str();
    while let Some((before, after)) = rest.split_once('`') {
        let Some((code, tail)) = after.split_once('`') else {
            break;
        };
        out.push_str(before);
        out.push_str("<code>");
        out.push_str(code);
        out.push_str("</code>");
        rest = tail;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_split_on_blank_lines() {
        let html = to_html("first line\nstill first\n\nsecond");
        assert_eq!(html, "<p>first line still first</p>\n<p>second</p>\n");
    }

    #[test]
    fn inline_code_spans() {
        let html = to_html("call `f(x)` twice");
        assert_eq!(html, "<p>call <code>f(x)</code> twice</p>\n");
    }

    #[test]
    fn unmatched_backtick_stays_literal() {
        let html = to_html("a ` b");
        assert_eq!(html, "<p>a ` b</p>\n");
    }

    #[test]
    fn fenced_code_block_is_escaped_verbatim() {
        let html = to_html("intro\n\n```lua\nlocal x = a < b\n```\nafter");
        assert!(html.contains("<pre><code>local x = a &lt; b\n</code></pre>"));
        assert!(html.contains("<p>intro</p>"));
        assert!(html.contains("<p>after</p>"));
    }

    #[test]
    fn unordered_and_ordered_lists() {
        let html = to_html("- one\n- two\n\n1. first\n2. second");
        assert_eq!(
            html,
            "<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n<ol>\n<li>first</li>\n<li>second</li>\n</ol>\n"
        );
    }

    #[test]
    fn unsupported_constructs_render_literally() {
        let html = to_html("# not a heading\n**not bold**");
        assert_eq!(html, "<p># not a heading **not bold**</p>\n");
    }

    #[test]
    fn html_in_doc_text_is_escaped() {
        let html = to_html("a <script> & \"quote\"");
        assert_eq!(html, "<p>a &lt;script&gt; &amp; &quot;quote&quot;</p>\n");
    }

    #[test]
    fn unclosed_fence_still_renders() {
        let html = to_html("```\ncode");
        assert_eq!(html, "<pre><code>code\n</code></pre>\n");
    }
}
