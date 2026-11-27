# u17 — Wallpaper retained raster cut, narrow retry

**Scope:** read-only search plus at most two safe pixel-identical cuts in the wallpaper
decode/cache path. No `git`, no `cargo`, no builds, tests, Python or sleep loops; shared
checkout, orchestrator commits. Visuals are unchanged. Marked UNBUILT.

## Step 1 — wallpaper references (`rg -il wallpaper crates --glob '*.rs' | head -20`)

1. `crates/flamewm-ui-document/src/template.rs`
2. `crates/flamewm-settings-core/src/tests.rs`
3. `crates/flamewm-settings-core/src/lib.rs`
4. `crates/flamewm-skin/src/palette.rs`
5. `crates/flamewm-ui-core/src/surface.rs`
6. `crates/flamewm-ui-core/src/glass.rs`
7. `crates/flamewm-wm-wayland/src/control/ports.rs`
8. `crates/flamewm-skin/src/recipes/decoration.rs`
9. `crates/flamewm-widgets-core/src/widgets.rs`
10. `crates/flamewm-x11-desktop/src/ports.rs`
11. `crates/flamewm-wm-wayland/src/render/titlebar.rs`
12. `crates/flamewm-image-core/src/wallpaper.rs`
13. `crates/flamewm-image-core/src/scale.rs`
14. `crates/flamewm-image-core/src/webp.rs`
15. `crates/flamewm-image-core/tests/cpu_raster_snapshot.rs`
16. `crates/flamewm-wm-wayland/src/render/decoration_tests.rs`
17. `crates/flamewm-wm-wayland/src/render/backdrop.rs`
18. `crates/flamewm-image-core/src/lib.rs`
19. `crates/flamewm-image-core/src/decode_cache.rs`
20. `crates/flamewm-imageviewer/src/lib.rs`

## Step 2 — decode/cache cuts

Pending.
