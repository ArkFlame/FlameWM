# u119-window-model changes

Area: Wayland window model/state, owned path `crates/flamewm-wm-wayland/src/state`. Target: stop
paying per-event scans and allocations on focus, output homing and work-area queries, keeping every
ordering and visual rule identical. Code reading only; no build or benchmark was run here (the
batch build gate owns that).

## Changes

1. **Focus no longer copies the visible stack twice** (`state/focus.rs`).
   `set_focus` built `visible_top_first()` before and after `raise` — two `Vec<WindowId>` of every
   visible window, a `Vec` allocation plus two full walks and a list compare per focus event — only
   to answer "did the stack change?". `raise` moves exactly one window to the end of the stacking
   vector, so the visible sequence changes **iff** that window was visible and not already the
   topmost visible one. That is precisely "the first id of `visible_top_first_iter()` changed", so
   the two lists are replaced by two `next()` calls on the borrowing iterator. Equivalent, no
   allocation, and it stops at the first visible window instead of walking all of them.
   `refocus` uses the same iterator instead of materialising the list for `.first()`.

2. **Primary output name cached** (`state/mod.rs`, `state/outputs.rs`).
   `smithay`'s `Output::name()` locks the output and clones its `String` on **every** call. The
   primary name is compared on:
   - every client commit: `output_handle_of` (handlers `commit` → line 162) used to allocate one
     `String` per driven output just to `find` the surface's home, then `output_handle` did the
     same again for `enter`/`leave`;
   - every work-area query, including the one on each pointer motion of a move/resize grab
     (`pointer_work_area` → `work_area_on` → `work_area` → `struts_on`);
   - `reflow_if_work_area_changed`, `rect_on_output`, `work_area_of`, `output_name_under_pointer`.

   `State` now holds `primary_name: String`, captured once in `State::new` from the output it owns
   (an `Output`'s name is fixed at construction; `self.output` is never replaced — verified by
   searching for assignments to `state.output`/`self.output`). Every one of those call sites now
   compares against the cached name; `output_handle`/`output_handle_of` gained a primary fast path
   before the handle scan, so the common single-output commit resolves with a hash lookup and a
   borrowed `&str` compare and zero allocations. Secondary outputs keep the scan, unchanged.

3. **`pointer_work_area` is allocation-free** (`state/mod.rs`).
   It ran `output_name_under_pointer()` (a `String` clone) and then `work_area_on(&name)`, i.e. one
   allocation per pointer motion while dragging or resizing. It now borrows the layout name with
   `output_at` + `layout_outputs.get(index)` and falls back to `work_area()` exactly where the old
   `None`/primary path did. `work_area_of` likewise borrows the primary name instead of cloning it.

## Composition shift

Ownership of the compositor's per-event allocator traffic: focus events drop two `Vec<WindowId>`
allocations and four O(windows) walks to constant work; each client commit drops one `String`
allocation per driven output (more on multi-output DRM), and each grab pointer motion drops one
`String` in the work-area path. In a one-second sample of a repainting client plus a window drag,
the affected share is the `alloc`/`free` slice of the input and commit handlers; layout, raster and
blit shares are untouched because no visual or ordering rule changed.

## Out of scope (owned path)

Hit-testing and stacking order live in `crates/flamewm-wm-wayland/src/model.rs` (`hit`,
`visible_top_first_iter`, `switcher_order`), which this unit does not own; `visible_top_first_iter`
already avoids the earlier per-window `is_hidden` re-lookup. The `toplevels`/`x11` id scans in
`state/mod.rs` (`toplevel`, `window_for_surface`) would need a hash index maintained where those
vectors are pushed and retained (`handlers.rs`, `xwayland.rs`), also outside this unit's paths.
