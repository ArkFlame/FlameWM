# u23-ui-core-text changes

- skin/palette: `ink_palette_on(surface)` and `on_accent(accent)` each ran two-to-four WCAG
  `powf` luminance solves per call, and every titlebar and accent-fill label asks them per frame
  (`ChromeTokens::ink`, wm-x11 chrome, Wayland render, `appearance_tokens`). Both are now memoised in
  bounded `Mutex<HashMap<Rgb, _>>` caches (cap 16, reset-on-full like the glass caches), keyed by the
  exact RGB they read. Composition shift: per-window titlebar ink and accent label ink stop
  recomputing contrast in the paint loop; a desktop with a fixed palette hits one entry.
- ui-core/glass: the ink solve cache (`ink_for`) was keyed by the whole `Appearance`, so
  `resolve` and `palette_from_settings` rebuilt the same text ink under different keys whenever
  they differed in taskbar mode, taskbar tint/opacity or blur — fields the solve never reads. A new
  private `InkKey` (light, tint, glass opacity, backdrop) keys the cache. Composition shift: the
  text-ink palette is solved once per distinct backdrop/tint instead of once per caller shape.
- ui-core/glass: `palette_from_settings` (the `--text`/`--muted` projection) no longer builds a full
  `Appearance` through `appearance_from_settings`. It reads only the ink inputs via two extracted
  helpers (`tint_rgb_from_settings`, `glass_opacity_from_settings`, also reused by
  `appearance_from_settings`), skipping taskbar mode, taskbar tint/opacity and blur-key parsing on
  every text-token publication. Composition shift: every frame that republishes text tokens drops
  roughly six settings lookups, one hex parse and one `is_group_custom` scan.
