//! Per-app window rules: pure matching. Loading and hot reload are owned by the backend.
//!
//! A pattern is either an exact app id, a prefix ending in `*`, or `*` for any app id. The first
//! rule in file order whose pattern matches wins, so specific rules go before catch-alls.

use flamewm_api::Size;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRule {
    pub app_id: String,
    pub floating: Option<bool>,
    pub workspace: Option<u32>,
    pub size: Option<Size>,
}

impl WindowRule {
    #[must_use]
    pub fn new(app_id: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            floating: None,
            workspace: None,
            size: None,
        }
    }
}

/// True when `pattern` matches `app_id`: exact, `prefix*`, or `*`. An empty pattern matches nothing.
#[must_use]
pub fn pattern_matches(pattern: &str, app_id: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    match pattern.strip_suffix('*') {
        Some(prefix) => app_id.starts_with(prefix),
        None => pattern == app_id,
    }
}

/// First rule in order whose pattern matches `app_id`.
#[must_use]
pub fn match_rule<'a>(rules: &'a [WindowRule], app_id: &str) -> Option<&'a WindowRule> {
    rules
        .iter()
        .find(|rule| pattern_matches(&rule.app_id, app_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_prefix_and_wildcard_patterns() {
        assert!(pattern_matches("firefox", "firefox"));
        assert!(!pattern_matches("firefox", "firefox-esr"));
        assert!(pattern_matches("org.gnome.*", "org.gnome.Calculator"));
        assert!(!pattern_matches("org.gnome.*", "org.kde.kate"));
        assert!(pattern_matches("*", "anything"));
        assert!(pattern_matches("*", ""));
        assert!(!pattern_matches("", ""));
        assert!(!pattern_matches("", "x"));
    }

    #[test]
    fn first_matching_rule_wins_in_file_order() {
        let rules = vec![
            WindowRule {
                floating: Some(true),
                ..WindowRule::new("org.gnome.Calculator")
            },
            WindowRule {
                workspace: Some(3),
                ..WindowRule::new("org.gnome.*")
            },
            WindowRule {
                size: Some(Size::new(800, 600)),
                ..WindowRule::new("*")
            },
        ];
        let calc = match_rule(&rules, "org.gnome.Calculator").expect("specific rule");
        assert_eq!(calc.floating, Some(true));
        assert_eq!(calc.workspace, None);
        let other = match_rule(&rules, "org.gnome.Maps").expect("prefix rule");
        assert_eq!(other.workspace, Some(3));
        let catch_all = match_rule(&rules, "xterm").expect("catch-all");
        assert_eq!(catch_all.size, Some(Size::new(800, 600)));
    }

    #[test]
    fn no_rule_means_no_match() {
        let rules = vec![WindowRule::new("firefox")];
        assert!(match_rule(&rules, "chromium").is_none());
        assert!(match_rule(&[], "firefox").is_none());
    }
}
