# Unit u10-missing-assets

Owned paths: `crates/flamewm-ui-document/src`, `crates/flamewm-integrations-linux/src`, this file.

Status: **UNBUILT** — static audit and source edits only. No `cargo`, build, test, Python or
sleep loop was run in this unit; verification and commit belong to the orchestrator.

Route evidence: workspace version `0.1.7` was read directly from `Cargo.toml` (`scripts/version`
invokes Python, so it was not executed). Read `AGENTS.md`, `.opencode/HARNESS.md`,
`.opencode/ROUTES.md`. Routed skills: `flamewm-source-first` (current source and callers first),
`flamewm-rust-systems` (Rust ownership and fallible boundaries). Shared checkout: no `git`,
no `cargo`, no branch, stash, reset, rebase or commit operations.

## Audit findings (current source)

Scope: hard fails (`unwrap` / `expect` / `panic!` / process exit, plus unchecked slice indexing)
when a font, icon theme, cursor theme, `.desktop` entry, portal, D-Bus service or `ui/*.html`
asset is missing or malformed. All production code in the two owned crates was scanned with a
top-level `#[cfg(test)]` boundary so fixture-only `expect` calls are excluded.

No literal asset-load `unwrap` / `expect` / `panic!` / exit path remains in either crate; these
crates had already been hardened. The gap was the missing failure record: several asset fallbacks
returned `None` / empty / default silently, so a missing or malformed asset produced no one-line
diagnostic and no blast radius. This unit adds those records without changing which fallback is
chosen. Unreachable invariants and guarded payload indexing were reviewed and left untouched
(listed below).

## Rows (file:line, failure, fix, blast radius)

| file:line | Failure | Fix (fallback) | Blast radius / one-line record |
| --- | --- | --- | --- |
| `crates/flamewm-integrations-linux/src/icon_theme/walk.rs:144` | `index.theme` of an inherited icon theme cannot be opened (missing/permission) | `File::open(...).ok()?` skipped the root silently; kept as skip-root fallback | One inherited theme's `Inherits` list ignored; record names theme, path and error |
| `crates/flamewm-integrations-linux/src/icon_theme/walk.rs:166` | `index.theme` unreadable or invalid UTF-8 | `read_to_string(...).ok()?` skipped the root silently; kept as skip-root fallback | One theme's icon directories ignored; record names theme, path and error |
| `crates/flamewm-integrations-linux/src/icon_theme/theme_roots.rs:36` | KDE `kdeglobals` / GTK `settings.ini` missing or unreadable | `ok()` chain fell through silently; kept fall-through to GTK, then bundled default | Configured icon theme reverts to default; one record per failed config read |
| `crates/flamewm-integrations-linux/src/themes/mod.rs:74` | Theme root directory cannot be enumerated | `let Ok(read) = ... else { continue }` skipped root silently; kept skip-root fallback | All themes under one root omitted; record names the root |
| `crates/flamewm-integrations-linux/src/themes/mod.rs:98` | Theme `index.theme` present but unreadable/malformed | `unwrap_or_default()` still yields an empty index and the directory id as name | Theme listed with no icon directories; record names path and theme id |
| `crates/flamewm-integrations-linux/src/themes/cursor.rs:45` | `cursors/left_ptr` missing, oversized, escaping the theme root or malformed Xcursor | `Err(CursorError)` unchanged; settings already drops the preview with `.ok()` | One cursor preview absent, theme still selectable; record names theme and error |
| `crates/flamewm-integrations-linux/src/launcher_entry.rs:285` | Malformed Unity `.desktop` launcher `Update` signal | `parse_update` `Err` was ignored; kept ignore-the-update fallback | One taskbar progress bar stays unchanged; record names the sender |

## Reviewed, no change needed

- `icon_service.rs:786,793,795,809` `expect("queued icon")` — guarded by the preceding wait loop
  that returns on `stopped` and only exits with a non-empty high/low queue.
- `launcher_entry.rs:182` `expect("entry inserted above")` — the key is inserted a few lines above
  when absent; malformed `application://…desktop` URIs return early instead.
- `template.rs:58` `expect("layout memo was just filled")` — the memo is assigned immediately above.
- `bluetooth_agent.rs:318`, `notifications.rs:656,677` `expect(...)` — constant, valid D-Bus names.
- `portal_file_chooser.rs:307,328`, `network_manager/reply.rs:257,324,340`, `mpris.rs:673` —
  direct payload indexing, each preceded by an explicit length check that returns an error.
- D-Bus service absence: `mpris.rs:154` starts offline and retries, `power.rs:169`,
  `power_profiles.rs:157`, `bluetooth.rs:409` and `network_slot.rs:30` degrade to unavailable
  snapshots. The `?` in `lib.rs:88-92` can therefore only fail on local socket-pair creation.
- Icon raster failures (`icons/mod.rs:968-1025`) return `RasterError`; theme discovery
  (`themes/mod.rs:60-115`) skips unusable entries; `icon_theme/index.rs:117-124` already uses
  `ok()?` plus a `hicolor` final fallback.
- Fonts and `ui/*.html` assets are loaded in `flamewm-render-x11` and the application build
  scripts/tests, outside this unit's owned paths; no font/HTML load path exists in either owned
  crate, so nothing in scope needed conversion.

## Verification

Not run by this unit (see Status). The orchestrator should run the crate checks after the batch
lands; edits are marked UNBUILT.
