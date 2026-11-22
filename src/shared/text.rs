//! The text policy the crate shares.
//!
//! Three decisions live here, because more than one layer needs them and none of them belongs to a
//! single client: how much of an external error body reaches the logs, how Telegram HTML is escaped,
//! and how text is split under Telegram's visible-character limit. What a body or a message means
//! stays with the caller.

/// Characters of an external error body kept for the logs.
pub(crate) const MAX_ERROR_BODY: usize = 500;

/// Maximum visible characters per Telegram message: the API rejects a longer one.
///
/// The limit budgets the text Telegram counts after entity parsing, not the markup, so chunking
/// stays on the raw text and escaping is applied per chunk, where a boundary can never split an
/// entity.
pub(crate) const TELEGRAM_MESSAGE_LIMIT: usize = 4_000;

/// Keeps at most [`MAX_ERROR_BODY`] characters of `body`, on a character boundary.
pub(crate) fn truncate(body: &str) -> String {
    match body.char_indices().nth(MAX_ERROR_BODY) {
        Some((index, _)) => body[..index].to_owned(),
        None => body.to_owned(),
    }
}

/// Escapes the characters Telegram's HTML parse mode treats as markup.
///
/// The ampersand is replaced first, so the entities introduced by the later replacements are not
/// escaped twice. Everything else is left untouched.
pub(crate) fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Splits `text` into pieces of at most `limit` bytes without cutting a character in half.
pub(crate) fn chunks(text: &str, limit: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let mut end = (start + limit).min(text.len());

        while !text.is_char_boundary(end) {
            end -= 1;
        }

        // A limit smaller than the next character still has to make progress.
        if end == start {
            end = start + text[start..].chars().next().map_or(0, char::len_utf8);
        }

        chunks.push(&text[start..end]);
        start = end;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_error_bodies_are_truncated() {
        let body = "x".repeat(MAX_ERROR_BODY * 2);
        assert_eq!(truncate(&body).chars().count(), MAX_ERROR_BODY);
        assert_eq!(truncate("short"), "short");
    }

    #[test]
    fn escape_html_escapes_markup_characters() {
        assert_eq!(escape_html("&"), "&amp;");
        assert_eq!(escape_html("<"), "&lt;");
        assert_eq!(escape_html(">"), "&gt;");
    }

    #[test]
    fn escape_html_escapes_the_ampersand_first() {
        assert_eq!(escape_html("a & b < c > d"), "a &amp; b &lt; c &gt; d");
        assert_eq!(escape_html("&lt;"), "&amp;lt;");
    }

    #[test]
    fn chunks_splits_ascii_text_at_the_limit() {
        assert_eq!(chunks("abcdef", 2), vec!["ab", "cd", "ef"]);
    }

    #[test]
    fn chunks_keeps_text_within_the_limit_in_one_piece() {
        assert_eq!(chunks("abcde", 5), vec!["abcde"]);
        assert_eq!(chunks("abc", 5), vec!["abc"]);
    }

    #[test]
    fn chunks_returns_nothing_for_empty_text() {
        assert!(chunks("", 5).is_empty());
    }

    #[test]
    fn chunks_never_splits_a_multibyte_character() {
        let text = "привет".repeat(2);

        let chunks = chunks(&text, 5);

        assert_eq!(chunks.concat(), text);
        for chunk in &chunks {
            assert!(!chunk.is_empty(), "a chunk must make progress");
            assert!(chunk.len() <= 5, "chunk {chunk:?} exceeds the limit");
        }
    }

    #[test]
    fn chunks_progress_even_below_one_character() {
        let chunks = chunks("привет", 1);

        assert_eq!(chunks.concat(), "привет");
        assert_eq!(chunks.len(), 6);
    }
}
