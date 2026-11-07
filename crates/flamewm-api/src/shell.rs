//! POSIX shell word quoting shared by session and remote-command builders.

/// Single-quotes `text` so `sh` reads it as one literal word; embedded `'` becomes `'\''`.
#[must_use]
pub fn single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_single_quotes_and_metacharacters_literal() {
        assert_eq!(single_quote("a'b"), "'a'\\''b'");
        assert_eq!(single_quote("$(rm)"), "'$(rm)'");
    }
}
