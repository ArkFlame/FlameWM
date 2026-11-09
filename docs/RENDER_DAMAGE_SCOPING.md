# Render damage scoping: where repaints exceed the change

Status: analysis only. Base: `origin/main` `485c55dd`. No product code changed.
Labels: **code** = read at cited line; **doc** = number from an existing repo doc (not re-run);
**analytic** = derived from code; **unmeasured** = not run in this pass.

## 0. Corrections to the brief

- `invalidate_all`, `full_repaint`, `Damage::Full`, `set_all`, `dirty.full` (as API) do not exist.
  Real names: `SurfaceDamage::Full` (`crates/flamewm-render-core/src/model/interaction.rs:23`),
  `mark_full`/`mark_dirty`/`touch*` (`model/document.rs`, `model/damage.rs`),
  `DirtyTracker.full` (`model/damage.rs:21`).
- task-manager, settings, shell, ui-core: zero damage sites. They reach damage through
  `RuntimeDocument` setters and `flamewm-ui-x11` `mark_full` (`surface_runtime.rs:390`).
- Node-scoped damage (`resolve_node_damage`, `take_incremental_damage`) has **no production
  caller**: `git grep take_incremental_damage` hits only `damage_tests.rs` and `layout_tests.rs`.

## 1. Repaint path

Setter → `RuntimeDocument::touch*` merges `SurfaceDamage::Full` into `pending_damage` and bumps
`revision` (`model/damage.rs:163`, `:227`; `model/document.rs:318`, `:341`). `pump.rs:87-93` drops
that doc-level Full. `redraw_if_dirty` (`app/render/frame.rs:33-58`) repaints on stale `revision`.
Bounding happens only in `plan_region_paint` (`app/damage_plan.rs:298-386`), which diffs old vs
new paint command lists (`diff_region`, `:327`). It returns `Region` only when enabled
(`:340` `region_disabled`). Enabled only for `FLAMEWM_RENDER_REGION=1` or the desktop role
(`damage_plan.rs:396-407`, `app/mod.rs:712-720`).
**Result: every non-desktop X11 window repaints fully on any document mutation** (analytic from `:340`).

## 2. Full-surface latches

| # | Site | Trigger | Node-scoped alt safe? (invariant) | Freq (est.) | Repaint vs necessary |
|---|---|---|---|---|---|
| F1 | `document.rs:318-319` `touch()` | untracked structural: `set_overflow` (`style/runtime.rs:139`), `set_border_radius` (`:97`), `set_font_size_px` (`:150`), `set_group_opacity` (`:202`), `set_ui_scale` (`:280`), `set_ui_font_defaults` (`:256`), `set_node_backdrop` (`:77`), image set/clear (`document.rs:494`, `:502`) | Partly: needs old+new box diff (`damage.rs:125-147` does this) | per setter call | window vs box+halo |
| F2 | `damage.rs:163` `touch_node` | `set_text_ref` (`document.rs:406`), `set_literal_color` (`style/runtime.rs:40`), `clear_color_field` (`:56`), `touch_geometry` (`geometry.rs:88`) | Yes in principle: `resolve_node_damage` damages old+new box+halo. Blocked by F9 | per write | window vs box |
| F3 | `damage.rs:227` `touch_isolated_box` (fn at `:217`) | text/visibility/font of isolated element | Yes, same resolver | per label write | window vs label |
| F4 | `document.rs:341-342` `touch_paint` | `set_color_variable` (`style/runtime.rs:10`), `set_foreground_color` (`:117`), `clear_foreground_color` (`:129`), `set_ui_text_shadow` (`:265`) | Yes: paint-only, box unchanged, halo from `paint_halo` | low (theme) | window vs box |
| F5 | `document.rs:349` `touch_light` + `interaction.rs:121`, `:134` `mark_dirty(Full)` | `apply_scroll_delta` (`interaction.rs:103-122`), `set_scroll_offset` (`:124-135`) | Yes: scroll viewport rect; outside-clip pixels unchanged | 10-60/s while scrolling (est.) | window vs viewport |
| F6 | `document.rs:424-427` `set_visible` | isolated visibility sets `dirty.full = true` | Yes, box+halo | popups (low-med) | window vs box |
| F7 | `style/runtime.rs:226-229` `set_font_weight` | weight change | Yes (isolated) | rare | window vs label |
| F8a | `app/events/event_loop.rs:143` `mark_full` | resize path | No: viewport change needs full | resize drag | necessary |
| F8b | `app/events/window.rs:43` | backdrop deadline expiry | Partly (backdrop rect) | per `LIVE_INTERVAL` | window vs backdrop rect |
| F8c | `app/window_backdrop.rs:132` | backdrop spec change | No: snapshot dropped | rare | necessary |
| F8d | `app/window_backdrop.rs:364` | refresh with no due time | Partly (backdrop rect) | per live event | window vs backdrop rect |
| F8e | `surface_controller/lifecycle.rs:153`, `:195` | hidden create; shape override set | Shape: old∪new shape bbox | per shape change | window vs shape bbox |
| F8f | `app/render/paint_full.rs:88` | not read | unknown | unknown | unknown |
| F9 | `damage.rs:236-251` `take_incremental_damage` | **no production caller** | the wiring gap; `take_damage` should use it | — | the fix seam for F2-F7 |
| F10 | `flamewm-assistant/src/osd_host.rs:214` → `ui-x11 surface_runtime.rs:390` | OSD `wants_timer()` per frame | Yes: OSD old+new rect | per frame while OSD visible | window vs OSD rect |
| F12 | `flamewm-wm-wayland/src/render/backdrop.rs:7`, `:206` | desktop marquee recapture, 100 ms | Partly: marquee ∪ blur radius | 10/s during marquee (doc) | unmeasured |

`live_backdrop.rs:387` keys recapture on a hash of all sampled pixels (42e986c2): an unchanged
backdrop skips reblur, but the surface still repaints.

## 3. Propagation model and containment

- **Layout rect**: `old_box.rect` and `new_box.rect` (`damage.rs:136`).
- **Paint halo**: `paint_halo` (`damage.rs:35-46`) covers `border_width` and `text_shadow` only.
  Missing: rounded-corner AA, outline/focus ring, text overhang beyond glyph box.
- **Backdrop**: any `node_backdrops` forces Full (`damage.rs:120`). Surface backdrops force
  `backdrops=true` (`render.rs:262`, `damage_plan.rs:312`).
- **Translucency**: `region_translucent` (`damage_plan.rs:413-420`) admits translucent fills only
  if the window backdrop is unchanged and no surface backdrop exists.
- **Clip**: `PushClip`/`PopClip` accepted without bounds (`damage_plan.rs:240-244`).

Containment that exists: absolute non-root element with explicit geometry
(`geometry_is_isolated`, `damage.rs:170`); its text-only children (`isolated_box_owner`,
`damage.rs:192`); desktop role region path (`mod.rs:712-720`).
Missing: an `island` concept at compiler/layout level; a node-level clip bound for damage
(scroll is Full, F5); a per-node opacity bound (F1).

### 3.1 Isolated paint island (proposed invariants)

A node is `island` only if all hold:

1. `position: absolute`, explicit left/top/width/height (no content-sized dimension).
2. Every child is absolute or text-only (no in-flow child, no sibling layout dependency).
3. `overflow` is `visible`, or the clip rect is the island box.
4. No `node_backdrop`; background opaque, or backdrop stable over box + blur radius.
5. Halo bounded and stored as `halo_px` = `paint_halo` + AA (≤1 px) + focus/outline width.
6. Text measured against its own box (no shrink-to-fit).

Detection: a pass in `flamewm-render-compiler` after style resolution sets `island` and
`halo_px` per node. Layout uses the flag to gate isolated patching. Analogous to CSS
`contain: layout paint size`, but checkable statically because the document is generated.

## 4. Transparency and blur (proposed, unverified)

Current: hash-skip recapture keys on sampled-pixel hash (`live_backdrop.rs:125`, `:142-144`,
`:387`). Proposed rules, each to be pixel-identical to full repaint:

- R1. Backdrop-dependent node damage = `box ⊕ blur_radius ⊕ halo`, clipped to viewport.
- R2. Backdrop samples come from the stable base (wallpaper + window backdrop snapshot), never
  the backbuffer.
- R3. Blur tiles overlap by ≥ kernel radius; a tile reaching outside the viewport falls back to
  Full (`backdrop_or_clip`).
- R4. Hash key per tile: `(hash(tile), size, rect, blur_percent)`, extending `live_backdrop.rs:387`.
- R5. Translucent node over an unchanged backdrop is region-safe; over a changed one it is Full.

Identity argument: the tile output depends only on pixels inside `box ⊕ radius`. Analytic only.

## 5. Measurement status

**Unmeasured in this pass.** Xvfb `:97` (pid 40026) belongs to another owner and was not touched.
No release build, no `scripts/perf/` harness run, no damage-rect instrumentation added.

Existing repo numbers (doc, not re-run):

| Scenario | Value | Source |
|---|---|---|
| Desktop recapture | 9.36 ms median, ~27 ms per run | `docs/PERF_AUDIT.md` "Marquee desktop backdrop" |
| Recapture cadence | 100 ms | `wm-wayland/src/render/backdrop.rs:7` |
| Marquee CPU | 89% to 142% of a core | same |
| Settings glass drag | ~34 fps, render-bound | `PERF_AUDIT` "WM wake attribution" |
| Drag cause | `BUFFER_AGE` refused, full software repaint each frame | same |
| Explorer scroll decodes | 268 → 86-120 | `PERF_AUDIT` "steady-view regression" |

Analytic ratio (non-desktop roles): repainted px ≥ window px / changed-object px, because
`damage_plan.rs:340` is unconditional. Concrete ratios need object sizes not measured here.

## 6. Paranoid damage mode (proposed safety net)

- `FLAMEWM_PARANOID_DAMAGE=1`, default off, read once via `OnceLock` (pattern of
  `plan_debug_enabled`, `damage_plan.rs:389`).
- After each `Region` repaint, render the full scene into a scratch buffer, compare against the
  presented backbuffer, log mismatching 16×16 tiles as `FLAMEWM_PLAN paranoid mismatch rect=...`.
- Present the full-render result in this mode, so a mismatch cannot reach the screen.
- Cost: off = one cached bool branch; on = one extra full paint and compare per region frame.
- Gate every §7 scoping change with a paranoid run of dock, task manager, settings, terminal.

## 7. Ranked work items (top 8)

1. **Paranoid mode.** Files: `crates/flamewm-render-x11/src/app/damage_plan.rs`,
   `app/render.rs`. Invariant: off = unchanged output. Test: forced-mismatch unit test.
   Acceptance: injected mismatch reported; off path unchanged. Risk: low. Deps: none.
2. **Wire node damage into production.** Replace doc-level `Full` merge in `touch_node`
   (`damage.rs:163`), `touch_isolated_box` (`:227`), `touch_paint` (`document.rs:341`) with the
   node rect from `take_incremental_damage` (`damage.rs:236-251`). Invariant: damage ⊇ old∪new
   box+halo (`resolve_node_damage`, `damage.rs:102-148`). Test: `damage_tests.rs` via
   `take_damage`. Acceptance: suite passes; 0 paranoid mismatches. Risk: medium. Deps: 1.
   Overlap: **render-core `model/` (document.rs, damage.rs); other worker owns style\***. Blocked.
3. **Scroll damage = viewport rect.** Replace `mark_dirty(Full)` at `interaction.rs:121`, `:134`
   with the scroll clip rect. Invariant: pixels outside clip unchanged. Test: `model/tests.rs`
   `:83-130`, plus paranoid on explorer scroll. Acceptance: damage = viewport area, 0 mismatches.
   Risk: scrollbar track must be inside rect. Deps: 2. Overlap: render-core `model/interaction.rs`.
4. **Region paint per role.** `region_paint_enabled` (`damage_plan.rs:403-410`) takes a role
   allowlist, starting with task manager and dock (**verify** dock role). Keep terminal and
   text-heavy windows off: `damage_plan.rs:5-7` records a terminal regression from Xft lacking
   clip. Test: `damage_plan.rs` tests (`:540-740`) plus paranoid run. Acceptance: task manager
   1 Hz region ≤ 50% viewport, 0 mismatches. Deps: 1. Overlap: render-x11 `damage_plan.rs`,
   `app/mod.rs:712-720`.
5. **OSD rect instead of mark_full.** `osd_host.rs:214` → OSD box damage. Invariant: OSD is an
   island (§3.1). Acceptance: damage = OSD box. Risk: low. Deps: none. Overlap: `flamewm-assistant`.
6. **Hover/active paint scoping.** Paint-only Paint node change (`style/runtime.rs:40`, `:56`)
   damages old∪new box+halo. Invariant: hover keeps layout (`geometry_is_isolated` already
   requires absolute hover position). Test: `model/tests.rs:45-79`. Acceptance: hover damage =
   node box. Risk: text-shadow halo. Deps: 2. Overlap: **render-core `style*` (other worker)**. Blocked.
7. **Island flag in render-compiler.** New pass in `flamewm-render-compiler` (not `utilities.rs`,
   owned elsewhere) computing `island`/`halo_px` per §3.1. Test: positive and negative fixture per
   invariant. Acceptance: set on dock/task-manager labels, unset on overflow or backdrop nodes.
   Risk: false positives, so keep conservative. Deps: none to detect; 2 to consume.
8. **Per-tile live-backdrop hash (R4).** `crates/flamewm-render-x11/src/app/live_backdrop.rs`
   `:125`, `:142-144`, `:387`. Test: `live_backdrop.rs:874-903` extended to tiles, plus paranoid
   on window drag over backdrop. Acceptance: more skipped reblurs on static backdrop; 0 mismatches.
   Risk: highest (blur identity). Deps: 1. Overlap: render-x11 `live_backdrop.rs` only.

Not ranked: F12 desktop recapture. Prior measurement caps it near 2% of total CPU (`PERF_AUDIT`).

## 8. Dependency order and parallel assignment

```
1 paranoid ──┬─> 2 wire node damage ──┬─> 3 scroll viewport
             │                        ├─> 6 hover paint (blocked: style*)
             │                        └─> 7 island consume
             ├─> 4 region per role
             └─> 8 per-tile backdrop hash
5 OSD rect, 7 island detect: independent
```

Parallel-safe now: **1** (render-x11 `damage_plan.rs`, `render.rs`), **5** (`flamewm-assistant`),
**7** (render-compiler, not `utilities.rs`). Serialise **2, 3, 6** (render-core `model/`).
**4** after 1 (same crate). **8** after 1 (`live_backdrop.rs` only).

## 9. Checks and gaps

- Ran: `git fetch origin`; worktree `/tmp/claude-1000/render-damage-scoping` at `origin/main`;
  static reads and `git grep` only.
- Not run: any `cargo` build or test, any Xvfb or perf harness, paranoid mode (not built).
- Not read: `app/render/paint_full.rs:88` trigger; Wayland render damage model; dock role variant;
  shell clock/weather text path (no `set_text` hit in `flamewm-shell`).
- Items 2-4 change render-core and region-paint behaviour. Each needs paranoid runs before merge.
