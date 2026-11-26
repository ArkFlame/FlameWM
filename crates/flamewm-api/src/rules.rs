//! Window and monitor rules, parsed from settings TOML.
//!
//! This module is a pure contract: it owns the rule types, their matchers, first-match-wins
//! resolution, and no backend wiring. The WM reads the resolved rules; it never lets a rule
//! become an authority of its own.
//!
//! # TOML shape
//!
//! Rules are plain TOML array-of-tables. Field names match the struct fields; every matcher and
//! action is optional, so an omitted field simply does not take part in the decision.
//!
//! ```toml
//! [[window_rule]]
//! app_id = "org.gnome.Terminal*"
//! class = "gnome-terminal*"
//! is_floating = true
//! width = 900.0
//! height = 600.0
//!
//! [[window_rule]]
//! title = "Picture-in-Picture"
//! is_floating = true
//! open_maximized = false
//!
//! [[monitor_rule]]
//! name = "eDP-1"
//! make = "Chimei Innolux Corporation"
//! x = 0
//! y = 0
//! scale = 1.25
//! rotation = "normal"
//! primary = true
//! disabled = false
//! ```
//!
//! Deserialize the whole document into [`RulesConfig`]:
//!
//! ```ignore
//! let rules: flamewm_api::rules::RulesConfig = toml::from_str(&settings_text)?;
//! ```
//!
//! # Matching order
//!
//! Rules are evaluated in declaration order and the **first** rule that matches wins
//! ([`resolve_window_rule`], [`resolve_monitor_rule`]). Put the most specific rules first and any
//! catch-all last. A rule with no matchers matches every candidate, which makes it an explicit
//! default.
//!
//! Patterns are regex-free globs, not regular expressions: `*` matches any run of characters and
//! `?` matches exactly one character. Every other character, including `.`, `[` and `]`, is
//! literal. Matching is case-sensitive and operates on Unicode scalar values. Set matchers are
//! combined with AND: when both `app_id` and `title` are set, both must match.

use serde::Deserialize;

use crate::display::Rotation;

/// Regex-free glob matcher: `*` matches any run of characters, `?` matches one character, and all
/// other characters are matched literally.
#[must_use]
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut retry = 0usize;

    while ti < text.len() {
        if let Some(&token) = pattern.get(pi) {
            if token == '?' || token == text[ti] {
                pi += 1;
                ti += 1;
                continue;
            }
            if token == '*' {
                star = Some(pi);
                retry = ti;
                pi += 1;
                continue;
            }
        }
        if let Some(start) = star {
            pi = start + 1;
            retry += 1;
            ti = retry;
            continue;
        }
        return false;
    }

    while pattern.get(pi) == Some(&'*') {
        pi += 1;
    }
    pi == pattern.len()
}

/// Identity of the window a [`WindowRule`] is tested against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowCandidate<'a> {
    /// Application id reported by the client.
    pub app_id: &'a str,
    /// `WM_CLASS` class the client announces.
    pub class: &'a str,
    /// Current window title.
    pub title: &'a str,
}

/// One `[[window_rule]]` entry: a set of glob matchers plus the actions to apply.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowRule {
    /// Glob matched against [`WindowCandidate::app_id`].
    pub app_id: Option<String>,
    /// Glob matched against [`WindowCandidate::class`].
    pub class: Option<String>,
    /// Glob matched against [`WindowCandidate::title`].
    pub title: Option<String>,
    /// Force the window to be floating.
    pub is_floating: Option<bool>,
    /// Floating width in pixels.
    pub width: Option<f64>,
    /// Floating height in pixels.
    pub height: Option<f64>,
    /// Workspace to open the window on.
    pub workspace: Option<String>,
    /// Open the window maximized (the fix for blurry fractional-scaled Chromium apps).
    pub open_maximized: Option<bool>,
}

impl WindowRule {
    /// Whether every matcher set on this rule matches `candidate`.
    ///
    /// A rule with no matchers matches every candidate; set matchers are combined with AND.
    #[must_use]
    pub fn matches(&self, candidate: &WindowCandidate<'_>) -> bool {
        self.app_id
            .as_deref()
            .is_none_or(|pattern| glob_matches(pattern, candidate.app_id))
            && self
                .class
                .as_deref()
                .is_none_or(|pattern| glob_matches(pattern, candidate.class))
            && self
                .title
                .as_deref()
                .is_none_or(|pattern| glob_matches(pattern, candidate.title))
    }
}

/// Identity of the output a [`MonitorRule`] is tested against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorCandidate<'a> {
    /// Connector name, for example `eDP-1`.
    pub name: &'a str,
    /// Manufacturer from EDID.
    pub make: &'a str,
    /// Model from EDID.
    pub model: &'a str,
}

/// Rotation requested by a [`MonitorRule`], in clockwise quarter turns.
///
/// This mirrors [`Rotation`], the canonical display type, but carries its own `Deserialize` impl
/// so `flamewm-api` stays the single owner of settings parsing. Convert with [`Self::to_display`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MonitorRotation {
    /// Native orientation (mango transform `0`).
    #[default]
    Normal,
    /// Quarter turn counter-clockwise (mango transform `1`).
    Left,
    /// Half turn (mango transform `2`).
    Inverted,
    /// Quarter turn clockwise (mango transform `3`).
    Right,
}

impl MonitorRotation {
    /// Convert into the display owner's rotation type.
    #[must_use]
    pub const fn to_display(self) -> Rotation {
        match self {
            Self::Normal => Rotation::Normal,
            Self::Left => Rotation::Left,
            Self::Right => Rotation::Right,
            Self::Inverted => Rotation::Inverted,
        }
    }
}

/// One `[[monitor_rule]]` entry: matchers plus the output settings to apply.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorRule {
    /// Glob matched against [`MonitorCandidate::name`].
    pub name: Option<String>,
    /// Glob matched against [`MonitorCandidate::make`].
    pub make: Option<String>,
    /// Glob matched against [`MonitorCandidate::model`].
    pub model: Option<String>,
    /// X origin in the shared desktop coordinate space.
    pub x: Option<i32>,
    /// Y origin in the shared desktop coordinate space.
    pub y: Option<i32>,
    /// `HiDPI` scale factor.
    pub scale: Option<f64>,
    /// Requested panel rotation.
    pub rotation: Option<MonitorRotation>,
    /// Mark as the `RandR` primary output for `XWayland` clients.
    pub primary: Option<bool>,
    /// Disable the output.
    pub disabled: Option<bool>,
}

impl MonitorRule {
    /// Whether every matcher set on this rule matches `candidate`.
    ///
    /// A rule with no matchers matches every candidate; set matchers are combined with AND.
    #[must_use]
    pub fn matches(&self, candidate: &MonitorCandidate<'_>) -> bool {
        self.name
            .as_deref()
            .is_none_or(|pattern| glob_matches(pattern, candidate.name))
            && self
                .make
                .as_deref()
                .is_none_or(|pattern| glob_matches(pattern, candidate.make))
            && self
                .model
                .as_deref()
                .is_none_or(|pattern| glob_matches(pattern, candidate.model))
    }
}

/// The `window_rule` and `monitor_rule` arrays of a settings document.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RulesConfig {
    /// Deserialized from `[[window_rule]]` tables, in declaration order.
    #[serde(rename = "window_rule", default)]
    pub window_rules: Vec<WindowRule>,
    /// Deserialized from `[[monitor_rule]]` tables, in declaration order.
    #[serde(rename = "monitor_rule", default)]
    pub monitor_rules: Vec<MonitorRule>,
}

impl RulesConfig {
    /// First matching window rule, or `None` when the list is empty or nothing matches.
    #[must_use]
    pub fn resolve_window(&self, candidate: &WindowCandidate<'_>) -> Option<&WindowRule> {
        resolve_window_rule(&self.window_rules, candidate)
    }

    /// First matching monitor rule, or `None` when the list is empty or nothing matches.
    #[must_use]
    pub fn resolve_monitor(&self, candidate: &MonitorCandidate<'_>) -> Option<&MonitorRule> {
        resolve_monitor_rule(&self.monitor_rules, candidate)
    }
}

/// First matching rule wins; earlier entries shadow later ones.
#[must_use]
pub fn resolve_window_rule<'a>(
    rules: &'a [WindowRule],
    candidate: &WindowCandidate<'_>,
) -> Option<&'a WindowRule> {
    rules.iter().find(|rule| rule.matches(candidate))
}

/// First matching rule wins; earlier entries shadow later ones.
#[must_use]
pub fn resolve_monitor_rule<'a>(
    rules: &'a [MonitorRule],
    candidate: &MonitorCandidate<'_>,
) -> Option<&'a MonitorRule> {
    rules.iter().find(|rule| rule.matches(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_supports_stars_question_marks_and_literals() {
        assert!(glob_matches("*", "anything with spaces"));
        assert!(glob_matches("firefox*", "firefox-developer"));
        assert!(glob_matches("*Terminal*", "org.gnome.Terminal"));
        assert!(glob_matches("a?c", "abc"));
        assert!(!glob_matches("a?c", "ac"));
        assert!(!glob_matches("a?c", "abbc"));
        // Regex metacharacters are literal, not special.
        assert!(glob_matches("a[b", "a[b"));
        assert!(!glob_matches("a[b", "ab"));
        assert!(glob_matches("main.rs", "main.rs"));
        assert!(!glob_matches("main.rs", "mainXrs"));
        // `?` consumes one Unicode scalar value, not one byte.
        assert!(glob_matches("caf?", "café"));
    }

    #[test]
    fn window_rule_combines_all_set_matchers_with_and() {
        let rule = WindowRule {
            app_id: Some("org.gnome.*".into()),
            title: Some("Terminal*".into()),
            ..WindowRule::default()
        };
        assert!(rule.matches(&WindowCandidate {
            app_id: "org.gnome.Terminal",
            class: "anything",
            title: "Terminal — bash",
        }));
        assert!(!rule.matches(&WindowCandidate {
            app_id: "org.gnome.Terminal",
            class: "anything",
            title: "Text Editor",
        }));
        assert!(!rule.matches(&WindowCandidate {
            app_id: "org.kde.konsole",
            class: "anything",
            title: "Terminal — bash",
        }));
    }

    #[test]
    fn window_rule_without_matchers_is_a_catch_all() {
        let rule = WindowRule {
            is_floating: Some(true),
            ..WindowRule::default()
        };
        assert!(rule.matches(&WindowCandidate {
            app_id: "anything",
            class: "anything",
            title: "anything",
        }));
    }

    #[test]
    fn window_resolution_is_first_match_wins() {
        let rules = vec![
            WindowRule {
                title: Some("Picture-in-Picture".into()),
                is_floating: Some(true),
                ..WindowRule::default()
            },
            WindowRule {
                app_id: Some("org.gnome.*".into()),
                is_floating: Some(false),
                workspace: Some("2".into()),
                ..WindowRule::default()
            },
            WindowRule {
                is_floating: Some(true),
                ..WindowRule::default()
            },
        ];
        let pip = WindowCandidate {
            app_id: "org.gnome.Terminal",
            class: "gnome-terminal",
            title: "Picture-in-Picture",
        };
        // The title rule is first, so it shadows the app_id and catch-all rules.
        let resolved = resolve_window_rule(&rules, &pip).expect("title rule matches");
        assert_eq!(resolved.title.as_deref(), Some("Picture-in-Picture"));
        assert_eq!(resolved.is_floating, Some(true));

        let terminal = WindowCandidate {
            app_id: "org.gnome.Terminal",
            class: "gnome-terminal",
            title: "Terminal — bash",
        };
        let resolved = resolve_window_rule(&rules, &terminal).expect("app_id rule matches");
        assert_eq!(resolved.workspace.as_deref(), Some("2"));
    }

    #[test]
    fn monitor_rule_combines_all_set_matchers_with_and() {
        let rule = MonitorRule {
            name: Some("eDP-*".into()),
            make: Some("Chimei Innolux Corporation".into()),
            ..MonitorRule::default()
        };
        assert!(rule.matches(&MonitorCandidate {
            name: "eDP-1",
            make: "Chimei Innolux Corporation",
            model: "0x15F5",
        }));
        assert!(!rule.matches(&MonitorCandidate {
            name: "eDP-1",
            make: "Some Other Vendor",
            model: "0x15F5",
        }));
        assert!(!rule.matches(&MonitorCandidate {
            name: "HDMI-A-1",
            make: "Chimei Innolux Corporation",
            model: "0x15F5",
        }));
    }

    #[test]
    fn monitor_resolution_is_first_match_wins() {
        let rules = vec![
            MonitorRule {
                name: Some("eDP-1".into()),
                x: Some(0),
                y: Some(0),
                primary: Some(true),
                ..MonitorRule::default()
            },
            MonitorRule {
                make: Some("Chimei Innolux Corporation".into()),
                x: Some(1920),
                y: Some(0),
                ..MonitorRule::default()
            },
            MonitorRule {
                x: Some(0),
                y: Some(0),
                ..MonitorRule::default()
            },
        ];
        let laptop = MonitorCandidate {
            name: "eDP-1",
            make: "Chimei Innolux Corporation",
            model: "0x15F5",
        };
        let resolved = resolve_monitor_rule(&rules, &laptop).expect("name rule matches");
        assert_eq!(resolved.primary, Some(true));
        assert_eq!(resolved.x, Some(0));

        let external = MonitorCandidate {
            name: "HDMI-A-1",
            make: "Chimei Innolux Corporation",
            model: "0x15F5",
        };
        let resolved = resolve_monitor_rule(&rules, &external).expect("make rule matches");
        assert_eq!(resolved.x, Some(1920));
    }

    #[test]
    fn rules_config_parses_toml_and_preserves_rule_order() {
        let toml_text = r#"
            [[window_rule]]
            app_id = "org.gnome.Terminal*"
            class = "gnome-terminal*"
            is_floating = true
            width = 900.0
            height = 600.0

            [[window_rule]]
            title = "Picture-in-Picture"
            is_floating = true
            open_maximized = false

            [[monitor_rule]]
            name = "eDP-1"
            make = "Chimei Innolux Corporation"
            x = 0
            y = 0
            scale = 1.25
            rotation = "normal"
            primary = true
            disabled = false
        "#;

        let config: RulesConfig = toml::from_str(toml_text).expect("valid rules TOML");
        assert_eq!(config.window_rules.len(), 2);
        assert_eq!(config.monitor_rules.len(), 1);

        let first = &config.window_rules[0];
        assert_eq!(first.app_id.as_deref(), Some("org.gnome.Terminal*"));
        assert_eq!(first.is_floating, Some(true));
        assert_eq!(first.width, Some(900.0));
        assert_eq!(first.workspace, None);

        let monitor = &config.monitor_rules[0];
        assert_eq!(monitor.name.as_deref(), Some("eDP-1"));
        assert_eq!(monitor.scale, Some(1.25));
        assert_eq!(monitor.rotation, Some(MonitorRotation::Normal));
        assert_eq!(
            monitor.rotation.map(MonitorRotation::to_display),
            Some(Rotation::Normal)
        );
        assert_eq!(monitor.primary, Some(true));
        assert_eq!(monitor.disabled, Some(false));

        let terminal = WindowCandidate {
            app_id: "org.gnome.Terminal",
            class: "gnome-terminal-server",
            title: "Terminal — bash",
        };
        assert_eq!(config.resolve_window(&terminal), Some(first));

        let laptop = MonitorCandidate {
            name: "eDP-1",
            make: "Chimei Innolux Corporation",
            model: "0x15F5",
        };
        assert_eq!(config.resolve_monitor(&laptop), Some(monitor));
    }

    #[test]
    fn empty_rules_config_resolves_to_nothing() {
        let config: RulesConfig = toml::from_str("").expect("empty document is valid");
        assert_eq!(config.window_rules, []);
        assert_eq!(config.monitor_rules, []);
        assert!(
            config
                .resolve_window(&WindowCandidate {
                    app_id: "x",
                    class: "x",
                    title: "x",
                })
                .is_none()
        );
        assert!(
            config
                .resolve_monitor(&MonitorCandidate {
                    name: "x",
                    make: "x",
                    model: "x",
                })
                .is_none()
        );
    }
}
