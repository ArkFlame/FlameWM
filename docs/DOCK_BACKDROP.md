# Dock backdrop: translucent surface over windows

Status: X11 live path implemented (this change). Wayland: documented gap, follow-up spec below.

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

### Wayland (gap, not implemented)

The dock is an Xwayland client, and the compositor draws it with its own scene. Windows beneath it
are native Wayland surfaces, which the X live path cannot read (F11). A correct fix samples the
compositor's rendered scene under the dock rect. That is an architectural change in
`flamewm-wm-wayland/src/render` (scene readback into a small target, blur, damage-limited refresh)
and is not done here. Current Wayland behaviour is unchanged: translucent dock over the desktop
layer, no window content (F10, F12).

Follow-up spec: "Wayland surface backdrop from scene": render the scene elements below a strut
surface's rect into a 1/4-scale target on damage, blur it, and feed it as that surface's backdrop
element. Refresh only on damage intersecting the rect.

## Verification

- Unit tests: `cargo test -p flamewm-render-x11 --lib backdrop::surface_live` (policy).
- Gate: `FLAMEWM_TOOLCHAIN_BIN=/run/media/linsaftw/SDD/rust189/bin scripts/gate <crates>`.
- Runtime (X11): Xvfb with `flamewm-wm-x11`, a window beneath the dock, dock translucent. Compare
  screenshots with and without the change. Not done unless recorded in the report.
