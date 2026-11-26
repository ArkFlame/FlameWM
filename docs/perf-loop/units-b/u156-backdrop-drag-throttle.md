# u156-backdrop-drag-throttle

Owner files: crates/flamewm-wm-wayland/src/render/dock_backdrop.rs (and the `Decorations`/`DockBacking` fields in render/titlebar.rs if a timestamp field is needed)

Task: Dragging a translucent window changes `BackdropKey.rect` every frame, so `refresh_dock_backdrops` (around dock_backdrop.rs lines 440-480) runs `capture_dock` every frame: scene render, GPU readback, CPU blur of the reduced image, crop. Cut that during a move. Design: when a Window site's cached key differs ONLY in `rect` position (same width and height, same `radius`, `window_radii`, `scale_milli`, `material`) and the previous capture is younger than 50 ms, keep the cached buffer and just let it draw at the new rect (the element is placed from `managed.rect` at draw time) and update nothing else; recapture once the capture is 50 ms old, or when any other key field changes, or when the scene stamp changes for a reason other than the moving window itself. Never apply this to Dock sites. Static scenes must still cost one stamp comparison per frame. Store the capture time as an `Instant` in `DockBacking`; take `now` as a parameter of a pure helper `reuse_while_moving(cached, wanted, now) -> bool` so it is unit-testable without a renderer. Add tests: position-only change within 50 ms reuses, after 50 ms recaptures, size/material/radius change recaptures, Dock site never reuses. Run `cargo test -p flamewm-wm-wayland dock_backdrop` once at the end. Append a docs/perf-loop/LOG.md entry with the expected composition shift (captures per second during a drag: about 60 down to about 20). Put DONE on its own line only after the tests pass.

Rules: keep visuals and features; no python or sleep polling; no git.

OPEN

DONE
