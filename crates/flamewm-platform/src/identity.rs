use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppIdentityInput {
    pub desktop_id: String,
    pub startup_wm_class: String,
    pub wm_class: String,
    pub wm_instance: String,
}

#[must_use]
pub fn normalize_identity(raw: &str) -> String {
    raw.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

#[must_use]
pub fn resolve_identity(input: &AppIdentityInput) -> String {
    [
        &input.desktop_id,
        &input.startup_wm_class,
        &input.wm_class,
        &input.wm_instance,
    ]
    .into_iter()
    .find(|candidate| !candidate.is_empty())
    .map_or_else(|| "unknown".to_owned(), |candidate| normalize_identity(candidate))
}

#[must_use]
pub fn merge_pinned_running(pinned: &[String], running: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    pinned
        .iter()
        .chain(running)
        .map(|value| normalize_identity(value))
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_order_never_uses_window_title() {
        let input = AppIdentityInput {
            desktop_id: String::new(),
            startup_wm_class: "Firefox".to_owned(),
            wm_class: "other".to_owned(),
            wm_instance: "Navigator".to_owned(),
        };
        assert_eq!(resolve_identity(&input), "firefox");
    }

    #[test]
    fn pinned_running_merge_is_stable_and_case_insensitive() {
        let pinned = vec!["Firefox".to_owned(), "code.desktop".to_owned()];
        let running = vec!["firefox".to_owned(), "Terminal".to_owned()];
        assert_eq!(
            merge_pinned_running(&pinned, &running),
            vec!["firefox", "code.desktop", "terminal"]
        );
    }
}
