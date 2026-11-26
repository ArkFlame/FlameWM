# FlameWM patch to smithay 0.7.0

Upstream: crates.io `smithay` 0.7.0, unmodified except `src/xwayland/xwm/mod.rs`
(search for "FlameWM patch").

Why: the XWM reparents every managed X11 window into a frame created with the root depth (24). An ARGB
(depth 32) client is then flattened onto that opaque frame, and XWayland marks the whole surface opaque,
so a translucent FlameWM app can never show the scene beneath it. The patch creates the frame with the
client's visual, depth and a matching colormap when the client is depth 32; other windows are unchanged.

Drop this patch when upstream smithay creates ARGB frames for ARGB clients.
