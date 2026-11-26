# u151-terminal-dirty-lines

DONE

Owner files: crates/flamewm-terminal*/src

Task: Audit the terminal render path: confirm dirty-line only redraw, batched PTY reads and no redraw when output is unchanged. Fix the largest wasteful share found.

Rules: keep visuals and features; no tests of absolute numbers; no python or sleep polling; add or keep unit tests; report composition shift and files touched.
