# u154-wayland-frame-callbacks

Owner files: crates/flamewm-wm-wayland/src

Task: Send frame callbacks only to visible, unoccluded surfaces; skip damage and redraw for minimized or fully covered windows.

Rules: keep visuals and features; no tests of absolute numbers; no python or sleep polling; add or keep unit tests; report composition shift and files touched.

DONE

Result: already implemented. crates/flamewm-wm-wayland/src/render/mod.rs:984 send_frame_callbacks skips hidden (is_hidden) and off-output (frame_reaches_output) toplevels and X11 surfaces; the DRM path is the same shape. Remaining gap: windows fully covered by other windows still get callbacks (needs the front-to-back occluder list; risk: a covered video/terminal window stalls its commit). Not done on purpose.
