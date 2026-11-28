//! Typed build-hygiene contracts for duplicate crate versions in `Cargo.lock`.
//!
//! This module is deliberately pure: it names one dependency entry and records the ranked
//! duplicate families measured from the checked-in lockfile. It never reads a file, resolves a
//! version, or mutates runtime state, so it cannot change product behaviour or visuals.

/// One dependency entry inside a `[[package]]` block of `Cargo.lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockDependency<'a> {
    /// Crate name, written before an optional version.
    pub name: &'a str,
    /// Explicit version, present when the lock disambiguates two versions of one name.
    pub version: Option<&'a str>,
}

/// A duplicate crate family: more than one version of the same name coexists in the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuplicateCrate {
    /// Crate name shared by every version of the family.
    pub name: &'static str,
    /// Version the unification edit removes.
    pub stale_version: &'static str,
    /// Distinct lock packages with a reverse dependency path to `stale_version`.
    pub transitive_weight: usize,
    /// Exact edit that removes the stale version, or `None` when no published release offers one.
    pub unification_edit: Option<&'static str>,
}

/// Parse one dependency line such as ` "x11rb 0.13.2",` or ` "libc",`.
///
/// Returns `None` for package headers, field lines, bracket lines and entries with trailing
/// metadata. Input is a single line; the caller owns iteration and lifetime.
pub fn parse_dependency_line(line: &str) -> Option<LockDependency<'_>> {
    let text = line.trim();
    let text = text
        .strip_prefix('"')?
        .strip_suffix(',')?
        .strip_suffix('"')?;
    let mut fields = text.split_whitespace();
    let name = fields.next()?;
    let version = fields.next();
    if name.is_empty() || fields.next().is_some() {
        return None;
    }
    Some(LockDependency { name, version })
}

/// Five largest duplicate families by transitive reverse weight in the checked-in lockfile.
///
/// Weight is an offline count of distinct packages that reach the stale version by following
/// dependency edges backwards. It is a static graph count, not a build or runtime measurement.
pub const TOP_DUPLICATES: [DuplicateCrate; 5] = [
    DuplicateCrate {
        name: "syn",
        stale_version: "2.0.119",
        transitive_weight: 58,
        unification_edit: Some(
            "third_party/smithay: winit 0.30.0 -> =0.31.0-beta.4 (drops the thiserror 1 chain)",
        ),
    },
    DuplicateCrate {
        name: "miniz_oxide",
        stale_version: "0.8.9",
        transitive_weight: 27,
        unification_edit: None,
    },
    DuplicateCrate {
        name: "hashbrown",
        stale_version: "0.15.5",
        transitive_weight: 23,
        unification_edit: None,
    },
    DuplicateCrate {
        name: "linux-raw-sys",
        stale_version: "0.4.15",
        transitive_weight: 9,
        unification_edit: Some(
            "third_party/smithay: drm 0.14.0 -> 0.15+ (registry check required)",
        ),
    },
    DuplicateCrate {
        name: "rustix",
        stale_version: "0.38.44",
        transitive_weight: 8,
        unification_edit: Some(
            "third_party/smithay: winit 0.30.0 -> =0.31.0-beta.4 (calloop 0.13 chain)",
        ),
    },
];

#[cfg(test)]
mod tests {
    use super::{LockDependency, TOP_DUPLICATES, parse_dependency_line};

    #[test]
    fn parses_qualified_and_unqualified_lock_entries() {
        assert_eq!(
            parse_dependency_line(" \"x11rb 0.13.2\","),
            Some(LockDependency {
                name: "x11rb",
                version: Some("0.13.2")
            })
        );
        assert_eq!(
            parse_dependency_line("\"libc\","),
            Some(LockDependency {
                name: "libc",
                version: None
            })
        );
        assert_eq!(
            parse_dependency_line("  \"rustix 1.1.4\",  "),
            Some(LockDependency {
                name: "rustix",
                version: Some("1.1.4")
            })
        );
    }

    #[test]
    fn rejects_non_dependency_lines() {
        assert_eq!(parse_dependency_line("[[package]]"), None);
        assert_eq!(parse_dependency_line("name = \"x11rb\""), None);
        assert_eq!(parse_dependency_line("dependencies = ["), None);
        assert_eq!(parse_dependency_line("\"]\""), None);
        assert_eq!(
            parse_dependency_line("\"ring 0.17.8 (registry+https://example)\","),
            None
        );
        assert_eq!(parse_dependency_line(""), None);
    }

    #[test]
    fn top_duplicates_are_ranked_and_stable() {
        assert_eq!(TOP_DUPLICATES.len(), 5);
        for pair in TOP_DUPLICATES.windows(2) {
            assert!(pair[0].transitive_weight >= pair[1].transitive_weight);
        }
        let mut names = std::collections::BTreeSet::new();
        for entry in TOP_DUPLICATES {
            assert!(names.insert(entry.name));
            assert!(entry.stale_version.contains('.'));
            assert!(entry.transitive_weight > 0);
        }
    }

    #[test]
    fn unavailable_unifications_are_explicit() {
        let unavailable: Vec<&str> = TOP_DUPLICATES
            .iter()
            .filter(|entry| entry.unification_edit.is_none())
            .map(|entry| entry.name)
            .collect();
        assert_eq!(unavailable, ["miniz_oxide", "hashbrown"]);
        for entry in TOP_DUPLICATES {
            if let Some(edit) = entry.unification_edit {
                assert!(
                    edit.contains("->"),
                    "edit must name the exact version change"
                );
            }
        }
    }
}
