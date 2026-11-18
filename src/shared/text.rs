//! Text policy shared by the clients that quote an external body back to the logs.
//!
//! Only how much of a body reaches the logs lives here; what the body means stays with the client
//! that read it.

/// Characters of an external error body kept for the logs.
pub(crate) const MAX_ERROR_BODY: usize = 500;

/// Keeps at most [`MAX_ERROR_BODY`] characters of `body`, on a character boundary.
pub(crate) fn truncate(body: &str) -> String {
    match body.char_indices().nth(MAX_ERROR_BODY) {
        Some((index, _)) => body[..index].to_owned(),
        None => body.to_owned(),
    }
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
}
