# u10-deps-ram

- ui-core/glass: `regular` memoises the resolved GlassRegular material in a bounded
  `Mutex<HashMap<Appearance, (tint, alpha, blur)>>` (cap 16, clear on overflow). The WCAG legibility
  solve it wraps (luminance `powf`, two scrim binary searches) ran again on every `surface_fill` /
  `dock_fill` / theme projection, which paint paths call per frame. Composition shift: repeated
  per-frame glass resolution leaves the UI thread; settings edits only add a cache entry because the
  key is the pure `Appearance` value.
- ui-core/surface: `surface_fill` split into `surface_fill_with` so `dock_fill` resolves
  `appearance_from_settings` once and reuses it for the fill and the Dock material. That function used
  to parse the same snapshot up to three times per taskbar paint (inherit mode) and twice otherwise.
  Composition shift: 1-2 fewer settings parses per dock projection per frame.
- ui-core/surface: `outline_fill` takes static key triples (`SURFACE_OUTLINE_KEYS`,
  `WINDOW_OUTLINE_KEYS`) instead of building `format!("{prefix}Color")`-style names. The three heap
  strings per outline (surface rim, window rim, inherited rim) are gone from every repaint.
- ui-core: deleted the dead `icons` module (297 lines). It duplicated the canonical
  `flamewm-integrations-linux` resolver and `flamewm-skin` icon tokens, allocated a `String` per
  `resolve`, and had no consumer in the workspace. Composition shift: less text resident and less
  compile work; the icon path keeps one owner.
- Owned `Cargo.toml` files needed no trimming: `flamewm-ui` depends only on `flamewm-ui-core`, and
  `flamewm-ui-core`'s three path deps (`flamewm-api`, `flamewm-skin`, `flamewm-ui-layout-core`) are
  all used; `flamewm-skin` has no dependencies.
