# Dock backdrop: translucent surface over windows

Status: X11 live path implemented (93448de1). Wayland follow-up implemented (`render/dock_backdrop.rs`); see Wayland below for the design and what is verified.

## Bug

The dock pill is translucent, but windows beneath it are not visible through it. It blends over
the desktop layer only, so a maximised window under the dock reads as wallpaper.

## Measured facts (code, file:line on origin/main 549fb728)

| # | Fact | Where |
|---|---|---|
| F1 | Dock backdrop is captured once per `(rect, opacity, blur)` key, desktop-only. | `crates/flamewm-shell/src/app/dock.rs:446-470` (`apply_dock_backdrop`) calls `runtime.desktop_backdrop`. |
| F2 | Taskbar uses the same desktop-only source. `surface_backdrop.rs` says shell surfaces "never read the root". | `crates/flamewm-shell/src/surface_backdrop.rs:1-5`, `crates/flamewm-shell/src/app/panels.rs:345-365`. |
| F3 | X11 desktop-only capture: crop of a cached wallpaper canvas, no window content. | `crates/flamewm-render-x11/src/surface_controller/backdrop_shape.rs:114-131`. |
| F4 | X11 already has a live path for Normal windows: the WM redirects frames through Composite and lists them in `_FLAMEWM_REDIRECTED_FRAMES`. Frames are composed server-side from named pixmaps, refreshed on root stacking/geometry and XDamage. The root is never read. | `crates/flamewm-render-x11/src/app/live_backdrop.rs:1-13`, `crates/flamewm-render-x11/src/backdrop/desktop.rs:9-14`. |
| F5 | The live path is process-agnostic. It needs only the display, the surface window and the root properties, so the shell's controller can call it. | `live_backdrop.rs` `LiveScene::compose(display, screen, window, rect, blur)`. |
| F6 | `LiveScene::new` calls `XSelectInput(root, SUBSTRUCTURE_NOTIFY)`, which replaces the client's root mask. `LiveScene::release` clears it with mask 0. Both affect the shell's root watch. | `live_backdrop.rs` `new` and `release`. |
| F7 | The controller already records root child map/unmap/configure/destroy (ignoring its own windows). `take_root_changed` has no caller in the shell. `watch_root_changes` is called by lock and osd only. | `crates/flamewm-render-x11/src/surface_controller/pump.rs:39-50`, `window_state.rs:225-238`, `crates/flamewm-ui-x11/src/surface_runtime.rs:349-364`. |
| F8 | Shell tick calls `sync_dock` on every tick. Tick is event/timer driven, not polled ("Steady state never polls"). | `crates/flamewm-shell/src/app/timers.rs:158`. |
| F9 | Material: opacity 100 means blur radius 0. Opaque material hides whatever is beneath. | `crates/flamewm-platform/src/settings/slots.rs:97-105`. |
| F10 | Wayland compositor draws X11 (Xwayland) unmanaged surfaces. Struts (docks) are excluded from its desktop blur ("not glass"). The compositor's blur is desktop-only. | `crates/flamewm-wm-wayland/src/render/mod.rs:228-241`, `render/backdrop.rs:1-5`. |
| F11 | Wayland windows are not X windows, so the X root does not contain them. The X live path cannot see them. | Structural: Xwayland root holds X clients only. |
| F12 | `_FLAMEWM_REDIRECTED_FRAMES` is set only by the X11 WM (`flamewm-wm-x11`). Under Wayland it is absent, so the live path returns `None` and the old desktop path runs. | `crates/flamewm-wm-x11/src/atoms.rs:71,140`. |

Not measured: no runtime screenshot or CPU number exists for this change yet. See Verification.

## Design

### X11 (implemented)

1. Opaque material (opacity 100, blur 0) takes no capture and clears any backdrop (F9).
2. For a translucent dock, the shell asks the controller for `live_backdrop(surface, rect, opacity, blur)`.
   The controller lazily builds one `LiveScene` and returns `Pixels(words)` or `Unchanged`. It returns
   `None` when no frames are redirected, and the caller falls back to `desktop_backdrop` (old behaviour).
3. Refresh policy (pure, `backdrop/surface_live.rs`):
   - recompose when the key `(rect, opacity, blur)` changes;
   - recompose on root child changes, only while the live path is active (F7). Each root event is
     coalesced into one recompose per shell tick. `LiveScene` skips the blur and upload when the
     sampled pixels are unchanged (`Unchanged`).
   - the region is the dock rect only. The compose reads frames under that rect, never the root.
4. The root watch is set once, after `LiveScene::new`, so the mask is not lost (F6).

Known gap (X11): content changes in a window beneath the dock (video, animation, repaint) with no
root event do not refresh the backdrop until the next root change or material change. The live
path's XDamage tracking lives inside `LiveScene` and is not routed to the shell. Follow-up: route
damage from the controller's pump to `live.classify`, or add a low-rate tick while the dock overlaps
a window.

### Wayland (implemented)

Code: `crates/flamewm-wm-wayland/src/render/dock_backdrop.rs`. The Dock is an `XWayland` strut surface
(`Unmanaged` entry with `strut`), so the compositor draws it with its own scene and reads its own
rendered scene, not the X root (F10, F11).

1. Material. `control/mirror.rs` `sync_chrome_settings` reads `dock_backdrop_percents` (the Dock role
   `(opacity, blur)`, the numbers the X11 shell reads through `taskbar_backdrop_percents`; that function
   now delegates to the same ui-core owner) and stores them with `set_dock_material`. Opaque material
   (opacity 100, blur 0), or flat `reduceTransparency`, calls `drop_docks()`: no render, no readback,
   no blur, and the cached textures are freed.
2. Sites. Each mapped `!below` strut surface is a site with its logical rect and published
   `_FLAMEWM_DOCK_CORNER_RADIUS`.
3. Scene stamp, every frame. `scene_beneath` walks `scene::front_to_back` after the Dock's index, so only
   what lies beneath it is considered, and builds those elements with `scene_elements(.., capture = true)`.
   Capture mode emits no backdrop elements, so a Dock never captures another Dock's backdrop. Elements
   overlapping the Dock's physical rect contribute `(physical rect, CommitCounter)` in front-to-back order.
4. Key and policy. `BackdropKey { rect, radius, scale_milli, material, scene }`.
   `needs_capture(wanted, cached)` is "not equal". A static scene matches exactly, so the frame costs the
   stamp comparison and nothing else. A move, resize, restack, commit or material/radius/scale change under
   or of the Dock is a different key.
5. Capture, only on a mismatch. The kept elements are relocated by `-origin` and rendered into a
   Dock-sized texture (`OutputDamageTracker`, full repaint, clear `CLEAR`). `read_reduced` box-averages it
   to 1/4 scale in strips (the same helper as the desktop capture), `blur_reduced` blurs it with
   `frosted_backdrop` at the Dock blur radius, and `crop_capture` samples the logical buffer. Its alpha is
   `rounded_coverage`, so the corners stay transparent, and it is premultiplied ARGB.
6. Draw. `build_elements` places the cached buffer right after the Dock surface, so it lies beneath the
   Dock and above the scene. The Dock's own translucent pixels blend over it.
7. Call sites. `refresh_dock_backdrops` runs once per frame per output in the winit `render_into` and in
   the DRM rebuild branch (DRM already rebuilds only when `scene_dirty`).

Deviations from the follow-up spec:
- The blur stays on the CPU (existing `frosted_backdrop` on the 1/4-scale capture). The Wayland
  compositor renders with `GlowRenderer`; the `flamewm-gpu-helper` child lives in `flamewm-render-x11`
  and is not used here. Moving the blur to GPU would be a separate change.
- The scene is walked each frame to compute the stamp, so a static scene pays element construction,
  not capture work. No render, readback or blur runs.

Known limits:
- A window beneath the Dock that commits every frame (video, animation) recaptures every frame. That is
  the damage gate working, at Dock size; it is bounded by the Dock rect, not the output.
- Menus beneath the Dock and a lower translucent Dock are drawn in the Dock backdrop without their own
  blur (capture mode emits no backdrops).
- Transparency is not disabled. Full-repaint policy is unchanged (partial repaint still waits on buffer age).

## Verification

- Unit tests: `cargo test -p flamewm-render-x11 --lib backdrop::surface_live` (policy).
- Gate: `FLAMEWM_TOOLCHAIN_BIN=/run/media/linsaftw/SDD/rust189/bin scripts/gate <crates>`.
- Runtime (X11): Xvfb with `flamewm-wm-x11`, a window beneath the dock, dock translucent. Compare
  screenshots with and without the change. Not done unless recorded in the report.
