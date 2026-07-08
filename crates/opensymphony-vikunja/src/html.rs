//! Vikunja stores descriptions and comments as HTML fragments. Agent-authored
//! content (e.g. the workpad comment) round-trips as plain text, but
//! user-edited content arrives with markup, so flatten it to plain text before
//! matching markers or rendering into prompts.

/// Flatten an HTML fragment into plain text, turning block-level boundaries
/// into newlines and decoding the handful of entities Vikunja's editor emits.
/// Plain-text input passes through unchanged.
pub(super) fn html_to_text(html: &str) -> Option<String> {
    let mut text = String::with_capacity(html.len());
    let mut chars = html.char_indices().peekable();

    while let Some((start, character)) = chars.next() {
        if character != '<' {
            text.push(character);
            continue;
        }

        let rest = &html[start..];
        let Some(end) = rest.find('>') else {
            // Unterminated `<` — treat the remainder as literal text.
            text.push_str(rest);
            break;
        };
        let tag = rest[1..end].trim();
        if is_block_boundary_tag(tag) && !text.ends_with('\n') {
            text.push('\n');
        }
        // Skip the characters that made up the tag.
        while let Some((index, _)) = chars.peek() {
            if *index > start + end {
                break;
            }
            chars.next();
        }
    }

    let text = decode_entities(&text);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn is_block_boundary_tag(tag: &str) -> bool {
    let name = tag
        .trim_start_matches('/')
        .split([' ', '\t', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "br" | "p" | "div" | "li" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            | "blockquote" | "pre" | "tr" | "table"
    )
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
    fn block_tags_become_line_breaks_and_entities_decode() {
        assert_eq!(
            html_to_text("<p>first &amp; second</p><p>third</p>"),
            Some("first & second\nthird".to_string())
        );
        assert_eq!(
            html_to_text("a<br/>b"),
            Some("a\nb".to_string())
        );
    }

    #[test]
    fn empty_markup_yields_none() {
        assert_eq!(html_to_text("<p></p>"), None);
        assert_eq!(html_to_text("   "), None);
    }
}
