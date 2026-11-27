# Unit p41-wm-wayland-drm-fallback
STATUS: OPEN

Crate flamewm-wm-wayland: backend selection (DRM, nested, winit, X11): when the preferred backend fails to init, fall through to the next with one failure record (stage, cause, blast radius); never abort while any backend can work. Audit main.rs backend ladder; add a fault test where cheap.
