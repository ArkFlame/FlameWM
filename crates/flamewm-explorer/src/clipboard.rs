//! Pure paste decisions for the explorer's system clipboard integration.
//! The system CLIPBOARD (other apps and this explorer) is authoritative when it holds
//! files; the in-app clipboard is the fallback. No I/O happens here.
use std::path::PathBuf;

/// A file list and whether it is a cut (`true`) or a copy (`false`).
pub type FileClip = (Vec<PathBuf>, bool);

/// Chooses what Paste uses. A system file list wins, including copies made by other apps.
/// A system cut that this explorer already moved (`spent`) is ignored so it cannot be moved twice.
/// The in-app clipboard is used only when the system holds no usable file list.
#[must_use]
pub fn resolve_paste(
    system: Option<FileClip>,
    in_app: Option<&FileClip>,
    spent: Option<&[PathBuf]>,
) -> Option<FileClip> {
    if let Some((paths, cut)) = system.filter(|(paths, _)| !paths.is_empty()) {
        let already_moved = cut && spent == Some(paths.as_slice());
        if !already_moved {
            return Some((paths, cut));
        }
    }
    in_app.cloned()
}

/// Updates the in-app clipboard after a cut batch finished. A cut that moved everything
/// clears the clipboard; a partial move keeps only the failed sources for a safe retry.
/// Clipboards that hold other content, or copies, are left untouched.
#[must_use]
pub fn settle_cut(
    clipboard: Option<FileClip>,
    sources: &[PathBuf],
    failed: Vec<PathBuf>,
) -> Option<FileClip> {
    match clipboard {
        Some((paths, true)) if paths == sources => (!failed.is_empty()).then_some((failed, true)),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn system_files_win_over_in_app_copy() {
        let system = Some((vec![p("/ext/a")], false));
        let app = (vec![p("/app/b")], false);
        assert_eq!(
            resolve_paste(system, Some(&app), None),
            Some((vec![p("/ext/a")], false))
        );
    }

    #[test]
    fn system_cut_is_honoured() {
        let system = Some((vec![p("/ext/a")], true));
        assert_eq!(
            resolve_paste(system, None, None),
            Some((vec![p("/ext/a")], true))
        );
    }

    #[test]
    fn falls_back_to_in_app_when_system_is_none() {
        let app = (vec![p("/app/b")], true);
        assert_eq!(resolve_paste(None, Some(&app), None), Some(app.clone()));
    }

    #[test]
    fn falls_back_to_in_app_when_system_list_is_empty() {
        let app = (vec![p("/app/b")], false);
        assert_eq!(
            resolve_paste(Some((vec![], true)), Some(&app), None),
            Some(app.clone())
        );
    }

    #[test]
    fn nothing_to_paste_when_both_empty() {
        assert_eq!(resolve_paste(None, None, None), None);
    }

    #[test]
    fn spent_system_cut_is_not_pasted_again() {
        let spent = vec![p("/ext/a")];
        let system = Some((vec![p("/ext/a")], true));
        assert_eq!(resolve_paste(system, None, Some(&spent)), None);
    }

    #[test]
    fn spent_check_does_not_hide_a_copy_or_a_new_cut() {
        let spent = vec![p("/ext/a")];
        // Same paths as a copy are still pasteable.
        assert_eq!(
            resolve_paste(Some((vec![p("/ext/a")], false)), None, Some(&spent)),
            Some((vec![p("/ext/a")], false))
        );
        // A different cut is not the spent one.
        assert_eq!(
            resolve_paste(Some((vec![p("/ext/c")], true)), None, Some(&spent)),
            Some((vec![p("/ext/c")], true))
        );
    }

    #[test]
    fn spent_cut_falls_back_to_in_app_retry() {
        let spent = vec![p("/ext/a")];
        let app = (vec![p("/ext/a2")], true);
        let system = Some((vec![p("/ext/a")], true));
        assert_eq!(
            resolve_paste(system, Some(&app), Some(&spent)),
            Some(app.clone())
        );
    }

    #[test]
    fn settle_cut_clears_after_full_move() {
        let clip = Some((vec![p("/a"), p("/b")], true));
        assert_eq!(settle_cut(clip, &[p("/a"), p("/b")], vec![]), None);
    }

    #[test]
    fn settle_cut_keeps_only_failed_sources_after_partial_move() {
        let clip = Some((vec![p("/a"), p("/b")], true));
        assert_eq!(
            settle_cut(clip, &[p("/a"), p("/b")], vec![p("/b")]),
            Some((vec![p("/b")], true))
        );
    }

    #[test]
    fn settle_cut_leaves_other_clipboards_alone() {
        let copy = Some((vec![p("/a")], false));
        assert_eq!(settle_cut(copy.clone(), &[p("/a")], vec![]), copy);
        let other = Some((vec![p("/x")], true));
        assert_eq!(settle_cut(other.clone(), &[p("/a")], vec![]), other);
    }
}
