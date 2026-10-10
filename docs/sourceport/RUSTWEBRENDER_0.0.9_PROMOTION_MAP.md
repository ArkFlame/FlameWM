# RustWebRender 0.0.9 Promotion Map

J08 source-port boundary for the supplied RustWebRender 0.0.9 corpus.
Promoted owners are compared after exact per-owner mechanical translations
below. Rust formatting is ignored; every other mechanic difference is drift.

- Reference: `.vendor/RustWebRender-0.0.9/`
- Active namespace: `crates/flamewm-render-*`
- Verifier: `tools/verify_rwr_promotion.py`

## Exact Promotion

| Vendor owner | Active owner | Allowed difference |
|---|---|---|
| `crates/rustwebrender-core/src/{codec,layout,lib,paint}.rs` | `crates/flamewm-render-core/src/{codec,layout,lib,paint}.rs` | namespace; codec diagnostics `RustWebRender` -> `FlameWM Render` |
| `crates/rustwebrender-compiler/src/{compile,css,html,lib}.rs` | `crates/flamewm-render-compiler/src/{compile,css,html,lib}.rs` | `rustwebrender_core` -> `flamewm_render_core` |
| `crates/rustwebrender-compiler/src/bin/rwr-inspect.rs` | `crates/flamewm-render-compiler/src/bin/flamewm-render-inspect.rs` | namespace and product target rename |
| `crates/rustwebrender-x11/src/{lib,xft,xlib,xrender}.rs` | `crates/flamewm-render-x11/src/{lib,xft,xlib,xrender}.rs` | namespace; declared Flame diagnostics/env names; `extern "C"` -> `unsafe extern "C"`; X11 role/property seam |
| `crates/rustwebrender-x11/src/bin/rwr-x11.rs` | `crates/flamewm-render-x11/src/bin/flamewm-render-x11.rs` | namespace and product target rename |

The verifier recognizes exactly these namespace substitutions, plus the
per-owner translations listed in the table:

```text
rustwebrender_core     -> flamewm_render_core
rustwebrender_compiler -> flamewm_render_compiler
rustwebrender_x11      -> flamewm_render_x11
```

## Declared Flame Extensions

Only these source-port extensions are allowed. Verifier removes their exact
extension bodies, compares remaining Rust tokens against vendor mechanics, then
checks extension anchors:

- `crates/flamewm-render-core/src/lib.rs`: public `ActionPhase`, `ActionEvent`,
  and `ControllerEvent` event types.
- `crates/flamewm-render-x11/src/lib.rs`: public controller-event callback seam,
  `X11WindowRole`, its default configuration field, and `_NET_WM_WINDOW_TYPE`
  property assignment.
- `crates/flamewm-render-x11/src/xlib.rs`: X11 property constants and
  `XChangeProperty` declaration required by role assignment.
- `crates/flamewm-render-x11/src/config.rs`: Flame-only `X11WindowIdentity`
  struct (ICCCM WM_CLASS instance/class pair) carried by the optional
  `X11Config.identity` field, which defaults to `None`. The 0.0.9 reference has
  no such type or field. The verifier removes the `pub identity` field and the
  `identity: None` default entry through `CONFIG_X11_FIELDS` and
  `CONFIG_X11_DEFAULT`. `X11WindowIdentity` and the `From<SurfaceConfig>` body
  are not compared items.

CSS and HTML are product-owned presentation inputs in this promotion; they are
not silently treated as exact vendor copies.

## Gate

Run `tools/verify_rwr_promotion.py` against a tree containing the 0.0.9 vendor
manifest. A pass proves source identity and declared anchor closure only.
Build, runtime, and visual parity remain separate gates.
