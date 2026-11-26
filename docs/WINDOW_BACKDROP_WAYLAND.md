# Window backdrop on Wayland: real scene transparency for FlameWM apps

Status: design (u148-u150). Goal: under the Wayland compositor, translucent FlameWM app windows
(explorer, terminal, task manager, settings, ...) show the live scene beneath them (other windows,
Wayland and X11, plus the desktop), not only the wallpaper.

## Why it shows only the desktop today

- FlameWM apps are X11 clients (render-x11, Xwayland under the Wayland compositor). They use
  pseudo-transparency: the app composes a static ARGB snapshot of what lies behind it, lays it
  into its own scene first, and blends its translucent fills over it
  (`render-x11/src/app/window_backdrop.rs`, `surface_backdrop.rs`).
- The X11 WM gives them a live source (`_FLAMEWM_REDIRECTED_FRAMES`, Composite pixmaps). Under
  Wayland that property is absent and Wayland windows are not X windows, so the live path returns
  `None` and the snapshot falls back to the desktop layer (`crop of the wallpaper canvas`).
- The compositor already owns the right data: it renders the whole scene. `render/dock_backdrop.rs`
  captures "the scene beneath a Dock", blurs it and draws it under the Dock, keyed by a per-element
  commit stamp so a static scene costs one comparison. App windows just do not use it.
- `render/mod.rs` also paints an opaque `client_backdrop` solid behind every decorated client,
  which would hide any transparency.

## Options

| Option | Verdict |
|---|---|
| A. Publish `_FLAMEWM_REDIRECTED_FRAMES` from the Wayland compositor | Rejected. Wayland windows have no X pixmaps; the app would read stale or empty frames. |
| B. App reads the compositor scene via a screencopy/dmabuf protocol | Rejected. Round trip, readback and a second blur per app, per frame; the compositor already has the pixels on the GPU. |
| C. Compositor draws the backdrop under translucent windows (generalise `dock_backdrop`) | Chosen. One capture and one blur in the process that owns the scene, cached by scene stamp, zero cost when the scene is static or the window is opaque. Matches KWin/Mutter blur-behind. |

## Contract (fixed; each unit implements its side)

1. Root flag: `_FLAMEWM_COMPOSITOR_BACKDROP`, `CARDINAL[1] = 1`, set on the Xwayland root by the
   compositor when its XWM starts (u149). Absent under the X11 WM, so X11 behaviour is unchanged.
2. Window material: `_FLAMEWM_WINDOW_MATERIAL`, `CARDINAL[2] = [opacity_percent, blur_percent]`,
   set by an app on its own window (u148). Absent or `[100, 0]` means opaque: no backdrop.
3. Model: `Managed::material: (u8, u8)` `(opacity, blur)`, default `(100, 0)`, refreshed from the
   property on map and on property change (u149). A change requests a redraw.
4. Compositor (u150): a managed window whose material needs a backdrop
   (`opacity < 100 || blur > 0`, and not flat/`reduceTransparency`) is a backdrop site. Its client
   rect captures the scene beneath it (same stamp/blur/cache as the Dock), the blurred buffer is
   drawn directly beneath the client's surface elements, and the opaque `client_backdrop` solid is
   skipped for it. Capture mode builds no backdrops, so windows never capture each other's glass.
5. App (u148): when `_FLAMEWM_COMPOSITOR_BACKDROP` is set on the root, the app installs NO snapshot
   and runs no live scene; it keeps its transparent clear so translucent fills reach the compositor
   with their alpha, and publishes the material property. Otherwise it behaves exactly as before.

## Verification

Nested run (`scripts/flamewm-nested-smoke` style, winit only): open a translucent app over a
window with distinct content; the content must be visible and blurred through the app body.
Gate: `scripts/perf-loop clippy`, crate tests, plus `scripts/perf-clients` idle ticks unchanged.
