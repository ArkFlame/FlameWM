# u153-explorer-async-audit

Owner files: crates/flamewm-explorer/src

Task: Find any filesystem call (readdir, stat, icons, thumbnails, size calc) still on the UI thread and move it to a cancelable worker with results delivered by event.

Rules: keep visuals and features; no tests of absolute numbers; no python or sleep polling; add or keep unit tests; report composition shift and files touched.
