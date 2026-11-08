//! Actions offered for the selected result by the Tab sheet. Pure data; the window runs them.

/// One action on a result, in the order the sheet lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// Open a file or folder with its default application.
    Open,
    /// Start an application.
    Launch,
    /// Show the file's folder.
    Reveal,
    /// Toggle the quick-look preview of the file.
    Preview,
    /// Toggle the details panel of the file.
    Details,
}

/// What the selected result is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    File,
    App,
}

/// Rows the sheet can show, which is the longest action list.
pub const SLOTS: usize = 4;

const FILE_VERBS: &[Verb] = &[Verb::Open, Verb::Reveal, Verb::Preview, Verb::Details];
const APP_VERBS: &[Verb] = &[Verb::Launch];

/// Actions for `subject`. The first one is the default that Enter runs.
#[must_use]
pub fn for_subject(subject: Subject) -> &'static [Verb] {
    match subject {
        Subject::File => FILE_VERBS,
        Subject::App => APP_VERBS,
    }
}

#[must_use]
pub fn label(verb: Verb) -> &'static str {
    match verb {
        Verb::Open => "Open",
        Verb::Launch => "Launch",
        Verb::Reveal => "Show in folder",
        Verb::Preview => "Preview",
        Verb::Details => "Details",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_offer_open_reveal_preview_and_details_in_order() {
        let labels: Vec<&str> = for_subject(Subject::File)
            .iter()
            .map(|verb| label(*verb))
            .collect();
        assert_eq!(labels, ["Open", "Show in folder", "Preview", "Details"]);
        assert!(for_subject(Subject::File).len() <= SLOTS);
    }

    #[test]
    fn apps_offer_only_launch() {
        let apps = for_subject(Subject::App);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0], Verb::Launch);
        assert_eq!(label(apps[0]), "Launch");
    }
}
