# FlameWM 0.1.2 — Pass 8: shared glass contour, Explorer edge, scrolling

Date: 2026-10-27. Baseline: Performance Pass 7 archive. Source-only implementation, not runtime-validated.

## Goals and protected behavior

1. Eliminate a fixed near-black X11 frame edge that can appear as a right-side black seam beside Explorer; use a configurable glass contour instead.
2. Reuse **one** Surfaces visual contract (stroke RGB and opacity), default `#FFFFFF` at `25%`, with 1px *inside* client surfaces for shell, Explorer, Terminal, Settings, start, calendar/weather/audio/Wi-Fi popups and OSD. Reuse the same contract for native frame/titlebar paint; the X11 window-border pixel is opaque and receives a flattened equivalent.
3. Keep all existing surface fill/blur, client sizing, taskbar placement, focus/geometry and interactions unchanged. Zero percent disables contour.
4. Reduce allocation during Explorer scroll animation without altering list rows, search positions, glide, visibility, row ordering, or hit testing.

## Source findings

- `crates/flamewm-wm-x11/src/wm/manage.rs` hardcoded frame border pixel `0x001b1e20`. The narrow outer frame strip plausibly explains a dark line adjacent to Explorer content; without a live capture this cause remains a hypothesis.
- `crates/flamewm-ui-core/src/context_menu.rs` already owns shared surface-colour resolution, making it the least-divergent place for the new independent `surface_stroke` helper. UI backends consume `UiColor` derived from this shared model; there is no duplicate per-popup RGB/alpha policy.
- `crates/flamewm-explorer/src/main.rs::project_rows` built a `Vec<&Entry>` on each projection, although its fixed `ROW_SLOTS` upper bound makes a stack array sufficient.
- The opaque X11 frame-border pixel cannot represent transparency. `frame_border_pixel` composites the RGB against the titlebar fill. The titlebar renderer also draws the actual alpha contour into its native pixel buffer, without a new X request.

## Implementation

- Product settings: `surfaceBorderColor`, `surfaceBorderOpacity` live in the stable settings map; defaults `#FFFFFF` / 25, validation, persistence and live snapshots included.
- Settings → Surfaces: six colour presets, custom picker, opacity stepper; live projection of root border.
- Native shell: shared `surface_stroke` projection for taskbar, Start, task/menu, launcher, switcher, media/OSD, and quick-control popup roots (including calendar, weather, Wi-Fi and audio).
- Explorer, Terminal and Settings: root CSS retains a non-zero 1px border; live `UiDocumentAccess::border` applies the shared setting. Explorer transient dialogs inherit the same contour.
- Native X11 frame: the formerly hardcoded dark frame border pixel follows `ChromeTokens.surface_border_*`; existing frames update on settings changes, and the titlebar buffer receives the one-pixel inner contour.
- Performance: replace Explorer `Vec<&Entry>` per projection with a bounded `[Option<&Entry>; ROW_SLOTS]` scratch array; unchanged visible-row selection and fallback behavior.
- Tests: Rust unit tests cover default/custom/disabled contour, per-shell-surface projection and frame-border flattening, plus Settings action mapping; the shared recording document now captures border writes.

## Expected savings (not benchmark data)

- Explorer animation: eliminates **one heap allocation and corresponding deallocation per row projection**. For a hypothetical 60 projections/s, at most ~60 allocation pairs/s while continuously scrolling; negligible memory savings at idle. New stack array is bounded by `ROW_SLOTS`.
- Contour composition is O(frame titlebar width + titlebar height) during existing titlebar repaints; it can slightly increase CPU when painting chrome. No new thread, polling loop or graphical surface is added. No reliable overall CPU/PSS estimate without native benchmarks.

## Verification

- Selected Python regression suite (4 files): `89 passed, 3 failed`. Those exact three source-contract failures also occur on the unchanged Pass 7 baseline (`PerOutputOpacity` expectations referencing old shell source). The errors are not introduced by this patch and tests were not modified to hide them.
- New Rust regression tests are **unexecuted** because neither `cargo` nor `rustc` is installed here.
- No compositor/X server or UI screenshots available; **pixel-perfect transparency, corner outlines, right-edge disappearance and X11 runtime behavior remain unverified**.
- Before merge, execute `cargo test --workspace` and `cargo build --release` on a Rust-capable Linux workstation, then visually check an Explorer window on both sides against the wallpaper, Terminal and Settings, titlebars, Start/submenus, each tray popup, taskbar normal/separate mode, maximized/fullscreen windows, and changes to colour, 0%, 25%, and 100% opacity. Verify multi-monitor scaling and retain screenshots. Sample idle PSS/CPU and Explorer scrolling before/after.

## Risks and scope

- X11 frame border is flattened to opaque, so it is an *approximation* of 25% alpha when the true underlying backdrop differs from the titlebar fill.
- The black line diagnosis is source-grounded but not screenshot-confirmed; if it persists, inspect client/frame extents and background exposures before changing X11 geometry.
- The native **Wayland compositor** still paints its server-side titlebars with independent Smithay solid-fill elements in `crates/flamewm-wm-wayland/src/render.rs`, without the X11 frame `ChromeTokens` path. Those native Wayland titlebars are **not** covered by this pass; do not claim a global Wayland-native border until the Wayland decoration layer and settings updates are wired to the shared colour/opacity owner and graphically verified. XWayland FlameWM apps already use the shared client-surface UI path.
- No shader rewrite, root event-loop modification, timeout changes, DPI model change or window positioning change.
