# x01 damage tracking and frame pacing — design

Scope: `crates/flamewm-wm-wayland` render path (nested winit and tty/DRM).
Goal of the unit: repaint only damaged regions and issue zero frames when nothing changed.
Constraint: visuals stay pixel-identical; no new dependencies; `.vendor/**` is evidence only.

## 1. Current structure (file:line)

Nested winit path — `crates/flamewm-wm-wayland/src/render/mod.rs`:

- `:1` module owns frame composition, layer surfaces, titlebars, popups and frame callbacks.
- `:12` imports smithay's `OutputDamageTracker` as a real crate dependency (not `.vendor`).
- `:66-79` `SCENE_GENERATION` atomic plus `bump_scene_generation` / `scene_generation`; one bump per
  observed scene change, so cursor-only paints keep the previous generation.
- `:81-97` `SceneCache` and the thread-local `WINIT_SCENE` slot (generation, size, scale, pointer
  owner, cursor slice length, element list).
- `:99` `CLEAR` colour used by every draw path.
- `:353-369` `build_elements_into` appends the scene into one reusable `Vec<Elem>`.
- `:386-395` `covers`, `:403-405` `frame_reaches_output`, `:416-439` `MAX_OCCLUDERS = 64` and
  `collect_occluders`: opaque-region culling of lower windows.
- `:773-819` `render_frame`: stamps pacing, consumes `scene_dirty`, and applies the zero-frame gate
  at `:789-797` through `crate::state::nested_frame`.
- `:821-1046` `render_into`: refresh backdrops, lock screen, cached/full scene selection at
  `:873-893`, readbacks, damage swap and age at `:920-935`, composite pass at `:992-1006`, the
  window damage draw at `:1007-1024`, submit at `:1027`, callbacks at `:1031`, cache store at
  `:1033-1044`.
- `:1013-1022` the cursor-only recovery: when smithay reports no damage, repeat with age 0 so a
  damage swap can never present an undrawn back buffer.
- `:1116-1187` `send_frame_callbacks` uses the same visibility predicate as element building.

Damage-swap state — `src/render/gl_probe.rs`:

- `:4-9` `is_software_renderer`; `:11-25` four process statics (`BUFFER_AGE_REFUSED`,
  `FIRST_FRAME_SUBMITTED`, `WINIT_DAMAGE_SWAP`, `WINIT_PROBE_DONE`).
- `:30-40` `note_winit_buffer_age` records exactly one post-present answer.
- `:43-45` `buffer_age_probe_allowed`; `:51-61` `frame_age` (age 1 for cursor-only damage swaps);
  `:63-74` backend/renderer refusal latches; `:76-92` one-time GL renderer log.

Typed decision facade — `src/state/frame_pacing.rs`:

- `:3-249` tty decisions: `FrameDecision`, `plan_frame`, `SceneBuild`, `scene_build`, `arm_retry`,
  `retry_owed`, `SYNC_WAIT_BUDGET`, `SyncWait`, `sync_wait_decision`, `scan_failed`, `GpuProbe`,
  `pick_gpu`, `OutputFrames` with its typed transitions.
- `:251-282` `NestedFrame` and the pure `const fn nested_frame` (four bool inputs, `Paint`/`Idle`).
- `:284-308` three unit tests for that gate.
- Re-exported at `src/state/mod.rs:5-11`.

tty/DRM path — `src/drm/mod.rs`:

- `:14` states the rule lives in the typed facade so it is testable without a GPU.
- `:300-337` coalesced dirty flag, `plan_frame` gate and `scene_build` cache reuse.
- `:473-565` `draw_pass` maps smithay's `is_empty` result to `empty`, queue, defer or fail.

## 2. Counted problems (no fresh measurement was run)

These are static counts from the source above, not performance numbers; this unit is UNBUILT.

1. Before step 1 the nested zero-frame rule existed only as one inline condition in
   `render_frame` (`render/mod.rs:789-797`); the tty side already had 8 typed decision helpers in
   `frame_pacing.rs`, so the two backends did not share the same testable decision vocabulary.
2. The nested path carries 4 process-wide damage/age statics (`gl_probe.rs:11-28`) plus 2 scene
   caches (`render/mod.rs:71,96`) that are read by one call site each.
3. One frame can take 3 distinct age paths (`render/mod.rs:935`, `:1013-1021`, `:995-1005`); the
   choice is a tuple of booleans rather than a named decision.
4. Damage-rectangle shaping is entirely delegated to smithay; FlameWM owns no typed rectangle
   budget, so the number of submitted damage rects is unbounded by local policy
   (`render/mod.rs:1027` submits whatever the tracker returns).
5. Occlusion culling is bounded at `MAX_OCCLUDERS = 64` (`render/mod.rs:416`) but the damage list is
   not; a shared typed bound is absent.

## 3. Target design

One typed owner per decision, in `frame_pacing.rs`, following the rule already used for the tty
backend. Product code consumes a typed decision; UI never becomes damage authority.

- `NestedFrame` (done): whether a host opportunity spends a frame at all. Pure on four bools.
- `DamageAge` (planned): typed age selection replacing the boolean tuple of
  `render/mod.rs:920-935`; wraps the existing `frame_age` and `buffer_age_probe_allowed` predicates
  without changing them.
- `SubmitPlan` (planned): typed outcome of `render_output` — submit with damage, force full repaint,
  or skip — folding the cursor-only recovery at `render/mod.rs:1013-1022` into one decision.
- `DamageBudget` (planned): typed rect-shaping policy with a local cap, ported semantically from
  smithay's shaper (see §5) but using project types and no new dependency.
- Both backends call the same typed facade; each step only replaces expressions with typed calls,
  leaving element construction, clear colour, transforms and feedback order untouched so pixels
  remain identical.

## 4. Steps — each compiles alone

**Step 1 (implemented with tests, UNBUILT).** Add `NestedFrame` and `const fn nested_frame` in
`src/state/frame_pacing.rs:251-282`, re-export at `src/state/mod.rs:6`, and replace the inline gate
at `src/render/mod.rs:789-797` with one call. Three tests at `frame_pacing.rs:284-308`. Additive:
the function and its tests compile before the call-site swap, and the call site is a single
expression with identical operands. Rollback: revert those hunks and restore the inline condition.

**Step 2.** Move `frame_age` / `buffer_age_probe_allowed` behind the typed facade as `DamageAge`,
re-export, and switch `render/mod.rs:924-935` to one typed call. Pure rename of existing predicates,
no new state. Compiles alone. Rollback: revert the re-export and the one call site.

**Step 3.** Introduce `SubmitPlan` from the `Option<damage>` result plus the cursor-only flag and
replace `render/mod.rs:1012-1027`. Keep the forced age-0 redraw exactly as today. Compiles alone
because the enum is additive and the call site is one expression. Rollback: restore the inline
`if cursor_only && damage.is_none()` block.

**Step 4.** Add `DamageBudget` in a new render module with rectangle-merge tests using smithay's
`Rectangle` type already in scope; cap the submitted rect list at `render/mod.rs:1027`. Compiles
alone, no new deps, no visual change when the cap is not exceeded. Rollback: delete the module and
restore the direct `submit(damage.as_deref())`.

**Step 5.** Share `FramePlan` between `render/mod.rs` and `drm/mod.rs` by composing the step 1-3
decisions. Compiles alone once those types exist. Rollback: restore each backend's own gate.

No persistent state, schema or config changes in any step; rollback is a file-hunk revert.

## 5. Reference-first port record

Question: how do upstream compositors decide that an output owes no new frame, and how do they
shape damage, without changing what is on screen? Active owner: `frame_pacing.rs` (decisions) and
`render/mod.rs` (draw/submit).

- `.vendor/smithay-master/src/backend/renderer/damage/mod.rs:127-189, 321-371`. `OutputDamageTracker`
  keeps element commit counters and geometry, and `render_output` returns early with `damage: None`
  when `self.damage.is_empty()` (`:365-368`), i.e. zero draw work. `MAX_AGE = 4` at `:125`.
  **Choice:** do not transplant the tracker. The active source already depends on the `smithay`
  crate and uses `OutputDamageTracker` at `render/mod.rs:12, 1010-1011`; only the *decision*
  semantics are reimplemented as project types (step 1), keeping tracking with the crate.
- `.vendor/cosmic-epoch/cosmic-comp/src/backend/winit.rs:49-104`. Cosmic always submits, passing
  `damage.map(|x| x.as_slice())` (`:72-74`, `None` = full repaint) and only presents feedback when
  `damage.is_some()` (`:78-96`). **Choice:** not adopted. FlameWM's gate at `render/mod.rs:789-797`
  returns before bind/submit, which is the stronger zero-frame behaviour the unit asks for; cosmic's
  always-submit path would repaint a frame nothing changed. The cursor-only forced redraw at
  `render/mod.rs:1013-1022` covers the undrawn-back-buffer case cosmic avoids differently.
- `.vendor/smithay-master/src/backend/renderer/damage/shaper.rs:5-36, 46-89`. Tile shaping with
  `DEFAULT_MIN_TILE_SIDE = 16`, `MAX_DAMAGE_TO_DAMAGE_BBOX_RATIO = 0.9`,
  `TILE_DAMAGE_GAP_FRACTION = 4`. **Choice:** semantic input for step 4 only; no code, constant or
  dependency copied now, and no effect on current pixels.

All reference material stayed read-only evidence; no path was added to
`docs/ARCHITECTURE_EXCEPTIONS.toml`.

## 6. Verification status

No `cargo`, build, test or runtime command was run for this unit (worker rules forbid them and the
orchestrator runs them after the batch lands). Step 1's three tests are present but **UNBUILT** and
unrun. Every change recorded in `x01-damage-tracking.md` is marked UNBUILT.
