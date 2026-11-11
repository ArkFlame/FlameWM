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
