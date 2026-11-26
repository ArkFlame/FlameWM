# u154-wayland-frame-callbacks

Owner files: crates/flamewm-wm-wayland/src

Task: Send frame callbacks only to visible, unoccluded surfaces; skip damage and redraw for minimized or fully covered windows.

Rules: keep visuals and features; no tests of absolute numbers; no python or sleep polling; add or keep unit tests; report composition shift and files touched.
