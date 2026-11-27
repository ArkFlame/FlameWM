# U24 — Start menu and popup open cost audit

Status: **UNBUILT**. No `git`, `cargo`, Python, build, test, or runtime command was run for this
unit; all conclusions are source-level. The orchestrator must build and test the batch.

## Scope and evidence

- Owner paths: `crates/flamewm-shell/src/projection/start.rs`,
  `crates/flamewm-shell/src/async_projection/start_view.rs`, this file.
- Open/scroll path: `app/start_view.rs::project_start` → `async_projection::project_start_unified`
  → `projection::start_program_rows_and_results` → `start_visible_results` →
  `StartModel::results_view`/`query_view`. Wheel and arrow keys re-enter this path on every step
  (`app/start_view.rs::scroll_start_list`, `start_scroll_rows`); each keystroke re-enters with a
  new query.
- The recent Session-row edits were read first and kept verbatim: `ProgramRow::Session`
  (start.rs:129), `is_selectable` treating session rows as selectable (start.rs:140-142), the
  three pinned rows at the bottom of All Programs (start.rs:221-223), the session-label branch
  (start_view.rs:133-146), and the `"session"` test marker (start.rs:501).
- Route skills loaded: `flamewm-source-first`, `flamewm-parallel-execution`, `flamewm-rust-systems`,
  `flamewm-performance-proof`. This is an allocation-shape audit, not a measured performance
  claim; no latency or CPU improvement is asserted.

## Findings

| file:line | cost | change | risk |
| --- | --- | --- | --- |
| start.rs:191-226 `all_programs_rows` | One `BTreeMap` node plus one heap `Vec` per letter bucket, rebuilt on every scroll/open; the three appended Session rows forced one more `Vec` growth. | Replaced the bucket map with one `Vec<(char, usize)>` sorted stably by bucket key; reserved the exact header count and `SESSION_ROW_COUNT = 3` up front. Stable sort preserves both key order (`char` `Ord` is the same order `BTreeMap` uses) and insertion order inside a bucket. Session-row code is unchanged. | Low. No behavior branch changed. If a future Session row is added without updating `SESSION_ROW_COUNT`, capacity is only a hint, so output stays correct. |
| start.rs:249-263 `start_program_rows_and_results` | Built a full `Vec<&DesktopApplication>` of all apps, then discarded it on the All Programs empty-query path, once per scroll/open. | Return an empty results vector early on that path. `project_start_with_results` reads results only in search mode (`searching == false` here), and `count` is used only in that same branch, so the projected document is unchanged. | Low, with one boundary: this `pub(crate)` helper now returns empty results for All Programs callers that might read them in future. Its only current caller is `project_start_unified`; revisit if a second caller appears. |
| start.rs:64-78 `project_start_with_results` | `category_bucket(*value)` recomputed 9 × N times per non-search projection (N applications), even though it depends only on the static rail table. | Precompute the nine rail buckets into a stack array once, then compare each application's bucket. Same nine comparisons per application, same visibility result. | Low. `StartBucket` is `Copy + Eq`; no allocation or ownership change. |
| start_view.rs:106-111 slot id buffers | Three `String::new()` buffers grow through repeated `write!` reallocations on the first slot of every open/scroll. | `String::with_capacity` sized for `"start-app-slot-"` + two digits, plus each suffix. Buffers are still cleared and reused across all slots; text is byte-identical. | Low. Capacity is a hint only; longer slot numbers still format correctly. |
| start.rs:229-243 `start_program_rows` / `start_visible_results` | Two `Vec` allocations per non-All-Programs slot-count lookup (`query_view` refs, then rows); called multiple times per scroll from `app/start_view.rs`. | Not changed. Removing the intermediate vector would require duplicating `StartModel::filter_view`/`admits` filtering outside the core owner, which risks divergence. | Deferred: owner boundary, not a safe local cut. |
| start.rs:12-33 / core `start_menu.rs:310-324` `query_view` | Full re-filter of the app list per keystroke: lowercased needle plus one `Vec` of matches. | Not changed. Caching needs the generation/category/query key and invalidation owned by `StartModel` (another crate); a module-level cache here would add shared mutable state and stale-generation risk. | Deferred: requires a typed cache in the model owner. |
| start_view.rs:133-146 session label; start.rs:148-153 `program_bucket` | Label match runs at most three times per projection and the header letter uses a 4-byte stack buffer; application names are borrowed, not cloned. | Not changed: no allocation to remove, and hoisting the match would add state without reducing work. | None. |
| start.rs:269-282 `start_selectable_window` | Rebuilds the whole row list to read a window of eight booleans on each keyboard step. | Not changed: eliminating it needs a row-count/window API on the model owner. | Deferred: owner boundary. |

## Verification withheld

Per the handoff, no build, test, lint, or runtime command was run. Type inference for
`[None; PRESENTATION_CATEGORIES.len()]`, `usize::from(bool)`, and `slice::windows` relies on the
workspace toolchain Rust 1.89 / edition 2024. Visual equivalence of the projected document and the
sort-order equivalence with the former `BTreeMap` are reasoned from source, not executed.
