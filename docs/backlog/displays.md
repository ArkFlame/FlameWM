# Displays: per-output layout and scale

Status: **Partial** (README roadmap row, `Displays | Per-output layout and scale`).
Basis: source reading of `origin/main` at the time of this note. No runtime
evidence was captured, so nothing here is Shipped.

## What exists (source, origin/main)

- Persisted config: `crates/flamewm-platform/src/settings/product.rs:255`
  (`per_output_scale: BTreeMap<String, u16>`), validated in
  `settings/product_validate.rs:206` (output id non-empty, 100..=200 in 5% steps).
  Read and write paths: `settings/snapshot_parse.rs`, `settings/snapshot_serialize.rs`,
  `services/settings.rs`.
- Platform apply and persist: `crates/flamewm-platform/src/host/displays.rs`
  (`persist_output_scale`, with rollback to prior value on error) and
  `host.rs:160` (applies saved scale to the display snapshot at start).
- Tests: `settings/tests.rs` (round-trip, invalid rejected),
  `host/tests.rs:218` (start applies persisted scale),
  `services/display/tests_five.rs:127` (scales survive remove/re-add).
- Layout model: `crates/flamewm-settings/src/display_layout.rs` (per-output
  position, primary flag, shell scale, overlap-free contiguous row, apply verification).
- Wayland helpers: `crates/flamewm-wm-wayland/src/output_layout.rs`
  (`scale_percent_for`, `logical_size`, `physical_size`, `arrange_row`,
  `output_at`, `clamp_point_to_layout`), with unit tests in the same file.
- Wayland scale on the output that the compositor drives:
  `main.rs:863` (`presentation_scale::output_scale`) and `state.rs:631`
  (logical size from scale); DRM mode choice uses `select_mode` (`drm/mod.rs:150`).

## What is missing (blocks Shipped)

1. Multi-output arrangement is not applied. `arrange_row`, `output_at` and
   `clamp_point_to_layout` have no non-test callers in `flamewm-wm-wayland`, so
   the compositor does not place outputs at the persisted origin. Scale is applied
   only to the single output the compositor drives.
2. Settings Displays UI: a per-output scale and position row with accessible
   names was not confirmed in the Settings pages. Treat as unverified.
3. X11: RandR code exists in `crates/flamewm-wm-x11/src/wm/outputs.rs`, but
   per-output scale is not hooked there. X11 is out of scope for this row.
4. Runtime proof: none captured. The nested winit backend exposes one output,
   so multi-output placement cannot be proven there; it needs a headless or
   virtual multi-output path, which was not identified.

## Shipped minimum (agreed definition)

1. Persisted per-output config: output name -> origin, scale percent. Validated, typed error path.
2. Wayland compositor applies per-output scale and origin for every output.
3. Settings Displays row to view and change per-output scale and position, with accessible names.
4. Unit tests for config validation, persistence and layout math (non-overlapping outputs, scale clamp).
5. Fresh runtime evidence: nested or virtual multi-output run with compositor logs
   showing applied scale and origin per output, plus screenshots, cited here.

Rotation, refresh rate, drag-and-drop and a revert timer are out of scope.

## Slice A landed (unit-level wiring only)

- `output_layout::origin_after` (`crates/flamewm-wm-wayland/src/output_layout.rs`)
  returns the origin of an output added after the placed ones, as `arrange_row`
  would place it. `drm/outputs.rs::build_output` uses it for every DRM output after
  the first, so a second connector no longer lands at (0,0) over the first.
  The first output keeps (0,0): single-output behavior is unchanged.
- Test: `single_output_stays_at_origin_and_later_outputs_follow_the_row`
  (output_layout.rs). This proves the math only, not smithay or DRM behavior.
- Origins are not persisted. They follow the row order of connector discovery.

## Still open (decisions needed)

1. Per-output scale key. The persisted key is fixed to `wayland-0`
   (`crates/flamewm-wm-wayland/src/control/mod.rs:78`, `control/settings.rs:107`),
   and all outputs use that one scale. Decision needed: key by connector name
   (changes single-output lookup for existing configs) or keep one shared scale.
2. Pointer clamping. `output_at` and `clamp_point_to_layout` are still not called
   from the input path. The input path has not been traced.
3. Settings scale slider. `DisplayScaleSlider` (`crates/flamewm-settings/src/actions.rs`)
   exists. Its accessible names are not verified.
4. Runtime. No multi-output runtime path has been run. The DRM path cannot run in the
   development environment, and no private nested multi-output session was found.

## Slice B (pointer clamp, settings label, decisions, runtime status)

1. Pointer clamping: wired. `State::pointer_layout` (`crates/flamewm-wm-wayland/src/state.rs`)
   is rebuilt from the live DRM outputs in `drm/outputs.rs::sync_pointer_layout` after
   connector add/remove and on drop. `input/pointer.rs::on_motion` clamps through
   `output_layout::clamp_point_to_layout`, using `output_layout::pointer_layout` (row
   when stored, else the primary `output_rect`). Commit `16212dd3`. Test
   `pointer_layout_prefers_the_row_and_falls_back_to_the_primary_output` proves selection
   and clamp math only, not runtime wiring. Behavior note: a motion past the single output
   edge now lands on its last pixel; in-bounds motion is unchanged.
   Still open: hit lookups (`pointer_target`) still use per-window geometry, not the row.
2. Settings scale slider: accessible name present. `ui/settings.html:58`
   (`role="slider" aria-label="Display scale" aria-valuemin="100" aria-valuemax="200"`).
   Test `view.rs` `scale_is_a_single_draggable_control_not_five_buckets` now asserts the
   label. Commit `8eb2c3b9`.
3. Per-output scale key: decision needed, nothing changed. `wayland-0` stays the persisted key.
   - Option A, per-connector keys (`DP-1`, `eDP-1`): `scale_percent_for` and
     `control/settings.rs:107` read the connector name; `control/mod.rs:78`
     `nested_output_scale_percent` picks the key per output; `drm/outputs.rs:190` and
     `build_output` read per output; `host/displays.rs::persist_output_scale` writes per output.
     Migration risk: existing `scale.wayland-0` values stop applying unless migrated to the
     primary connector name. Connector names can change across hardware or port moves, so a
     stale key silently falls back to 100%.
   - Option B, single shared key: no code change beyond the docs. Every output gets one
     scale; mixed-DPI multi-output setups remain impossible.
   - Needed decision: A or B, and for A whether `wayland-0` migrates to the primary connector
     or stays as a fallback key.
4. Runtime: no private nested multi-output path exists. The nested backend
   (`FLAMEWM_WAYLAND_BACKEND=winit`, `main.rs:5`) creates one `Output` (`state.rs` `on_resized`).
   The DRM path needs a real tty and connectors; it was not probed (host buses off limits).
   The settings acceptance SS08 multi-output case is `ENVIRONMENT_BLOCKED` in `docs/TERRA_RESULTS.md:371`.
   No new runtime evidence was captured in this slice. README stays `Partial`.

## Evidence

- Runtime evidence: none.
- Static evidence: the file references above, read on `origin/main`.
- Gate (pre-rebase, base 6c1319d2): `scripts/gate flamewm-wm-wayland` run 2 was
  `EXIT_GUARD=0 EXIT_FMT=0 EXIT_CLIPPY=0 EXIT_TEST=0 GATE_DONE all=0`
  (`/tmp/opencode/displays2-A2-gate.log`). Run 1 had `EXIT_FMT=1` (one wrapped line),
  fixed with rustfmt.
- `cargo check -p flamewm-wm-wayland --features drm`: exit 0.
- `python3 -I tools/architecture_guard.py`: exit 0.
- README stays `Partial`.
