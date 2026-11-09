# Design tokens: audit and plan

Status: audit and plan only. No CSS, Rust or settings code changed in this pass. Base commit: `20449c57` (origin/main).

## Method

`/tmp/claude-1000/tokens/audit.py` scans `ui/**/*.css` and the inline `<style>` blocks of `ui/**/*.html`.
It counts px values in `font-size`, `border-radius`, `gap`/`row-gap`/`column-gap`, `padding` and `margin`.
Each row counts declarations that contain the value. Values inside `var()` are not counted.
`crates/flamewm-ui-core/src/style.rs` is not scanned by the regex; its native sizes are listed in section 4.

## 1. Existing token state (evidence)

- `ui/base/tokens.css` already is the shared sheet. `ui/base/base.css`, `ui/flamewm.css` and `ui/shell/flamewm.css` import it. Shell pages link it. `ui/settings.html` reaches it through `base/base.css`.
- `ui/task-manager/task-manager.css` has its own `:root` and does not import `tokens.css`.
- `tokens.css` already defines a second scale: `--radius-sm..xl` (3, 6, 9, 11 px), `--font-xs..xl` (10, 11, 12, 13, 14, 16 px), `--space-xs..xl` (4, 6, 8, 10, 12 px). Shell, settings, flamewm and assistant CSS use these names.
  These values do not match the requested scale, so the names cannot be remapped without changing rendering. They must be retired one surface at a time.
- `ui/**/*.css` contains no `calc(`.
- Runtime document variables are colour-only: `RuntimeDocument::set_color_variable` (`crates/flamewm-render-core/src/model/style.rs:291`) and `UiDocumentAccess::color_variable` (`crates/flamewm-ui-document/src/template.rs`).
- Length variables are inlined at compile time: `inline_length_tokens` (`crates/flamewm-render-compiler/src/compile.rs:949`) substitutes `var(--name)` with its `:root` string.

## 2. Literal audit (`ui/**`, base 20449c57)

| property | value | count | files (ui/ prefix trimmed) |
|---|---|---:|---|
| border-radius | 4 | 30 | assistant/osd.css, desktop/desktop.css, explorer/explorer.css, flamewm.css +6 |
| border-radius | 8 | 25 | archiver/archiver.css, desktop/desktop.css, finder/finder.css, flamewm.css +7 |
| border-radius | 999 | 18 | lock/lock.css, shell/audio.html, shell/flamewm.css, shell/network.html +1 |
| border-radius | 6 | 16 | desktop/desktop.css, explorer/explorer.css, shell/audio.html, shell/calendar.html +1 |
| border-radius | 12 | 15 | assistant/assistant.css, desktop/desktop.css, finder/finder.css, imageviewer/imageviewer.css +4 |
| border-radius | 5 | 13 | desktop/desktop.css, explorer/explorer.css, finder/finder.css, settings.css +1 |
| border-radius | 3 | 12 | archiver/archiver.css, desktop/desktop.css, explorer/explorer.css, finder/finder.css +3 |
| border-radius | 2 | 11 | assistant/assistant.css, explorer/explorer.css, flamewm.css, shell/flamewm.css +2 |
| border-radius | 18 | 10 | assistant/assistant.css, desktop/desktop.css, shell/audio.html, shell/calendar.html +2 |
| border-radius | 16 | 8 | assistant/assistant.css, finder/finder.css, settings.css, shell/network.html +1 |
| border-radius | 10 | 8 | assistant/assistant.css, desktop/desktop.css, explorer/explorer.css, finder/finder.css |
| border-radius | 24 | 7 | assistant/assistant.css, flamewm.css, shell/audio.html, shell/flamewm.css +2 |
| border-radius | 11 | 4 | explorer/explorer.css, shell/dock-label.html |
| border-radius | 15 | 3 | desktop/desktop.css, flamewm.css, shell/flamewm.css |
| border-radius | 14 | 3 | assistant/assistant.css, assistant/osd.css, settings.css |
| border-radius | 13 | 3 | settings.css, shell/flamewm.css |
| border-radius | 0 | 2 | desktop/desktop.css, explorer/explorer.css |
| border-radius | 2.5 | 2 | shell/osd.html |
| border-radius | 1 | 1 | flamewm.css |
| border-radius | 28 | 1 | assistant/osd.css |
| border-radius | 17 | 1 | assistant/assistant.css |
| border-radius | 9 | 1 | desktop/desktop.css |
| border-radius | 164 | 1 | desktop/desktop.css |
| border-radius | 26 | 1 | desktop/desktop.css |
| border-radius | 7 | 1 | desktop/desktop.css |
| font-size | 12 | 46 | archiver/archiver.css, desktop/desktop.css, explorer/explorer.css, finder/finder.css +8 |
| font-size | 13 | 31 | archiver/archiver.css, desktop/desktop.css, explorer/explorer.css, finder/finder.css +7 |
| font-size | 11 | 30 | assistant/osd.css, desktop/desktop.css, explorer/explorer.css, finder/finder.css +4 |
| font-size | 14 | 26 | archiver/archiver.css, assistant/osd.css, desktop/desktop.css, explorer/explorer.css +7 |
| font-size | 15 | 9 | desktop/desktop.css, explorer/explorer.css, finder/finder.css, flamewm.css +3 |
| font-size | 9 | 6 | desktop/desktop.css, flamewm.css, shell/flamewm.css |
| font-size | 10 | 6 | desktop/desktop.css, finder/finder.css, shell/weather.html, task-manager/task-manager.css |
| font-size | 20 | 5 | assistant/assistant.css, finder/finder.css, flamewm.css, shell/flamewm.css |
| font-size | 17 | 5 | assistant/assistant.css, desktop/desktop.css, settings.css, shell/network.html |
| font-size | 18 | 4 | flamewm.css, shell/flamewm.css |
| font-size | 16 | 4 | desktop/desktop.css, explorer/explorer.css, task-manager/task-manager.css |
| font-size | 22 | 3 | desktop/desktop.css, flamewm.css, shell/flamewm.css |
| font-size | 21 | 2 | flamewm.css, shell/flamewm.css |
| font-size | 24 | 2 | flamewm.css, shell/flamewm.css |
| font-size | 26 | 2 | desktop/desktop.css, settings.css |
| font-size | 44 | 2 | shell/battery.html, shell/weather.html |
| font-size | 52 | 1 | desktop/desktop.css |
| font-size | 50 | 1 | desktop/desktop.css |
| gap | 2 | 22 | assistant/assistant.css, assistant/osd.css, desktop/desktop.css, explorer/explorer.css +6 |
| gap | 8 | 16 | archiver/archiver.css, desktop/desktop.css, explorer/explorer.css, finder/finder.css +3 |
| gap | 14 | 14 | assistant/assistant.css, desktop/desktop.css, finder/finder.css, flamewm.css +4 |
| gap | 6 | 13 | desktop/desktop.css, explorer/explorer.css, finder/finder.css, shell/network.html +2 |
| gap | 10 | 13 | archiver/archiver.css, desktop/desktop.css, explorer/explorer.css, finder/finder.css +4 |
| gap | 0 | 10 | desktop/desktop.css, finder/finder.css, flamewm.css, shell/audio.html +4 |
| gap | 12 | 10 | archiver/archiver.css, desktop/desktop.css, explorer/explorer.css, finder/finder.css +1 |
| gap | 9 | 9 | explorer/explorer.css, flamewm.css, shared/menu.css, shell/flamewm.css +1 |
| gap | 1 | 9 | explorer/explorer.css, finder/finder.css, flamewm.css, shell/flamewm.css |
| gap | 3 | 8 | assistant/assistant.css, desktop/desktop.css, flamewm.css, settings.css +1 |
| gap | 5 | 8 | explorer/explorer.css, finder/finder.css, flamewm.css, shell/flamewm.css |
| gap | 4 | 8 | archiver/archiver.css, desktop/desktop.css, explorer/explorer.css, shell/audio.html +2 |
| gap | 7 | 7 | finder/finder.css, flamewm.css, settings.css, shell/flamewm.css +1 |
| gap | 18 | 6 | flamewm.css, shell/flamewm.css |
| gap | 13 | 4 | flamewm.css, shell/flamewm.css |
| gap | 11 | 1 | flamewm.css |
| gap | 16 | 1 | settings.css |
| gap | 24 | 1 | shell/calendar.html |
| margin | 0 | 11 | shell/audio.html, shell/battery.html, shell/flamewm.css, shell/notification-center.html +1 |
| margin | 5 | 4 | flamewm.css, shared/menu.css, shell/flamewm.css |
| margin | 8 | 4 | settings.css, shell/audio.html |
| margin | 14 | 3 | settings.css, shell/audio.html |
| margin | 2 | 3 | shell/flamewm.css |
| margin | 16 | 2 | shell/notification-center.html |
| margin | 6 | 1 | settings.css |
| margin | 1 | 1 | settings.css |
| margin | -2 | 1 | shell/flamewm.css |
| margin | 4 | 1 | explorer/explorer.css |
| margin | 10 | 1 | shell/notification-center.html |
| padding | 0 | 70 | archiver/archiver.css, desktop/desktop.css, explorer/explorer.css, flamewm.css +7 |
| padding | 14 | 29 | assistant/assistant.css, desktop/desktop.css, explorer/explorer.css, finder/finder.css +7 |
| padding | 8 | 26 | assistant/assistant.css, assistant/osd.css, desktop/desktop.css, explorer/explorer.css +4 |
| padding | 10 | 25 | archiver/archiver.css, assistant/assistant.css, desktop/desktop.css, explorer/explorer.css +6 |
| padding | 12 | 25 | archiver/archiver.css, assistant/assistant.css, assistant/osd.css, desktop/desktop.css +6 |
| padding | 5 | 19 | assistant/assistant.css, explorer/explorer.css, flamewm.css, settings.css +2 |
| padding | 9 | 18 | assistant/assistant.css, explorer/explorer.css, flamewm.css, settings.css +1 |
| padding | 16 | 16 | explorer/explorer.css, finder/finder.css, settings.css, shell/notification-center.html +1 |
| padding | 6 | 16 | assistant/assistant.css, desktop/desktop.css, explorer/explorer.css, finder/finder.css +6 |
| padding | 13 | 13 | assistant/assistant.css, explorer/explorer.css, flamewm.css, settings.css +1 |
| padding | 2 | 11 | assistant/assistant.css, flamewm.css, settings.css, shell/flamewm.css |
| padding | 4 | 11 | assistant/assistant.css, finder/finder.css, settings.css, shell/audio.html +3 |
| padding | 3 | 7 | desktop/desktop.css, flamewm.css, shell/flamewm.css |
| padding | 18 | 7 | assistant/assistant.css, finder/finder.css, flamewm.css, shell/flamewm.css |
| padding | 7 | 7 | explorer/explorer.css, flamewm.css, settings.css, shell/flamewm.css |
| padding | 1 | 7 | flamewm.css, shell/flamewm.css, task-manager/task-manager.css |
| padding | 23 | 4 | flamewm.css, shell/flamewm.css |
| padding | 27 | 4 | flamewm.css, shell/flamewm.css |
| padding | 11 | 4 | desktop/desktop.css, explorer/explorer.css, flamewm.css, shell/flamewm.css |
| padding | 20 | 4 | finder/finder.css, flamewm.css, shell/flamewm.css |
| padding | 24 | 2 | flamewm.css, shell/flamewm.css |
| padding | 35 | 2 | flamewm.css, shell/flamewm.css |
| padding | 28 | 2 | settings.css, shell/notification-center.html |
| padding | 30 | 2 | settings.css |
| padding | 15 | 1 | desktop/desktop.css |
| padding | 36 | 1 | finder/finder.css |
| padding | 22 | 1 | finder/finder.css |
| padding | 17 | 1 | shell/calendar.html |
| padding | 32 | 1 | shell/notification-center.html |

Near-duplicates and off-grid values (candidates only; each needs a visual review before change):

- Font size: 14 px (26 uses) is off the requested scale. Candidates are 13 or 15. 9 px (6) and 10 px (6) sit below `--fs-xs`. 16, 17, 18, 20, 21, 22, 24, 26 px are mostly component sizes and stay literal unless they match a token exactly.
- Padding: 14 px (29), 10 px (25), 5 px (19), 9 px (18), 13 px (13), 7 px (7) and 1 px (7) are off the 4-pt grid. 12, 8, 16, 6 (grid-off), 4 and 2 are on or near it.
- Gap: 2, 8, 6, 10, 12 and 0 dominate. 6 and 10 are off the requested grid (`--sp-*` has 8 and 12).
- Radius: 4 (30), 8 (25), 6 (16), 12 (15), 5 (13), 3 (12), 2 (11), 10 (8), 16 (8), 18 (10). Pill `999` (18) is already exact.
  Near-duplicates: 3/4/5 and 6/8. The requested base values are 4, 8 and 12, which covers the dominant values.

## 3. Proposed token set (not applied)

Type scale: `--fs-xs 11px`, `--fs-sm 12px`, `--fs-md 13px`, `--fs-lg 15px`, `--fs-xl 18px`, `--fs-2xl 22px`.
Spacing: `--sp-1 2px`, `--sp-2 4px`, `--sp-3 8px`, `--sp-4 12px`, `--sp-5 16px`, `--sp-6 24px`.
Radii: `--corner: 1`; `--r-sm: calc(4px * var(--corner))`; `--r-md: calc(8px * var(--corner))`; `--r-lg: calc(12px * var(--corner))`; `--r-pill: 999px`.

The radius base is 4/8/12, not the 6/10/14 in the brief. 4/8/12 are the dominant values (30, 25 and 15 uses), so the default render stays unchanged for most elements.

Blocking issue: `calc(... * var(--corner))` is untested. Compile-time inlining would need a probe test in `flamewm-render-compiler` before any CSS change.

## 4. Native mirror (`crates/flamewm-ui-core/src/style.rs`)

- `ShellMetrics.radius` = 4 (`style.rs:84`) equals `--r-sm`.
- `SettingsMetrics.card_radius` = 5 (`style.rs:133`) is off-scale. Candidate: 4 or 8, needs visual review.
- `Material.radius` default = 0 (`style.rs:34`).
- No font-size or spacing constants were audited here. Mirror them after the CSS scale is accepted.

## 5. Blockers

1. Live `appearance.cornerStyle` cannot reach pages through the current mechanism. Pages receive colour variables only, and lengths resolve at compile time. Options: (a) add a numeric runtime variable beside `set_color_variable` across `flamewm-render-core`, `flamewm-ui-document` and `flamewm-ui-x11`, which is a new mechanism needing owner sign-off; (b) recompile the affected pages per change, which is not live. Decision needed before the knob work starts.
2. `calc(... * var(--corner))` through `inline_length_tokens` is unproven. Required probe: a unit test in `flamewm-render-compiler` covering `--corner: 1` and `0.5`.
3. `docs/AGENT_PLAYBOOK.md` does not exist in this repository.
4. Scope boundary for this pass: no CSS was migrated. The dock/taskbar, settings, task-manager and other surfaces are untouched.
5. Scout rows: row 1 (token set) is the basis of this plan. Row 5 is a D-Bus notification timeout, not a token; it is out of scope for this task.

## 6. Next steps (in order)

1. Decide blocker 1 (runtime numeric variable vs recompile).
2. Add the `calc(var)` probe test, then move the token block into `ui/base/tokens.css`.
3. Migrate CSS surface by surface, changing only exact px matches. Commit per surface.
4. Add `appearance.cornerStyle` (`sharp|default|round` to 0.5|1|1.5) in `flamewm-platform` settings, with tests modelled on `dock_settings_tests.rs`, plus entries in `flamewm-settings` `consts.rs` and `actions.rs`.
5. Gate: `FLAMEWM_TOOLCHAIN_BIN=/run/media/linsaftw/SDD/rust189/bin scripts/gate flamewm-platform flamewm-settings flamewm-ui-core flamewm-shell`.
6. Xvfb screenshots of Settings and the dock at each cornerStyle value (display 116-119).
