//! Header subtitle for the chat window. A pure text rule, so the window code only forwards it.

use crate::Link;

/// Short status line shown under the window title.
#[must_use]
pub fn subtitle(link: Link, working: bool) -> &'static str {
    match (working, link) {
        (true, _) => "Thinking...",
        (false, Link::Checking) => "Checking...",
        (false, Link::Online) => "Online",
        (false, Link::Offline) => "Offline",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_follows_the_link_and_turn() {
        assert_eq!(subtitle(Link::Checking, false), "Checking...");
        assert_eq!(subtitle(Link::Online, false), "Online");
        assert_eq!(subtitle(Link::Offline, false), "Offline");
        assert_eq!(subtitle(Link::Online, true), "Thinking...");
    }
}
