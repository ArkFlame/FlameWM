# Overnight optimization plan (2026-10-24)

Constraint: no cargo/rustc/build/test is allowed. Every change is verified by reading only, so each
change must be small enough to prove by inspection, and each is listed here for a later
`cargo check` + test pass.

## Done (uncompiled)
| Area | File | Change |
|---|---|---|
| clip dedupe | render-x11 app/render.rs, mod.rs | `gc_clip` cache skips repeated XSetClip*; reset at every other GC clip mutator |
| clip pairs | render-x11 app/scroll.rs | drop PushClip/PopClip pairs whose run paints nothing |
| backdrop | render-x11 app/window_backdrop.rs, overlay.rs | single `to_vec`; single-pass min/max |
| layout | render-core layout.rs, paint.rs | merged duplicate measure loops; skip scroll-chrome lookup |
| scrollback | terminal screen.rs | `compact_history` trims trailing blanks (keeps 1 cell) |
| rows | terminal ui.rs | `fit` returns `Cow`, no per-row double clone |
| pointer | terminal app/pointer.rs | write selection only if changed |
| snap preview | wm-x11 snap_preview.rs | ring-mask skip on pure move; no hide() before re-show |
| selection | explorer main.rs, desktop rubber.rs | skip redundant rubber work; lazy style |
| slots/drag | desktop slots.rs, drag.rs | borrow instead of clone; shared helper |
| thumbs | explorer thumbs.rs | O(1) LRU lookup |
| sort | explorer view.rs | use cached `lower_name` |
| icon layout | desktop main.rs, slots.rs | `ProjectedCellsCache` keyed on inputs |
| region paint | render-x11 surface_controller.rs `pump_events` | drained document `Full` damage maps to `None` so document edits stop forcing `region_force_full`; needs Full-vs-Region pixel equality proof (revert: restore `app.mark_dirty(pending)`) |
| region test | render-x11 app/damage_plan.rs | `moved_translucent_rect_over_unchanged_backdrop_stays_region` (unrun) |

## Round 2-4 (uncompiled): simplification + test-module moves
- ~550 lines of dedupe/dead-code removal across terminal, shell, desktop, ui-x11, lock, render-core, render-x11, wm-x11.
- `#[cfg(test)]` bodies moved to sibling files (shell app/runtime, desktop, wm-x11 mod, terminal screen, render-x11 overlay/events, explorer main/lib/thumbs); `include_str!` paths shifted `../`.
- Python source-contract tests repointed: explorer move test, pass54, pass62, pass26. Still failing and NOT caused by the moves (also fail at HEAD): explorer Cargo `rustix` marker, desktop pass18 Delete key, flicker pass16 overlay mask, pass26 text-shadow marker was fixed by reverting the paint.rs `text_command` helper.
- `tools/architecture_guard.py` run: 8 errors, none in files whose content this phase changed except the unproven desktop `facade-source-loc>5000`.

## Planned large changes (ordered by value / risk)
1. **Terminal pointer "changed" flag** (terminal main.rs:145-153, app/pointer.rs). `pointer()` returns
   bool; `project()` runs only when true. Rule: any path that is not provably a no-op returns true.
   Callers: grep `pointer(`; update tests. Risk: a missed true = stale frame. Low if default is true.
2. **PaintCommand text as `Arc<str>`** (render-core paint.rs + every consumer in render-x11, shell,
   terminal tests). Order: (a) grep all `PaintCommand::Text`/`TextShadow` constructors and matchers,
   (b) change the field type, (c) fix each site with `.into()`/`&*`. Risk: high without a compiler;
   do it only once `cargo check` is allowed. Saves a String clone per text command per frame.
3. **Per-frame layout skip** for text edits that cannot change the box. Needs runtime proof; defer.
4. **Snap-preview redraw skip** on pure move. Needs runtime proof of repaint-after-move; defer.

## Verification queue (needs the build ban lifted)
`cargo check` on: render-core, render-x11, terminal, wm-x11, explorer, desktop, shell, ui-x11.
Tests: `cargo test -p flamewm-render-core -p flamewm-render-x11 -p flamewm-terminal -p flamewm-wm-x11
-p flamewm-explorer -p flamewm-desktop`, then the Xvfb region-equivalence tests and the T07 canary.

## Vendor-derived ideas (read-only audit, no code edit)
- `.vendor/` holds icewm-master, flamewm-c-reference, terminator-master, creamui-main, cosmic-epoch. No picom, openbox, i3, awesome, xfwm, skia or webrender trees are present, so those techniques could not be checked here.
- Motion coalescing during drag is already implemented: `crates/flamewm-wm-x11/src/event_pump.rs` collapses contiguous same-event/child `MotionNotify` to the latest, and `crates/flamewm-wm-x11/src/wm/mod.rs:1086-1101` drains the pump before and after the reactor hook once per turn. No change needed.
- Redundant preview updates are already skipped: `crates/flamewm-wm-x11/src/snap_preview.rs:99-127` returns early when candidate and geometry are unchanged (`is_current`), and a visible surface is moved or resized in place instead of hidden and re-mapped.
- Unverified candidate: the icewm drag/outline path (`.vendor/icewm-master`) may show a cheaper outline-only redraw. Not read yet; needs a runtime measurement before any change.
