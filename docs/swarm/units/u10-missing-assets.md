# Unit u10-missing-assets

Owned paths: `crates/flamewm-ui-document/src`, `crates/flamewm-integrations-linux/src`, this file.

Status: **UNBUILT** — static audit and source edits only. No `cargo`, build, test, Python or
sleep loop was run in this unit; verification and commit belong to the orchestrator.

Route evidence: workspace version `0.1.7` (read directly from `Cargo.toml`; `scripts/version`
invokes Python, so it was not executed). Read `AGENTS.md`, `.opencode/HARNESS.md`,
`.opencode/ROUTES.md`. Routed skills: `flamewm-source-first` (current source and callers first),
`flamewm-rust-systems` (Rust ownership and fallible boundaries). Shared checkout: no `git`,
no `cargo`, no branch/stash/reset/commit operations.

## Audit findings (current source)

Scope: hard fails (`unwrap` / `expect` / `panic!` / process exit, plus unchecked slice indexing)
on loading a font, icon theme, cursor theme, `.desktop` entry, portal, D-Bus service or
`ui/*.html` asset. All production code in the two owned crates was scanned with a top-level
`#[cfg(test)]` boundary so fixture-only `expect` calls are excluded.

No literal asset-load `unwrap`/`expect`/`panic!`/exit path remains in either crate. The remaining
production `expect` calls are unreachable invariants (queue pop after a non-empty wait, a map
entry inserted a few lines above, a layout memo just assigned, constant D-Bus identifiers) and
are recorded below as reviewed, not as asset failures. Unchecked payload indexing in the portal,
NetworkManager and MPRIS parsers is likewise guarded by explicit length checks.

The gap this unit closes is the missing one-line failure record: several asset fallbacks silently
return `None`/empty/default, so a missing or malformed asset leaves no diagnostic and no blast
radius. Rows below record each such site and the fallback it uses.

## Rows (file:line, failure, fix, blast radius)

| file:line | Failure | Fix (fallback) | Blast radius / record |
| --- | --- | --- | --- |
| `crates/flamewm-integrations-linux/src/icon_theme/index.rs:122` (pending) | Unreadable or non-UTF-8 `index.theme` | `ok()?` skips the index; inheritance chain still ends at `hicolor` | One theme's directories ignored; add one-line record |
| `crates/flamewm-integrations-linux/src/icon_theme/theme_roots.rs:36` (pending) | Missing/unreadable `kdeglobals` or GTK `settings.ini` | `ok()` chain yields `None`, so bundled/system default theme is used | Icon theme choice reverts to default; add one-line record |
| `crates/flamewm-integrations-linux/src/themes/mod.rs:88` (pending) | Icon directory cannot be read | `continue` skips that root | All themes under one root omitted; add one-line record |
| `crates/flamewm-integrations-linux/src/themes/mod.rs:95` (pending) | Malformed/unreadable theme `index.theme` | `unwrap_or_default()` yields an empty index; `id` used as display name | Theme listed with no icon directories; add one-line record |
| `crates/flamewm-integrations-linux/src/themes/cursor.rs:45` (pending) | Missing, oversized, escaping or malformed `cursors/left_ptr` | `Err(CursorError)`; settings drops the preview via `.ok()` | Single cursor preview absent, theme remains selectable; add one-line record |
| `crates/flamewm-integrations-linux/src/launcher_entry.rs:277` (pending) | Malformed Unity `.desktop` launcher `Update` | `parse_update` returns `Err`, update ignored | One progress bar not updated; add one-line record |

## Verification

Not run by this unit (see Status). Orchestrator should run the crate checks after the batch lands.
