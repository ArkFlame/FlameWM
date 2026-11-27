# Unit p47-render-damage-only
STATUS: PARTIAL

## Result
STARTED

Crates flamewm-wm-wayland and flamewm-render-x11: find full-surface repaints where only a small region changed (clock tick, hover); restrict to damage rects, pixels identical. Report with file:line, max 3 edits.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-render-core/src/model/damage.rs:102-147 | `resolve_node_damage` already turns a tracked text/clock edit into `SurfaceDamage::Region` (old+new boxes, paint halo, 50% area cap); a node backdrop or untracked move correctly yields Full. | none - already correct | none |
| crates/flamewm-render-x11/src/app/render/frame.rs:74-97 | `document.take_damage()` region is merged into `pending_damage` at :79 and then unconditionally cleared at :97; the paint planner never receives the damage rect. | none - wiring it would switch ordinary windows to region paint, which mod.rs:252-256 records as having dropped terminal updates, clipped windows and stale selection rectangles | edit not applied (UNBUILT n/a) |
| crates/flamewm-render-x11/src/app/damage_plan.rs:62-79, 338-341, 403-408 | The damage-rect planner `plan_redraw` is `#[cfg(test)]` only; production `plan_region_paint` is opt-in (`FLAMEWM_RENDER_REGION`) and on by default only for Desktop. Document-driven clock ticks on other windows therefore take Full. | none - enabling it would loosen the opt-in correctness gate, forbidden by the brief | edit not applied (UNBUILT n/a) |
| crates/flamewm-render-x11/src/app/damage_plan.rs:327-337, 424-457 | Hover with equal command lists already plans `Skip`; hover that adds/removes a command makes `diff_region` return None, so Full is the safe fallback. | none - teaching `diff_region` to handle length changes loosens the equality gate | edit not applied (UNBUILT n/a) |
| crates/flamewm-wm-wayland/src/render/mod.rs:916-931, 1003-1023 (with render/gl_probe.rs:68-74) | Scene frames use the buffer-age fallback; nested winit EGL refuses BUFFER_AGE, so age 0 forces full-surface damage for clock ticks. Cursor-only motion already gets age-1 damage rects after swap-damage is proven. | none - an unproven age > 0 can present an undrawn back buffer | edit not applied (UNBUILT n/a) |

Edits applied: 0 of 3 allowed. No edit could make a full-surface repaint damage-only without loosening an existing correctness gate (region-paint opt-in, command-list equality, or buffer-age proof); the brief forbids that, and pixel identity cannot be proven with builds/runtime checks also forbidden here.

PARTIAL - investigation complete for both crates; all full-surface repaints found are deliberate, gated fallbacks rather than missed damage rects, so no safe production edit was made and no change was built or runtime-verified.
