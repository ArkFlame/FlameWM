# u161 element building is 27% of drag frame time
Area: compositor. File: crates/flamewm-wm-wayland/src/render/mod.rs (`build_elements_into` / `scene_elements_into`).
Measured (u159 timers, 2 runs): `elements` is 27% of compositor frame time, about 8 ms per frame over 89 frames, while a window drags.
Do: time inside the build (decorations, per-window surface elements, layers) and find what is recomputed every frame although only the dragged window's position changed; a cached element list with the window's offset swapped (as the DRM path already does for the cursor slice) is the likely shape.
Accept: lower `elements` share on the same drag, pixels unchanged.
Status: open.
