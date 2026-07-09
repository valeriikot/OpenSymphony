//! Vikunja stores descriptions and comments as HTML fragments. Agent-authored
//! content (e.g. the workpad comment) round-trips as plain text, but
//! user-edited content arrives with markup, so flatten it to plain text before
//! matching markers or rendering into prompts.

const BLOCK_TAGS: &[&str] = &[
    "br", "p", "div", "li", "ul", "ol", "h1", "h2", "h3", "h4", "h5", "h6", "blockquote", "pre",
    "tr", "table", "thead", "tbody",
];

const INLINE_TAGS: &[&str] = &[
    "a", "b", "i", "u", "s", "em", "strong", "span", "code", "del", "ins", "mark", "sub", "sup",
    "small", "img", "hr", "td", "th", "label", "input",
];

/// Flatten an HTML fragment into plain text, turning block-level boundaries
/// into newlines and decoding the handful of entities Vikunja's editor emits.
/// Only recognized HTML tag names are stripped, so plain text containing
/// angle-bracket tokens (`<none>`, `Vec<String>`) passes through unchanged.
pub(super) fn html_to_text(html: &str) -> Option<String> {
    let mut text = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        let candidate = &rest[open..];
        match recognized_tag(candidate) {
            Some((tag_name, tag_len)) => {
                if is_block_tag(&tag_name) && !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                rest = &candidate[tag_len..];
            }
            None => {
                text.push('<');
                rest = &candidate['<'.len_utf8()..];
            }
        }
    }
    text.push_str(rest);

    let text = decode_entities(&text);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// If `input` (starting with `<`) opens a recognized HTML tag, return the
/// lowercase tag name and the byte length of the whole `<...>` sequence.
fn recognized_tag(input: &str) -> Option<(String, usize)> {
    let end = input.find('>')?;
    let inner = &input[1..end];
    // A tag name must immediately follow `<` (or `</`); `< b` is plain text.
    let name_part = inner.strip_prefix('/').unwrap_or(inner);
    if !name_part
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic())
    {
        return None;
    }
    let name = name_part
        .split([' ', '\t', '\n', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if is_block_tag(&name) || INLINE_TAGS.contains(&name.as_str()) {
        Some((name, end + 1))
    } else {
        None
    }
}

fn is_block_tag(name: &str) -> bool {
    BLOCK_TAGS.contains(&name)
}

fn decode_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::html_to_text;

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(
            html_to_text("## Agent Harness Workpad\nstate: done"),
            Some("## Agent Harness Workpad\nstate: done".to_string())
        );
    }

    #[test]
    fn unrecognized_angle_bracket_tokens_are_preserved() {
        assert_eq!(
            html_to_text("blocked_on: <none>\nuse Vec<String>"),
            Some("blocked_on: <none>\nuse Vec<String>".to_string())
        );
        assert_eq!(html_to_text("a < b and c > d"), Some("a < b and c > d".to_string()));
    }

    #[test]
    fn block_tags_become_line_breaks_and_entities_decode() {
        assert_eq!(
            html_to_text("<p>first &amp; second</p><p>third</p>"),
            Some("first & second\nthird".to_string())
        );
        assert_eq!(html_to_text("a<br/>b"), Some("a\nb".to_string()));
        assert_eq!(
            html_to_text("<p><strong>bold</strong> text</p>"),
            Some("bold text".to_string())
        );
    }

    #[test]
    fn empty_markup_yields_none() {
        assert_eq!(html_to_text("<p></p>"), None);
        assert_eq!(html_to_text("   "), None);
    }
}
