//! Per-app window rules: pure matching. Loading and hot reload are owned by the backend.
//!
//! A pattern is either an exact app id, a prefix ending in `*`, or `*` for any app id. The first
//! rule in file order whose pattern matches wins, so specific rules go before catch-alls.
//!
//! File format, the same `key = value` line family as `settings.toml`: `#` starts a comment, each
//! `app = <pattern>` line starts a rule, and `workspace`, `floating`, `size` (`WxH`) follow it.
//! `workspace` is 1-based, as users see workspaces.

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

/// Parses a rules file. Any malformed line rejects the whole file, so a typo never applies a partial
/// rule set; the caller keeps its previous rules and reports the error.
pub fn parse_rules(text: &str) -> Result<Vec<WindowRule>, String> {
    let mut rules: Vec<WindowRule> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let number = index + 1;
        let (key, value) = line
            .split_once('=')
            .map(|(key, value)| (key.trim(), value.trim()))
            .ok_or_else(|| format!("line {number}: missing '='"))?;
        if key == "app" {
            if value.is_empty() {
                return Err(format!("line {number}: empty app pattern"));
            }
            rules.push(WindowRule::new(value));
            continue;
        }
        let rule = rules
            .last_mut()
            .ok_or_else(|| format!("line {number}: '{key}' before any 'app ='"))?;
        match key {
            "workspace" => {
                rule.workspace = Some(
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|workspace| *workspace >= 1)
                        .ok_or_else(|| format!("line {number}: workspace must be 1 or more"))?,
                );
            }
            "floating" => {
                rule.floating = Some(match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err(format!("line {number}: floating must be true or false")),
                });
            }
            "size" => {
                rule.size = Some(
                    parse_size(value).ok_or_else(|| format!("line {number}: size must be WxH"))?,
                );
            }
            _ => return Err(format!("line {number}: unknown key '{key}'")),
        }
    }
    Ok(rules)
}

fn parse_size(value: &str) -> Option<Size> {
    let (width, height) = value.split_once('x')?;
    let width = width.trim().parse::<i32>().ok().filter(|v| *v > 0)?;
    let height = height.trim().parse::<i32>().ok().filter(|v| *v > 0)?;
    Some(Size::new(width, height))
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
    fn parses_rules_in_file_order_with_comments() {
        let text = "# calc\napp = org.gnome.Calculator\nfloating = true\nsize = 400x300\n\napp = org.gnome.*\nworkspace = 3\n";
        let rules = parse_rules(text).expect("valid rules");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].floating, Some(true));
        assert_eq!(rules[0].size, Some(Size::new(400, 300)));
        assert_eq!(rules[0].workspace, None);
        assert_eq!(rules[1].app_id, "org.gnome.*");
        assert_eq!(rules[1].workspace, Some(3));
    }

    #[test]
    fn empty_file_means_no_rules() {
        assert_eq!(parse_rules("# only a comment\n"), Ok(Vec::new()));
    }

    #[test]
    fn malformed_lines_reject_the_whole_file() {
        for (text, needle) in [
            ("workspace = 2\n", "before any 'app ='"),
            ("app = a\nworkspace = 0\n", "workspace must be 1 or more"),
            (
                "app = a\nfloating = yes\n",
                "floating must be true or false",
            ),
            ("app = a\nsize = 0x300\n", "size must be WxH"),
            ("app = a\nsize = 800\n", "size must be WxH"),
            ("app = a\ncolor = red\n", "unknown key 'color'"),
            ("app = a\nfloating true\n", "missing '='"),
            ("app =\n", "empty app pattern"),
        ] {
            let error = parse_rules(text).expect_err(text);
            assert!(error.contains(needle), "{text:?} -> {error}");
        }
    }

    #[test]
    fn no_rule_means_no_match() {
        let rules = vec![WindowRule::new("firefox")];
        assert!(match_rule(&rules, "chromium").is_none());
        assert!(match_rule(&[], "firefox").is_none());
    }
}
