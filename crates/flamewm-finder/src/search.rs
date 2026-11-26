//! Query parsing, matching and ranking over the file index. Pure: no I/O and no clock reads, so every
//! rule is covered by unit tests.
//!
//! Syntax: free words are name/path terms that must all match. `type:ext` keeps files with that
//! extension; `in:folder` keeps paths under that folder (`~` expands to the home folder).

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use crate::index::{Entry, Index};

/// Matches kept for display. The total count is still reported.
pub const DISPLAY_LIMIT: usize = 200;
/// Fuzzy (subsequence) matching needs at least this many characters, or it matches almost anything.
const FUZZY_MIN_CHARS: usize = 3;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Query {
    /// Lowercase terms; each one must match the file name or the path.
    pub terms: Vec<String>,
    /// Lowercase extensions from `type:` filters, without the dot.
    pub types: Vec<String>,
    /// Folder from an `in:` filter, with `~` expanded.
    pub scope: Option<String>,
}

impl Query {
    #[must_use]
    pub fn parse(input: &str, home: &str) -> Self {
        let mut query = Self::default();
        for token in input.split_whitespace() {
            if let Some(ext) = token.to_lowercase().strip_prefix("type:") {
                let ext = ext.trim_start_matches('.');
                if !ext.is_empty() {
                    query.types.push(ext.to_owned());
                }
            } else if let Some(folder) = token.strip_prefix("in:") {
                let folder = match folder.strip_prefix('~') {
                    Some(rest) => format!("{}{rest}", home.trim_end_matches('/')),
                    None => folder.to_owned(),
                };
                let folder = folder.trim_end_matches('/').to_owned();
                if !folder.is_empty() {
                    query.scope = Some(folder);
                }
            } else {
                let term = token.to_lowercase();
                if !term.is_empty() {
                    query.terms.push(term);
                }
            }
        }
        query
    }

    /// True when the query has no terms and no filters, which shows the recent-files view.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.types.is_empty() && self.scope.is_none()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Hits {
    /// Total matches before the display limit.
    pub total: usize,
    /// Index positions in rank order, at most [`DISPLAY_LIMIT`].
    pub indices: Vec<usize>,
}

/// Score of one term against a lowercase path, or `None` when it does not match.
#[must_use]
pub fn term_score(lower: &str, name_at: usize, term: &str) -> Option<i64> {
    let name = &lower[name_at..];
    if name == term {
        return Some(1000);
    }
    if name.starts_with(term) {
        let extra = (name.len() - term.len()).min(200);
        return Some(800 - i64::try_from(extra).unwrap_or(200));
    }
    if let Some(pos) = name.find(term) {
        return Some(600 - i64::try_from(pos.min(200)).unwrap_or(200));
    }
    if lower.contains(term) {
        return Some(300);
    }
    if term.chars().count() >= FUZZY_MIN_CHARS
        && let Some(gap) = subsequence_gap(name, term)
    {
        return Some(100 - i64::try_from(gap.min(90)).unwrap_or(90));
    }
    None
}

/// Number of characters skipped inside `haystack` while matching `needle` as a subsequence.
///
/// The fuzzy path runs for every entry that misses the substring tests, so it is the hot part of a
/// keystroke over the whole index. Plain ASCII names and terms are compared byte by byte, which
/// skips UTF-8 decoding; other text keeps the character walk so the match stays exact.
fn subsequence_gap(haystack: &str, needle: &str) -> Option<usize> {
    if haystack.is_ascii() && needle.is_ascii() {
        return ascii_subsequence_gap(haystack.as_bytes(), needle.as_bytes());
    }
    let mut wanted = needle.chars().peekable();
    let mut skipped = 0;
    let mut matched = 0;
    for ch in haystack.chars() {
        match wanted.peek() {
            Some(&next) if next == ch => {
                wanted.next();
                matched += 1;
            }
            Some(_) => skipped += 1,
            None => break,
        }
    }
    (wanted.peek().is_none() && matched > 0).then_some(skipped)
}

/// Same walk as [`subsequence_gap`] for ASCII bytes: one byte is one character.
fn ascii_subsequence_gap(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let mut at = 0;
    let mut matched = 0;
    let mut skipped = 0;
    for &byte in haystack {
        match needle.get(at) {
            Some(&next) if next == byte => {
                at += 1;
                matched += 1;
            }
            Some(_) => skipped += 1,
            None => break,
        }
    }
    (at == needle.len() && matched > 0).then_some(skipped)
}

fn recency_bonus(modified: i64, now: i64) -> i64 {
    let age = now.saturating_sub(modified);
    if age < 86_400 {
        40
    } else if age < 7 * 86_400 {
        20
    } else {
        0
    }
}

fn passes_filters(entry: &Entry, query: &Query) -> bool {
    if !query.types.is_empty() {
        if entry.is_dir {
            return false;
        }
        let ext = entry.lower.get(entry.name_at..).and_then(|name| {
            name.rfind('.')
                .filter(|&dot| dot > 0)
                .map(|dot| &name[dot + 1..])
        });
        match ext {
            Some(ext) if query.types.iter().any(|wanted| wanted == ext) => {}
            _ => return false,
        }
    }
    if let Some(scope) = &query.scope {
        let inside =
            entry.path.starts_with(scope.as_str()) && entry.path[scope.len()..].starts_with('/');
        if !inside {
            return false;
        }
    }
    true
}

/// Scores one entry, or `None` when any term or filter excludes it.
#[must_use]
pub fn score(entry: &Entry, query: &Query, now: i64) -> Option<i64> {
    if !passes_filters(entry, query) {
        return None;
    }
    let mut total = 0;
    for term in &query.terms {
        total += term_score(&entry.lower, entry.name_at, term)?;
    }
    let depth = i64::from(entry.depth.min(40));
    Some(total + recency_bonus(entry.modified, now) - depth)
}

/// One ranked match. Orders best-first by score, then by path, so a bounded heap can drop the
/// worst match without collecting and sorting every candidate first.
#[derive(PartialEq, Eq)]
struct Candidate<'a> {
    score: i64,
    path: &'a str,
    index: usize,
}

impl Ord for Candidate<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.path.cmp(self.path))
    }
}

impl PartialOrd for Candidate<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Ranks the index for `query`. An empty query returns the recent-files view.
#[must_use]
pub fn rank(index: &Index, query: &Query, now: i64) -> Hits {
    if query.is_empty() {
        // Recently used documents lead, then the newest files that are not already listed.
        let mut indices = index.docs.clone();
        indices.extend(
            index
                .recent
                .iter()
                .copied()
                .filter(|i| !index.docs.contains(i)),
        );
        return Hits {
            total: indices.len(),
            indices,
        };
    }
    // Keep only the display limit in a min-heap, so a query over the whole index allocates a few
    // kilobytes instead of one tuple per entry, and sorts the kept matches only.
    let mut best: BinaryHeap<Reverse<Candidate<'_>>> = BinaryHeap::with_capacity(DISPLAY_LIMIT + 1);
    let mut total = 0usize;
    for (index, entry) in index.entries.iter().enumerate() {
        let Some(value) = score(entry, query, now) else {
            continue;
        };
        total += 1;
        let candidate = Candidate {
            score: value,
            path: entry.path.as_str(),
            index,
        };
        if best.len() < DISPLAY_LIMIT {
            best.push(Reverse(candidate));
        } else if best.peek().is_some_and(|Reverse(worst)| candidate > *worst) {
            best.pop();
            best.push(Reverse(candidate));
        }
    }
    let mut scored: Vec<Candidate<'_>> = best.into_iter().map(|Reverse(item)| item).collect();
    scored.sort_unstable_by(|a, b| b.cmp(a));
    Hits {
        total,
        indices: scored.into_iter().map(|item| item.index).collect(),
    }
}

/// Byte ranges of every term occurrence in the name, sorted and merged, for highlighting.
#[must_use]
pub fn highlight_ranges(name: &str, terms: &[String]) -> Vec<(usize, usize)> {
    let lower = name.to_lowercase();
    if lower.len() != name.len() {
        return Vec::new();
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms.iter().filter(|term| !term.is_empty()) {
        let mut from = 0;
        while let Some(found) = lower.get(from..).and_then(|rest| rest.find(term.as_str())) {
            let start = from + found;
            let end = start + term.len();
            ranges.push((start, end));
            from = end;
        }
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Splits the name into alternating plain and highlighted pieces. Concatenating the pieces gives
/// back the name.
#[must_use]
pub fn segments(name: &str, terms: &[String]) -> Vec<(String, bool)> {
    let mut pieces = Vec::new();
    let mut at = 0;
    for (start, end) in highlight_ranges(name, terms) {
        if start > at {
            pieces.push((name[at..start].to_owned(), false));
        }
        pieces.push((name[start..end].to_owned(), true));
        at = end;
    }
    if at < name.len() {
        pieces.push((name[at..].to_owned(), false));
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000_000;

    fn entry(path: &str, is_dir: bool, modified: i64) -> Entry {
        Entry::new(path.to_owned(), is_dir, 1, modified)
    }

    fn index(entries: Vec<Entry>) -> Index {
        Index::new(entries, false)
    }

    #[test]
    fn parse_splits_filters_from_terms() {
        let query = Query::parse("Invoice type:.PDF in:~/Work/ 2024", "/home/u");
        assert_eq!(query.terms, ["invoice", "2024"]);
        assert_eq!(query.types, ["pdf"]);
        assert_eq!(query.scope.as_deref(), Some("/home/u/Work"));
        assert!(Query::parse("   ", "/h").is_empty());
    }

    #[test]
    fn exact_name_outranks_prefix_then_substring() {
        let idx = index(vec![
            entry("/h/a/notes-old.txt", false, 0),
            entry("/h/a/notes.txt", false, 0),
            entry("/h/a/my-notes", false, 0),
        ]);
        let hits = rank(&idx, &Query::parse("notes", "/h"), NOW);
        let names: Vec<&str> = hits
            .indices
            .iter()
            .map(|&i| idx.entries[i].name())
            .collect();
        assert_eq!(names, ["notes.txt", "notes-old.txt", "my-notes"]);
    }

    #[test]
    fn every_term_must_match() {
        let idx = index(vec![
            entry("/h/report-2024.pdf", false, 0),
            entry("/h/report-2023.pdf", false, 0),
        ]);
        let hits = rank(&idx, &Query::parse("report 2024", "/h"), NOW);
        assert_eq!(hits.total, 1);
        assert_eq!(idx.entries[hits.indices[0]].name(), "report-2024.pdf");
    }

    #[test]
    fn path_terms_match_folder_names() {
        let idx = index(vec![entry("/h/projects/x/readme.md", false, 0)]);
        let hits = rank(&idx, &Query::parse("projects", "/h"), NOW);
        assert_eq!(hits.total, 1);
    }

    #[test]
    fn fuzzy_subsequence_needs_three_characters() {
        let idx = index(vec![entry("/h/quarterly-budget.xlsx", false, 0)]);
        assert_eq!(rank(&idx, &Query::parse("qbud", "/h"), NOW).total, 1);
        assert_eq!(rank(&idx, &Query::parse("qz", "/h"), NOW).total, 0);
    }

    #[test]
    fn type_filter_keeps_files_with_that_extension_only() {
        let idx = index(vec![
            entry("/h/a.pdf", false, 0),
            entry("/h/a.txt", false, 0),
            entry("/h/pdf", true, 0),
        ]);
        let hits = rank(&idx, &Query::parse("type:pdf", "/h"), NOW);
        assert_eq!(hits.total, 1);
        assert_eq!(idx.entries[hits.indices[0]].name(), "a.pdf");
    }

    #[test]
    fn scope_filter_keeps_only_paths_inside_the_folder() {
        let idx = index(vec![
            entry("/h/work/a.txt", false, 0),
            entry("/h/workshop/b.txt", false, 0),
        ]);
        let hits = rank(&idx, &Query::parse("in:/h/work a", "/h"), NOW);
        assert_eq!(hits.total, 1);
        assert_eq!(idx.entries[hits.indices[0]].path, "/h/work/a.txt");
    }

    #[test]
    fn recent_recency_breaks_equal_name_scores() {
        let idx = index(vec![
            entry("/h/old/data.csv", false, NOW - 90 * 86_400),
            entry("/h/new/data.csv", false, NOW - 60),
        ]);
        let hits = rank(&idx, &Query::parse("data", "/h"), NOW);
        assert_eq!(idx.entries[hits.indices[0]].path, "/h/new/data.csv");
    }

    #[test]
    fn empty_query_returns_recent_view() {
        let idx = index(vec![entry("/h/a", false, 5), entry("/h/b", false, 9)]);
        let hits = rank(&idx, &Query::default(), NOW);
        assert_eq!(idx.entries[hits.indices[0]].name(), "b");
    }

    #[test]
    fn recent_documents_lead_the_empty_view_without_repeats() {
        let mut idx = index(vec![
            entry("/h/a", false, 5),
            entry("/h/b", false, 9),
            entry("/h/doc.odt", false, 1),
        ]);
        idx.set_docs(vec![
            "/h/doc.odt".to_owned(),
            "/h/missing.txt".to_owned(),
            "/h/a".to_owned(),
        ]);
        let hits = rank(&idx, &Query::default(), NOW);
        let names: Vec<&str> = hits
            .indices
            .iter()
            .map(|&i| idx.entries[i].name())
            .collect();
        assert_eq!(names, ["doc.odt", "a", "b"]);
        assert_eq!(hits.total, 3);
    }

    #[test]
    fn display_limit_caps_rows_but_not_total() {
        let entries = (0..DISPLAY_LIMIT + 5)
            .map(|i| entry(&format!("/h/file{i}.txt"), false, 0))
            .collect();
        let hits = rank(&index(entries), &Query::parse("file", "/h"), NOW);
        assert_eq!(hits.total, DISPLAY_LIMIT + 5);
        assert_eq!(hits.indices.len(), DISPLAY_LIMIT);
    }

    #[test]
    fn highlight_marks_every_term_occurrence() {
        assert_eq!(
            highlight_ranges("Quarterly Report.pdf", &["report".to_owned()]),
            vec![(10, 16)]
        );
        // Two terms, one repeated: each occurrence is marked and overlaps merge.
        let ranges = highlight_ranges(
            "report 2024 report",
            &["report".to_owned(), "2024".to_owned()],
        );
        assert_eq!(ranges, vec![(0, 6), (7, 11), (12, 18)]);
        assert!(highlight_ranges("abc", &["zz".to_owned()]).is_empty());
    }

    #[test]
    fn segments_alternate_and_rebuild_the_name() {
        let pieces = segments(
            "Quarterly Report 2024.pdf",
            &["report".to_owned(), "24".to_owned()],
        );
        assert_eq!(
            pieces,
            vec![
                ("Quarterly ".to_owned(), false),
                ("Report".to_owned(), true),
                (" 20".to_owned(), false),
                ("24".to_owned(), true),
                (".pdf".to_owned(), false),
            ]
        );
        let rebuilt: String = pieces.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(rebuilt, "Quarterly Report 2024.pdf");
    }
}
