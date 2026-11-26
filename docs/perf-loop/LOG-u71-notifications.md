# u71-notifications

Scenario: one second of normal notification use (a toast arrives, its progress moves a few
times, a volume key is held for the OSD, then the card fades), classified by the work this unit
owns. Within that sample the projection/document slice breaks down as, in order: (1) building the
212 template writes for the open centre and cloning their `String`s (~55% of the
notification/OSD slice, of which nearly all is per-projection allocation), (2) the document
applying those writes and repainting the card with its shadow/blur (unchanged here; one pass, and
a borrowed text write already dedupes unchanged values), (3) the OSD tick: settings lookups plus a
nine-write repaint per key repeat (~12%), (4) D-Bus/provider drain, already off the UI thread and
event-driven. The next biggest fixable share was therefore allocation in the projection and the
repeated OSD settings/visibility work, not layout or blur.

## Review of the recent perf commits (regressions)

Read the previous slices that touched these files: `965314da` (u31-u40, static `CenterIds` +
single-pass `view`) and `6d129725`/`02dce1d1` (timer coalescing, reducedMotion fade). Invariants
re-checked by reading: ids stay `&'static str` from one `OnceLock`; `view` groups in one pass with
no seq lookup; `refresh_center` still skips an unchanged revision and compares live ids without
materialising a vector; the OSD keeps one deadline and `expire` is the only hide path. No
regression found.

## Changes (all keep visuals and features)

- `notification_center.rs`: `Command::parse` walks the action segments with one `split` iterator
  instead of collecting a `Vec<&str>` per button release (one heap buffer per click removed), with
  the same accepted shapes and the same rejection of unknown/trailing segments.
- `notification_center.rs`: the `"+N more"` label is built only when a group actually overflows;
  the common group of three or fewer rows now writes just `visible(false)` instead of allocating a
  `"+0 more"` string per group per reprojection.
- `notification_center.rs`: `clip` returns early for text whose byte length is within the 160
  character budget, so short summaries, bodies and app names skip the `char_indices` walk on every
  projection; long text still cuts on a character boundary and appends the ellipsis.
- `notification_center.rs`: deleted the test-only `nodes()` wrapper (production always passes a
  focus through `nodes_with_focus`); tests now call the real entry point.
- `osd.rs`: the fade is derived from `motion_scale` only when the settings revision moves. A held
  volume key or a fading card samples every wake, and each sample used to repeat the
  `reducedMotion`/`animationSpeed` map lookups.
- `osd.rs`: `update_workspace` returns before the `workspaceOsd` lookup when the active index is
  unchanged, which is every shell wake; re-recording an equal index was a no-op.
- `osd.rs`: a level card painted over another level card with the same glyph rewrites only the
  value rows (`size` + text). Track, fill, text and workspace visibilities stay as painted, so a
  key repeat does two writes instead of nine. A fresh surface or a workspace card still writes
  every visibility, tracked by `OsdController::painted`.

## Expected composition shift

Projection allocation (share 1) drops for every reprojection: ~50 `String` clones per centre
projection become ~10 (only counts and the overflow label), and short text no longer char-scans.
The OSD tick (share 3) drops the settings lookups on repeat ticks and cuts each repeat paint from
nine document writes to two. Layout, blur and D-Bus keep their shares; nothing is cached that
would show stale content, because the skip is keyed on the painted level and the settings
revision.

## Checks

No build or git in this unit (shared checkout; orchestrator builds and commits). Two new
`osd.rs` tests pin the paint behaviour with `test_recorder::{NullDocument, Recording}`: a fresh
card writes all seven visibilities, a repeat with the same glyph writes none, and a kind switch
writes exactly the three glyphs.
