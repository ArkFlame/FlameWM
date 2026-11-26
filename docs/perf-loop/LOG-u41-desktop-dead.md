# Log u41-desktop-dead

Area: `flamewm-desktop` widgets lifecycle/render, `hide_panels`, `visibility_effect`,
analog/ring pixel generation, and the desktop binary entry.

Reviewed the recent perf slices for this area (u22 desktop/wm, u41 predecessors) by reading
the current code. The visibility gate (`visibility_effect`/`set_obscured`), the obscured
deferral in `main.rs`, the coalesced drag path, the `SyncKey` fast path and the ring/face/graph
image caches all still match their logs; no behavioural regression was found. The one real
smell was a duplicate function left behind by the panel split (below).

## Optimizations

- `widgets/render.rs` (`sync_widgets`): the idle seconds clock was the hottest steady-state
  path. A tick where only the wall clock changed failed the whole `SyncKey` compare, so every
  second cloned the settings snapshot, the widget list, weather and media, then rewrote the
  geometry, surface, eight edge flags and every part-visibility of every card, even though only
  a clock could have changed. Added a second compare (`sync_key_inputs_match`) that ignores
  time: when only the clock differs, the sync redraws just the time-reading slots
  (Clock/Calendar/Custom), skips all card chrome, and updates `key.time` in place instead of
  rebuilding the key. Expected shift: a one-second tick with N widgets costs one clock slot
  instead of a full-document rewrite plus an owned snapshot of all desktop state.
- `widgets/render.rs` (`surface_key` + `SyncKey`): replaced the stored `SettingsSnapshot`
  (a `BTreeMap` of every setting) with the two values the widget layer actually reads — the
  inherited surface fill/blur and the resolved accent. Expected shift: no deep settings-map
  clone on every sample (4 s) or interaction sync; the next-turn compare becomes two small
  value compares. `widget_surface` still resolves from a snapshot for callers/tests.
- `widgets/render.rs` (`widget_placeholders_if_used`): custom-template placeholders are
  formatted (time, date, weekday, uptime, five owned strings) only when a Custom widget exists,
  instead of on every clock tick. Expected shift: no per-second placeholder formatting or
  strings for clock/gauge/media-only desktops.
- `widgets/render.rs` + `widgets/sync.rs`: `projection_accent` (which re-parses the accent
  setting) is resolved once per sync and threaded into `sync_analog`/`sync_gauge`, instead of
  once per card (and once more inside the gauge ring key). Expected shift: one accent parse
  per sync instead of up to two per widget slot.
- `widgets/pixels.rs` (`analog_face_pixels`): the face generator resolved each of the 60 tick
  endpoints and 3 hand endpoints with `sin/cos` inside the per-pixel loop — over five million
  trig pairs for a 300 px face, once per second for an analog seconds clock. Tick and hand
  endpoints are now computed once per face before the pixel loop; the per-pixel work is the
  same capsule coverage math, so the image is pixel-identical. Expected shift: the analog
  face rebuild loses its dominant transcendental cost.

## Dead code

- `widgets/render.rs`: deleted the duplicate `hide_panels`. The panel split left it in both
  `render.rs` and `panels.rs`, and `mod.rs` re-exported only `panels::hide_panels`, so the
  render copy was unreachable. Behaviour is unchanged (`menus.rs` already called the panels
  version).

Static scanning found no other provably unused function or import in the owned paths; the
test-only helpers sit behind `#[cfg(test)]`. The unused-warning items the brief attributes to
`flamewm-explorer` and `flamewm-settings` are outside this unit's owned paths and were not
touched, to avoid colliding with the units that own those crates.

## Files touched

- crates/flamewm-desktop/src/widgets/render.rs
- crates/flamewm-desktop/src/widgets/sync.rs
- crates/flamewm-desktop/src/widgets/pixels.rs
- crates/flamewm-desktop/src/widgets/mod.rs

UNVERIFIED: no `cargo`/`git` was permitted for this unit, so this is reviewed and formatted
with `rustfmt --check` only. Compilation and runtime behaviour were not executed. Public
types, node ids, layout math and error texts are unchanged.
