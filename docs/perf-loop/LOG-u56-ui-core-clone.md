# u56-ui-core-clone changes

Batch seven, area `ui-core` / `ui-document` / `ui`. Reviewed the earlier perf work in these crates
by reading `LOG-u23-ui-core-text.md`, `LOG-u39-ui-doc2.md` and the current source. The
clone/alloc-density figures in the unit brief mostly pointed at `#[cfg(test)]` code
(`settings_values` clones, `template.rs` fixtures); production clone sites were only
`TextInput::event` (inherent: `UiEvent` owns a `String`) and `Button::activate`/tree clones of the
id types. No correctness regression from the earlier ink-cache / appearance-projection work was
found: `InkKey`, `REGULAR_CACHE`, `appearance_is_light`, `palette_from_settings` and the retained
layout memo all still use the same inputs their consumers read, so this slice records
"no regression found" rather than a fix.

## Ranked hotspots and changes

### 1. `surface_blur_percent` built a whole `Appearance` to read one blur value

Every desktop widget render calls `surface_blur_percent` (`crates/flamewm-desktop/src/widgets/render.rs`)
and the shell settings projection calls it per republish. It used to run
`appearance_from_settings` (~twelve hash-map lookups, a `surfaceColor` hex parse, taskbar
mode/tint/opacity parsing, the backdrop lookup) and then `resolve(GlassRegular, ...)`, whose only
needed output is `glassBlurPx` scaled by 100. Added `glass::regular_blur_percent_from_settings`,
which reads the flat flag and `glassBlurPx`/legacy `surfaceBlurPercent` only. The scrim can raise
alpha but never blur, so the result is identical to the old resolve path.

Composition shift: per-widget backdrop blur reads drop from a full appearance build to two setting
lookups on the default translucent desktop.

### 2. `dock_backdrop_percents` resolved the full Dock material for two integers

`dock_backdrop_percents` (shell/Wayland dock) built `Appearance` and called `resolve(Dock, ...)`.
For `BlurOnly`/`CustomTint` the resolution reads only `taskbarOpacityPercent` and
`taskbarBlurPx`; for a flat material it is `(100, 0)` regardless of tint. Added
`glass::non_inherit_dock_material_from_settings` (one owner, mirrors the Dock branch of `resolve`)
and an early flat return, so the common non-inherit and reduced/high-contrast cases never build an
`Appearance` or enter the regular-glass ink solve. `Inherit` still uses the full solve because the
ink scrim can raise its alpha.

Composition shift: dock backdrop projection skips the appearance build and, for non-inherit modes,
the ink solve entirely.

### 3. `dock_fill` built an `Appearance` it discarded, and resolved non-inherit docks the slow way

`dock_fill` called `appearance_from_settings` before its `highContrast` check, so a high-contrast
dock paid for an appearance it immediately replaced with the opaque palette surface. Moved that
check ahead of the build, then routed `BlurOnly`/`CustomTint` through the new non-inherit material
helper (same RGB/alpha mapping as before). `Inherit` keeps the shared `surface_fill_with` path and
the light-taskbar alpha floor.

Composition shift: high-contrast docks and non-inherit docks stop building an unused appearance.

### 4. `WidgetId`/`ActionId` clone allocated a new `String` per dispatch

`stable_id!` stored `String`, so cloning an id into a `UiEvent`, a cloned `UiNode` tree, or
`Button::activate`'s `ActionId` copied the string (and could not share it). The backing is now
`Arc<str>`: `new`, `From<&str>`, `From<String>`, `as_str`, `Display`, `Eq/Ord/Hash` are unchanged,
but clones are a refcount bump. Construction from an owned `String` still allocates once into the
`Arc`; from `&str` it allocates once instead of the previous `String` allocation, so no call site
grows.

Composition shift: event/tree dispatch no longer allocates for each id clone.

## Shared-reader refactor

`appearance_from_settings` now uses the extracted `taskbar_mode_from_settings`,
`taskbar_opacity_from_settings`, `taskbar_custom_tint_from_settings`,
`glass_blur_px_from_settings` and `reduce_transparency_from_settings`. The fast paths and the full
builder therefore read one definition of each key rule instead of two.

## Regression checks added

`glass::fast_path_tests` asserts the blur fast path equals `resolve(GlassRegular, ...)` and the
non-inherit dock material equals `resolve(Dock, ...)` across default, legacy blur,
reduce-transparency and high-contrast snapshots, plus that `Inherit` still reports `None` (needs
the full solve).

## Dead code

None found in the owned paths: every private helper has a caller (`rg` over `crates/**`), and the
remaining single-file public items (e.g. `inner_contour_coverage`, `ScrollStore`, the `flamewm-ui`
model components) are exported renderer-neutral contracts, not dead internals. Deleted none.

## Files touched

- `crates/flamewm-ui-core/src/glass.rs`
- `crates/flamewm-ui-core/src/surface.rs`
- `crates/flamewm-ui/src/id.rs`

## UNVERIFIED

No cargo/rustc/git in this shared checkout, so none of the above was compiled or tested here. The
orchestrator must build, run `glass`/`surface` unit tests and eyeball dock blur/opacity/visuals.
