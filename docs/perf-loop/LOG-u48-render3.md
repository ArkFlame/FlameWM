# u48-render3 changes

Sixth pass over render-core / render-x11 / ui-x11. The earlier render units removed the
first-order waste (retained software and GPU frame scratch, borrowed shadow/mask keys,
allocation-free measure loop, whole-frame GPU batching, opaque/table fill paths). This pass
targets the work that still sits on every relayout, every text-bearing frame and every
cross-process visual lookup. `git log` is forbidden in the shared checkout, so the review read the
current sources and the earlier render logs (u08, u29, u38) instead of the commit diff.

- render-x11/xft + xft/backend_measure: text advances are cached. `measure_text` issued one
  `XftTextExtentsUtf8` server round trip per call, and the layout engine calls it for every word
  and every growing wrap candidate (`text_width` in `text_layout.rs`), on every relayout and on
  every paint that rebuilds commands. The measured width is now cached per (family, text, size,
  weight) in a bounded map keyed through the existing borrowed `TextMaskRef` view, so a hit
  allocates nothing and skips the round trip. Entries stop at 4096, text keys stop at 256 bytes,
  and the map is cleared when the preferred family changes (advances depend on the face).
  Composition shift: wrapping a paragraph drops from O(prefixes) server calls per layout to one
  per distinct word, and relayout of unchanged labels (hover, scroll, patch) measures no text at
  all. The old comment claiming "nothing is cached across frames" is corrected.
- render-x11/app/surface_backdrop + app/images (regression): the retained `argb_visual` field is
  documented as "avoid XMatchVisualInfo for each image upload/text shadow", and `render.rs`,
  `render/text_shadow.rs` and `gpu.rs` already use it, but the image and backdrop upload paths
  still called `XMatchVisualInfo(display, screen, 32, …)` on every upload. Both now take the
  visual resolved once at startup; `match_argb32_visual` is the same match, so this is exactly
  equivalent. Composition shift: every surface backdrop upload and every image cache miss drops
  one server round trip. This also removes the last per-upload visual query from the steady path.
- render-core/pixels: clipped rounded fills no longer force the whole-frame scratch copy.
  `paints_in_place` now admits `FillRoundedRectClipped` after proving the clip lies inside the
  frame (or is empty), and `apply_ops_in_place` runs it on the frame through a reusable
  thread-local region buffer with the same 16 MiB retention cap as the other scratches.
  `fill_clipped_cpu` only reads and writes the clip rows, so in-place output is identical; the
  whole-frame copy is only needed for blur and backdrop ops and those still take the scratch path.
  Composition shift: a batch that mixes clipped fills with fills/text/image compositing (stroked
  window borders under a clip stack, scrolled content) loses two full-frame copies per painted
  frame on the software/Wayland CPU path.
- render-x11/app/render/shape: `base_shape_spans` returns `Cow` instead of owning. An installed
  alpha silhouette (`shape_override`) can carry one span per row and was cloned on every changed
  frame; it is now borrowed, and only computed coverage/dock masks still allocate.
  Composition shift: shape-backed popups/overlays stop copying their whole span vector each
  changed frame; the XShape apply/hash behaviour is unchanged.

## Review notes

- `diff_region` bails to `Full` on the first changed command without bounds and only compares
  commands whose old/new forms differ; the skip path still requires `interaction_only` and an
  identical list. No regression found there.
- `insert_cached_image` recomputes the byte sum on insert and `evict_node_images` allocates a
  small stale-key vector, but both are bounded by 64 entries; left alone rather than churn the
  cache contract in a pass that cannot build.
- Dead-code/unused-warning cleanup in `flamewm-desktop`, `flamewm-explorer` and
  `flamewm-settings` is not in this unit's owned paths; it belongs to u41-desktop-dead,
  u42-explorer-warn and u43-settings-lazy. Nothing in render-core/render-x11/ui-x11 was deleted
  speculatively because no compiler output is available in the shared checkout.

## Verification

Static reading plus `rustfmt --edition 2024 --check` on every edited file (parses, format-clean).
No cargo, no git in the shared checkout: compile and tests are the orchestrator's step.

## Skipped

- Backdrop blur in place: `gaussian_blur_rgba_with` can return `Err` mid-op, and the buffered
  path is what preserves the backend's "untouched on error" contract; kept on the scratch path.
- Coverage-span memoization across frames: needs a content key for the command list and a
  `Cell`-based cache on `&self`; left for a unit that can benchmark it.
