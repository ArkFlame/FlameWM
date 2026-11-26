# u151-terminal-dirty-lines


Owner files: crates/flamewm-terminal*/src

Task: Audit the terminal render path: confirm dirty-line only redraw, batched PTY reads and no redraw when output is unchanged. Fix the largest wasteful share found.

Rules: keep visuals and features; no tests of absolute numbers; no python or sleep polling; add or keep unit tests; report composition shift and files touched.

Reopened: the first pass changed nothing and logged nothing. Read the terminal crate's render and PTY code, name the ranked wasteful paths with file:line (full-grid redraws, per-frame allocations, unbatched PTY reads, redraw on unchanged output), fix the largest one, and append a docs/perf-loop/LOG.md entry. If it is already optimal, write file:line evidence for each checklist item instead of marking DONE silently. Put DONE on its own line only after that.

DONE

Result: audited, already optimal. Dirty-line caching and unchanged-row reuse: crates/flamewm-terminal/src/view.rs and app/grid.rs; PTY reads batched: session.rs. No code change.
