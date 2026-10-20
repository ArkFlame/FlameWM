# FlameWM Architecture

## 1. Product boundary

FlameWM is now an independent Rust X11 desktop/window manager.

Historical IceWM-derived code is **reference material only**. Production behavior may be learned from it, but production ownership lives in FlameWM crates.

```text
.vendor/**                    exact historical/reference corpora
       |
       | engineering evidence only
       v
crates/flamewm-wm             X11 WM authority
crates/flamewm-platform       product services/policy
crates/flamewm-*-core         pure product models
crates/flamewm-shell          V8 shell controller
crates/flamewm-render-*       compiled HTML/CSS renderer
```

No active crate may include/link/load `.vendor/**`.

## 2. Window-manager authority

`flamewm-wm` owns:

- SubstructureRedirect/root WM ownership;
- client discovery and reparenting;
- SaveSet/frame lifetime;
- ICCCM close/state handling;
- EWMH root/client state;
- focus/activation;
- minimize/maximize/fullscreen;
- interactive move/resize;
- workspace assignment/count/current workspace;
- work-area/snap geometry.

It is implemented against `x11rb`; no IceWM executable or compatibility bridge runs underneath it.

Workspace topology has one owner: `flamewm-api::workspace` defines the product two-row layout
(`WORKSPACE_ROWS = 2`, `workspace_columns(count) = ceil(count / 2)`, row-major cells, no
wrap-around) and every layer consumes it instead of repeating the arithmetic —
`flamewm-platform::workspace` facades it, `flamewm-wm-x11` resolves arrow shortcuts with
`grid_neighbor` and publishes the same grid through `_NET_DESKTOP_GEOMETRY`/`_NET_DESKTOP_VIEWPORT`,
and `flamewm-shell-core::workspaces` owns only the pager presentation (cell ids, metrics, labels)
derived from it. The earlier single-row rule that made up/down no-ops is superseded: left/right stay
inside the current row, up/down switch rows.

## 3. WebRender authority

WebRender owns only FlameWM-rendered presentation:

- compiled tree/style data;
- layout;
- hit testing;
- paint commands;
- typed runtime mutations;
- X11/Xft painting.

WebRender never becomes the authoritative model for managed X11 clients/workspaces.

## 4. FlameWM Render 0.0.6 source promotion

0.0.6 is used as an identity-preserving renderer tranche, not merely as a design description. The active renderer source was refreshed from its exact core/compiler/X11 owners, with only mechanical crate renaming and FlameWM-specific integration retained.

Important retained 0.0.6 contracts:

```text
MotionNotify -> collapse only contiguous same-window motions
             -> never search/reorder beyond ButtonRelease/other event
             -> mutate from newest currently-arrived coordinate

image copy -> cached server pixmap + optional 1-bit transparency mask
           -> GraphicsExpose disabled

interactive launcher -> release binary
```

## 5. V8 presentation

`ui/index.html` and `ui/flamewm.css` are derived from the 0.0.6 V8 showcase that visually converged against the supplied prototype. The source asset names/geometry remain stable on purpose.

Production compilation happens in `flamewm-shell/build.rs`; no HTML/CSS parser is retained at runtime.

## 6. Unsafe boundary

The product workspace forbids unsafe Rust where workspace lints are inherited. The Xlib/Xft renderer crate is the explicit FFI boundary and does not inherit that lint. `flamewm-wm` uses safe `x11rb` protocol APIs.

## 7. Integration direction

Near-term:

```text
flamewm-wm process            authoritative real X11 windows
flamewm-shell process         desktop/panel/popovers/settings presentation
```

Long-term convergence keeps one state authority and minimizes process duplication. The UI controller consumes typed state/events and issues commands; it never constructs a second hidden WM.
