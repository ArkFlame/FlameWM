# u12-ui-document changes

- ui-document/template: `UiDocumentView::layout_box` now shares the retained-layout memo with
  `UiDocument::layout_box_by_id` (one `cached_layout` helper keyed by layout revision, geometry
  revision and viewport). Events that query several node boxes in a row lay the document out once
  instead of once per query, and paint-only edits keep hitting the same memo. Composition shift: the
  full `LayoutEngine::compute` pass leaves every repeated geometry read in an event handler.
- ui-document/focus: `next_focusable_where` walks the wraparound ring with a plain loop instead of
  building a `Box<dyn Iterator>` per call. Composition shift: Tab/Shift+Tab dispatch stops heap
  allocating and virtual-dispatching once per key press.
- ui-core/glass: the WCAG ink solve (`glass_ink`: luminance `powf` plus two scrim binary searches)
  behind `ink_for` is cached by `Appearance` in a bounded `Mutex<HashMap>` (cap 16, same policy as
  the regular-glass cache). Composition shift: every frame that republishes `--text`, `--muted` or a
  surface fill reuses one solve per distinct appearance instead of re-running it per projection.
- ui-core/glass: `overlays_from_settings` reads the light flag straight from the `appearance` key
  (`appearance_is_light`, also used by `appearance_from_settings`) instead of building a full
  `Appearance`. Composition shift: overlay token projection skips tint parsing, blur mapping and
  backdrop lookup on every frame; only the two keys it needs are touched.
- ui-layout-core: `solve_linear` reuses the already accumulated `used` extent as `content_extent`
  when no flex distribution ran. Composition shift: the common fixed/content stack layout drops one
  O(items) pass per solve, including the zip/filter/fold of every hidden item.
