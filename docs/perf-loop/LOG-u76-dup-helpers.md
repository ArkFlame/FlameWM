# u76-dup-helpers log

Batch eleven over duplicated small helpers in `crates/**/src`. `git` and `cargo` are forbidden in this shared
checkout, so this slice is source-verified only; the orchestrator builds and commits.

## What changed

- **Text truncation (`flamewm-api::format`).** Added `char_prefix` (borrowed, zero-allocation prefix cut on a char
  boundary) and `truncate_chars` (first `keep` chars plus `…`, with the byte-length fast path). Five local copies are
  gone: `notification_center::clip`, `protocols::clipboard::preview` (kept as `truncate_chars(line, PREVIEW_CHARS - 1)`
  after its existing full-fit guard), `explorer::progress::name_of`, `desktop-core::presentation::truncate_final` and
  `shell::projection::media::fit_media_title` (now `char_prefix(..).trim_end()`). Expected composition shift: the
  per-projection body/summary and per-label paths no longer collect into a temporary `String` char by char; one char
  walk with one allocation remains.
- **Calendar name tables (`flamewm-api::format`).** Added `MONTH_NAMES`, `WEEKDAY_NAMES`, `month_name` and
  `weekday_name`. Deleted the four full-month tables (`shell/projection/clock`, `desktop/widgets/sync`,
  `widgets-core/widget_text`, `lock/clock`) and the two weekday tables (`widget_text`, `lock/clock`) and routed each
  site through the shared accessor. Same output; mixed valid/invalid handling preserved (`saturating_sub` keeps month 0
  as January, out-of-range becomes `Unknown`).
- **Hex colour parsing (`flamewm-api::color`).** `settings::actions::accent_color` and
  `terminal::fetch::truecolor` now call `parse_hex_rgb` instead of hand-splitting `#RRGGBB`; the terminal's strict
  six-digit check and the settings error string are unchanged.
- **Home directory (`flamewm-api::paths`).** `explorer::home_dir` and `terminal::app::home_dir` now start from
  `paths::home_dir` and keep their existing `is_dir` / `/` fallback filters, so no `$HOME` parsing is duplicated.

## Composition reasoning

These helpers run on projection/label paths (per clock tick, per notification body, per desktop icon, per media
title) and on one-off settings/config parses. The duplicate tables cost static data plus a separate `chars()` walk in
the truncation copies; the merged forms keep one byte-length fast path and one decode, so the label share shrinks
without changing rendered text. No layout, render, timer or wakeup behavior changed.

## Verification

Static: all touched crates already depend on `flamewm-api` (`widgets-core`, `desktop`, `desktop-core`, `shell`,
`wm-wayland`, `explorer`, `terminal`, `settings`, `lock`); no new dependency. No cargo/git run (shared checkout).
