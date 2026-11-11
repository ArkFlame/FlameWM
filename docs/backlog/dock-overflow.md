# Dock overflow at narrow widths

> Backlog row: `cmp-dock-overflow` (`docs/BACKLOG.md`).

Status: OPEN. The taskbar has no recorded narrow-width runtime check with both
the system tray and clock present. No placement/overflow policy has been
decided; this note specifies evidence to collect, not a UI design or behavior.

## Bounded runtime check

Run the current `flamewm-shell` artifact under a private Xvfb display and a
private D-Bus session. Keep the tray and clock enabled and visible, and use a
fixed, recorded set of taskbar entries. At each width, capture a screenshot
and record the taskbar's measured bounds plus the tray and clock bounds. Verify
that both remain visible, lie within the taskbar, and do not overlap other
taskbar content. Include one comfortably wide control and progressively
narrow widths down to the smallest supported test width; stop after the first
failure and record its width. Preserve exact launch/settings commands,
artifact identity, display size, geometry measurements, screenshot paths, and
cleanup result under `.agents/runtime/<job>/`.

The check is evidence gathering only. If content cannot fit, record the
observed failure and ask for a placement decision before implementing hiding,
wrapping, scrolling, or another policy. No XTest input is needed unless
reproducing the failure requires interaction.

## Current evidence

- Source/backlog inspection: `crates/flamewm-shell/src/app/dock.rs` has no
  overflow implementation (backlog records zero `overflow` hits).
- Runtime geometry and screenshot: NOT RUN in this documentation-only slice.
- Placement decision: unresolved; implementation intentionally deferred.

## Source findings (layout owner, origin/main 0193147d)

- Owner: `ui/shell/panel.html` (`#task-slots` has 8 fixed `task-slot-N` buttons,
  `#tray-group`, `#clock-capsule`) with CSS in `ui/shell/flamewm.css`
  (`.task-slot` 42px). Rust projection: `crates/flamewm-shell/src/projection/panel.rs`
  (`project_panel`, `project_task_slots_extent`, `task_slots_rendered`), mirrored in
  `async_projection/panel.rs` and `async_projection/task_slots.rs`.
- Task count is capped at `TASK_SLOT_COUNT = 8` only, not by width. Nothing measures
  the bar width against task, tray and clock widths. `crates/flamewm-shell/src/app/dock.rs`
  is the app dock surface, not the taskbar, so the backlog's original pointer there was
  off-target.
- Flex inference (static, unverified at runtime): `#task-slots` has an explicit width,
  its slots cannot shrink below their 42px min-content, so excess width spills past the
  bar edge rather than clipping. Tray and clock can be pushed out of view.
- Slice 1 (landed): windows past `TASK_SLOT_COUNT` show as a `#task-overflow` `+N` cell
  (role=status, text is the accessible name) and the cell adds to `#task-slots` width;
  no clipping is silent. Precedent: KDE Plasma icon-tasks and COSMIC applet overflow
  (`.vendor/cosmic-epoch/cosmic-applets`) both collapse excess items into an overflow
  control; a popup is deferred because no popover route exists for it here. Width-aware
  cap and runtime check remain open.
- Earlier reasoning: a non-overlapping cap needs the real `#tray-group`
  width. That width depends on media, audio, network, battery, weather and SNI item
  visibility, which is split across `projection/panel.rs`, `tray_host.rs`,
  `taskbar/status/*` and the weather projection. Reserving the tray's maximum width
  would cut slots at normal widths. The indicator/menu UX is also an open decision.
- Next slice (needs a placement decision first): a single shared tray-width helper used
  by all three projection paths, then `max_task_slots` in `taskbar_scale.rs` gating
  `index < rendered` for visibility. Runtime geometry check per
  `Bounded runtime check` above is still NOT RUN.
