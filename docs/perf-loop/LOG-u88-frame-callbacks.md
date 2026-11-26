# u88 frame callbacks / presentation feedback

Batch: frame callbacks and presentation feedback. Owned paths: `crates/flamewm-wm-wayland/**`.
No `cargo`, no `git` (shared checkout), so this slice is reasoned by call composition; the
orchestrator builds it.

## Composition reasoning

Per painted frame the compositor runs, in this order: build the element list from
`scene::front_to_back`, submit, then `presentation_scale::collect_feedback` and
`send_frame_callbacks`. Frame callbacks only reach visible surfaces already (both backends skip
hidden windows), and `send_frames_surface_tree` skips unmapped ones, so the
fan-out itself is one lock-plus-iteration per surface with no heap work. The two costs that were
still on every frame:

1. `collect_feedback` built `mapped_surfaces(...)`: a fresh `Vec<WlSurface>` with one Arc clone per
   toplevel, one per popup, one per layer surface, plus one `Vec` per popup tree. That is 2 + N
   heap allocations and 1 + N surface clones on every painted frame, for every session, including
   the large majority with no `wp_presentation` client at all.
2. The scene traversal behind both drawing and pointer picking (`visible_top_first_iter`) called
   `is_hidden(id)`, which looked the id back up in the same window vector: O(windows²) per painted
   frame and per pointer motion.

## Change

- `presentation_scale.rs`: `collect_feedback` walks owners directly (toplevels, their popups,
  layer surfaces of the output) and drops `mapped_surfaces`, so it allocates nothing per frame.
- `presentation_scale.rs`: feedback is now collected only for windows on screen. A hidden window
  (other workspace, minimised) was never presented on this output, so reporting it as presented
  was both a false report to the client and a reason for its pipeline to keep drawing off screen.
- `model.rs`: `visible_top_first_iter` filters the window it already holds instead of re-looking
  it up; the allocation-free traversal stays, minus the quadratic scan.

Expected composition shift: removes the presentation-feedback slice of per-frame allocation (2 + N
allocs, 1 + N surface clones per painted frame) and the quadratic scene scan from the paint and
pointer-motion paths. Frame-callback fan-out cost itself is unchanged and already visible-only.

Not done: full occlusion culling for behind-window clients. Doing it safely needs per-window
opacity (client buffers may be translucent and glass titlebars show what is behind), which the
model does not track; a wrong guess would freeze visibly animating content.

Verification: static only (no `cargo`/`git` in this unit). Orchestrator runs the check.
