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

## Evidence

- Runtime evidence: none. Not run in this pass.
- Static evidence: the file references above, read on `origin/main`.
