# MATE desktop (libmate-desktop) learnings for FlameWM

Snapshot: 2026-11-08. Source: read-only reference in `.vendor/mate-desktop-master/`.
Licence: libmate-desktop is GPL-2.0 or LGPL (`COPYING.LIB` is LGPL text; `COPYING` is GPL). Only ideas are used; code is rewritten in Rust, never copied.
FlameWM paths are current `origin/main` (`478cc386`).

Verdict key: adopt, defer, reject. Effort: S < 1 day, M 1-3 days, L > 3 days.

## Headline

MATE's shared library is a toolkit of helpers (thumbnails, desktop entries, RandR, background, dconf/gsettings wrappers, colour picker). FlameWM already covers the same ground in Rust crates: thumbnails (`flamewm-explorer/src/thumbs.rs`), desktop entries (`flamewm-applications`), RandR and outputs (`flamewm-wm-x11/src/wm/outputs.rs`, `flamewm-x11-desktop`), and typed settings. The only item worth a later look is persisted thumbnail failures, and it is rejected for now because it adds disk writes.

## Learnings

| # | Idea | What MATE does (vendor path:line) | FlameWM today (path:line) | Gap? | Effort | Mem/CPU | Verdict and reason |
|---|---|---|---|---|---|---|---|
| 1 | Thumbnail failure marker on disk (freedesktop `fail/` dir) | Writes a failure entry in the thumbnail cache so a broken file is not decoded again after restart (`libmate-desktop/mate-desktop-thumbnail.c:698-706` `thumbnail_failed_path`, `:744-749` `lookup_failed_thumbnail_path`) | In-memory failure cache, LRU-bounded at 256 entries and 256 KiB: `flamewm-explorer/src/thumbs.rs:6-7`, `:30-32` | Partial (no persistence across restarts) | M | Disk writes plus a lookup per file on the worker. Real cost on large folders. | Reject for now. The in-memory cache already bounds memory and avoids repeated decode within a session. Persisting failures adds disk I/O and a cache-dir contract for a small gain. |
| 2 | Thumbnail validity by mtime and URI | Validates the thumbnail against file mtime and URI (`mate-desktop-thumbnail.c:715-740` `validate_thumbnail_path`, `:717-724`) | Keyed by path plus mtime and size: `flamewm-explorer/src/thumbs.rs:2-3` | No | n/a | n/a | Already done. |
| 3 | Desktop-entry launch with environment and field codes | `mate_desktop_item_launch_with_env` (`libmate-desktop/mate-desktop-item.c:2179-2195`) | Field-code expansion with tests: `flamewm-applications/src/lib.rs:446-455` (`expand_exec`), `:965-972` | No | n/a | n/a | Already done. |
| 4 | RandR output config and labeler | `mate-rr.c`, `mate-rr-config.c`, `mate-rr-labeler.c`, `mate-rr-output-info.c` (monitor layout, mode, rotation, saved profiles) | Outputs and layout: `flamewm-wm-x11/src/wm/outputs.rs` (11.2K), `flamewm-x11-desktop`; no saved monitor profiles found | Partial (saved profiles and labeler) | L | Only when the user applies a config. No idle cost. | Defer. A saved-profile feature needs a settings owner and a UI. Not a bug fix. |
| 5 | Per-monitor background handling | `mate-bg.c` (`mate-bg` ships one background per monitor, no per-workspace state) | Per-monitor wallpaper with fit and stamp: `flamewm-desktop/src/wallpaper_fit.rs`, `wallpaper_stamp.rs` | No | n/a | n/a | Already done. Per-workspace wallpaper is not a MATE feature; see the xfce file, row 17. |
| 6 | Crossfade on background change | `mate-bg-crossfade.c` | Not found | Visual feature | M | Blends two decoded images for a short time | Reject. Adds animation and changes the visual look, which project rules forbid. |
| 7 | Colour selection dialog and HSV helpers | `mate-colorsel.c`, `mate-hsv.c` | Theme picker in `flamewm-settings/src/theme_picker.rs` | Not comparable | n/a | n/a | Reject. FlameWM uses its own theme model. |
| 8 | EDID parsing and PnP ID lookup | `edid.h`, `edid-parse.c`, `pnp.ids` | Not found in `crates/**` (checked by name) | Unverified value | M | Parses one blob per output on change | Defer. Only useful for monitor naming in the labeler (row 4). |
| 9 | GSettings schema defaults and dconf wrappers | `mate-gsettings.c`, `mate-dconf.c`, `schemas/` | Typed settings: `flamewm-settings-core`, `flamewm-platform/src/settings.rs` | No | n/a | n/a | Reject as a rewrite. The typed core is already the owner. |
| 10 | Keybinding and power/lock conventions | Schema-level only (`schemas/`); not a code path in libmate-desktop | Shortcuts and lock exist in `flamewm-platform/src/shortcuts.rs`, `flamewm-lock` | Not checked in depth | n/a | n/a | Not compared in depth. Out of scope for this pass. |

## Rejected and why (summary)

- Persisted thumbnail failures: disk writes on the thumbnail path; in-memory cache already bounded.
- Crossfade: visual change, forbidden by project rules.
- Colour dialog and GSettings wrappers: FlameWM already has the owner.

## Next actions

1. If monitor profiles are requested, design saved RandR profiles (row 4) with the settings owner first.
2. Revisit EDID naming (row 8) only together with row 4.

Skipped or not checked: `mate-desktop-item` launch edge cases, `mate-languages.c`, `mate-image-menu-item.c` (GTK-only), accessibility and i18n helpers, power and screensaver code (not in this tree), and runtime behaviour. Risk: row 4 and row 10 are not verified against FlameWM code.
