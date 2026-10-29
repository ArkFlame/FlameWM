//! Prefix modes of the search box. `>` or `/` lists commands, `@` lists folder scopes, and any
//! other text searches files. A `/` starts a command only when nothing follows it or the word after
//! it matches a command, so an absolute path such as `/usr/bin` still searches files.

use std::path::Path;

/// One entry of the command list. `id` is what the window matches on to run the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}

pub const COMMANDS: [CommandSpec; 6] = [
    CommandSpec {
        id: "help",
        label: "Show shortcuts",
        hint: "F1",
    },
    CommandSpec {
        id: "duplicates",
        label: "Find duplicate files",
        hint: "",
    },
    CommandSpec {
        id: "setup",
        label: "Index setup",
        hint: "",
    },
    CommandSpec {
        id: "grid",
        label: "Toggle grid view",
        hint: "Ctrl G",
    },
    CommandSpec {
        id: "home",
        label: "Open home folder",
        hint: "",
    },
    CommandSpec {
        id: "clear",
        label: "Clear search",
        hint: "",
    },
];

/// Folders offered by the `@` menu, when they exist under the home folder.
const SCOPE_FOLDERS: [&str; 7] = [
    "Desktop",
    "Documents",
    "Downloads",
    "Music",
    "Pictures",
    "Videos",
    "Projects",
];

/// One `@` scope choice: its label and the query text it writes into the search box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Files,
    Commands(String),
    Scopes(String),
}

/// Mode for the raw search text, with the filter the prefix leaves after it (lowercase, trimmed).
#[must_use]
pub fn mode(query: &str) -> Mode {
    if let Some(rest) = query.strip_prefix('>') {
        return Mode::Commands(filter_text(rest));
    }
    if let Some(rest) = query.strip_prefix('/') {
        if rest.is_empty() || !command_matches(&filter_text(rest)).is_empty() {
            return Mode::Commands(filter_text(rest));
        }
    }
    if let Some(rest) = query.strip_prefix('@') {
        return Mode::Scopes(filter_text(rest));
    }
    Mode::Files
}

fn filter_text(rest: &str) -> String {
    rest.trim().to_lowercase()
}

fn matches_all(label: &str, filter: &str) -> bool {
    let label = label.to_lowercase();
    filter.split_whitespace().all(|term| label.contains(term))
}

/// Commands whose label contains every word of `filter`, in list order.
#[must_use]
pub fn command_matches(filter: &str) -> Vec<&'static CommandSpec> {
    COMMANDS
        .iter()
        .filter(|command| matches_all(command.label, filter))
        .collect()
}

/// Scope choices for `home` whose label contains every word of `filter`.
#[must_use]
pub fn scope_choices(home: &str, filter: &str) -> Vec<Choice> {
    let mut all = vec![
        Choice {
            label: "Everything".to_owned(),
            query: String::new(),
        },
        Choice {
            label: "Home".to_owned(),
            query: "in:~ ".to_owned(),
        },
    ];
    for folder in SCOPE_FOLDERS {
        if Path::new(home).join(folder).is_dir() {
            all.push(Choice {
                label: folder.to_owned(),
                query: format!("in:~/{folder} "),
            });
        }
    }
    all.into_iter()
        .filter(|choice| matches_all(&choice.label, filter))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_pick_the_mode_and_leave_the_filter() {
        assert_eq!(mode("> Show"), Mode::Commands("show".to_owned()));
        assert_eq!(mode("/dup"), Mode::Commands("dup".to_owned()));
        assert_eq!(mode("/"), Mode::Commands(String::new()));
        assert_eq!(mode("@ doc"), Mode::Scopes("doc".to_owned()));
        assert_eq!(mode("report"), Mode::Files);
        assert_eq!(mode("/usr/bin"), Mode::Files);
        assert_eq!(mode("/usr"), Mode::Files);
    }

    #[test]
    fn command_filter_needs_every_word() {
        let all = command_matches("");
        assert_eq!(all.len(), COMMANDS.len());
        let grid: Vec<&str> = command_matches("grid view").iter().map(|c| c.id).collect();
        assert_eq!(grid, ["grid"]);
        assert!(command_matches("grid nonsense").is_empty());
    }

    #[test]
    fn scope_choices_write_home_relative_query_text() {
        let home = std::env::temp_dir().join(format!("flamewm-palette-{}", std::process::id()));
        std::fs::create_dir_all(home.join("Documents")).expect("temp home");
        let home_text = home.to_string_lossy().into_owned();
        let all = scope_choices(&home_text, "");
        assert_eq!(all[0].query, "");
        assert_eq!(all[1].query, "in:~ ");
        let docs = scope_choices(&home_text, "docu");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].query, "in:~/Documents ");
        // Folders that do not exist are not offered.
        assert!(scope_choices(&home_text, "music").is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }
}
