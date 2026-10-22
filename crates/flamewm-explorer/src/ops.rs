//! Pre-flight validation for file operations. Pure: callers pass filesystem facts in,
//! so every rule is unit-testable. A `NoOp` is silently ignored; `Refuse` explains why.
use std::path::{Path, PathBuf};

/// Filesystem facts for one selected source, gathered by the caller with `symlink_metadata`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub exists: bool,
    pub is_dir: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Trash,
    Delete,
    Move,
    Copy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Run the operation on these sources (missing and duplicate entries removed).
    Run(Vec<PathBuf>),
    /// Redundant or empty: do nothing and show no confirmation.
    NoOp,
    /// Invalid: show this message in the status line.
    Refuse(&'static str),
}

/// Validates an operation. `dest` is the target directory for `Move`/`Copy`; `dest_writable`
/// is the caller's access check for that directory.
pub fn validate(kind: Kind, sources: &[Source], dest: Option<&Path>, dest_writable: bool) -> Verdict {
    let mut seen: Vec<&Path> = Vec::new();
    let mut kept: Vec<PathBuf> = Vec::new();
    for source in sources {
        if !source.exists || seen.contains(&source.path.as_path()) {
            continue;
        }
        seen.push(&source.path);
        kept.push(source.path.clone());
    }
    if kept.is_empty() {
        return Verdict::NoOp;
    }
    let Some(dest) = dest else {
        return Verdict::Run(kept);
    };
    if matches!(kind, Kind::Move | Kind::Copy) && !dest_writable {
        return Verdict::Refuse("Destination is read-only");
    }
    let mut runnable = Vec::new();
    for source in sources.iter().filter(|s| kept.contains(&s.path)) {
        if !runnable.contains(&source.path) {
            // Moving an item into the folder it already lives in changes nothing.
            if kind == Kind::Move && source.path.parent() == Some(dest) {
                continue;
            }
            if source.is_dir && (dest == source.path || dest.starts_with(&source.path)) {
                return Verdict::Refuse("Cannot place a folder inside itself");
            }
            runnable.push(source.path.clone());
        }
    }
    if runnable.is_empty() {
        Verdict::NoOp
    } else {
        Verdict::Run(runnable)
    }
}

/// Shortens a directory for display: the home directory becomes `~`.
pub fn display_location(dir: &Path, home: &Path) -> String {
    match dir.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => dir.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(path: &str, is_dir: bool) -> Source {
        Source { path: PathBuf::from(path), exists: true, is_dir }
    }

    #[test]
    fn empty_and_missing_sources_are_noops() {
        assert_eq!(validate(Kind::Trash, &[], None, true), Verdict::NoOp);
        let gone = Source { path: "/x/a".into(), exists: false, is_dir: false };
        assert_eq!(validate(Kind::Delete, &[gone], None, true), Verdict::NoOp);
    }

    #[test]
    fn duplicate_sources_are_collapsed() {
        let sources = [src("/x/a", false), src("/x/a", false), src("/x/b", false)];
        assert_eq!(
            validate(Kind::Trash, &sources, None, true),
            Verdict::Run(vec!["/x/a".into(), "/x/b".into()])
        );
    }

    #[test]
    fn move_into_same_parent_is_noop_but_copy_is_kept() {
        let sources = [src("/x/a", false)];
        assert_eq!(validate(Kind::Move, &sources, Some(Path::new("/x")), true), Verdict::NoOp);
        // Copy into the same folder is allowed; the caller keeps both names.
        assert_eq!(
            validate(Kind::Copy, &sources, Some(Path::new("/x")), true),
            Verdict::Run(vec!["/x/a".into()])
        );
    }

    #[test]
    fn folder_into_itself_or_descendant_is_refused() {
        let folder = [src("/x/dir", true)];
        assert_eq!(
            validate(Kind::Move, &folder, Some(Path::new("/x/dir")), true),
            Verdict::Refuse("Cannot place a folder inside itself")
        );
        assert_eq!(
            validate(Kind::Copy, &folder, Some(Path::new("/x/dir/sub")), true),
            Verdict::Refuse("Cannot place a folder inside itself")
        );
        // A file whose name merely prefixes the destination is not a descendant.
        let file = [src("/x/dir", false)];
        assert_eq!(validate(Kind::Move, &file, Some(Path::new("/x/dir2")), true), Verdict::Run(vec!["/x/dir".into()]));
    }

    #[test]
    fn read_only_destination_is_refused_for_transfers_only() {
        let sources = [src("/x/a", false)];
        assert_eq!(
            validate(Kind::Copy, &sources, Some(Path::new("/ro")), false),
            Verdict::Refuse("Destination is read-only")
        );
        assert_eq!(validate(Kind::Trash, &sources, None, false), Verdict::Run(vec!["/x/a".into()]));
    }

    #[test]
    fn display_location_shortens_home() {
        let home = Path::new("/home/u");
        assert_eq!(display_location(Path::new("/home/u"), home), "~");
        assert_eq!(display_location(Path::new("/home/u/Docs"), home), "~/Docs");
        assert_eq!(display_location(Path::new("/home/ux"), home), "/home/ux");
        assert_eq!(display_location(Path::new("/media/d"), home), "/media/d");
    }
}
