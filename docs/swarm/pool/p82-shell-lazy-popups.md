# Unit p82-shell-lazy-popups
STATUS: PARTIAL

Crate flamewm-shell: popups (weather, usage, calendar, network, bluetooth, audio, media) build their document or caches at startup: make construction lazy on first open where it does not change the first-open frame. Read first, then at most 3 edits. Output identical.

## Result
STARTED

Source-inspection re-audit only: no `git`, `cargo`, build, test, Python or sleep loop was run in this
shared checkout. Reference-first: `.vendor/smithay-master/anvil/src/shell/xdg.rs:58-67` tracks a
popup only from `new_popup` (on demand, never at compositor startup) and
`.vendor/kwin-master/src/xdgshellintegration.cpp:73-81` emits `windowCreated(new XdgPopupWindow(..))`
only when a client actually creates one. Both support lazy-on-open construction; nothing was copied
in as a dependency or runtime input.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-shell/src/quick_controls/host/surfaces.rs:10-18, 52-71` | Helper-owned popups audio/network/calendar/weather (and battery) are already lazy: `handles: [Option<SurfaceHandle>; 5]` is documented "created on its first open", and `ensure()` decodes the artifact and creates the surface only on first use, then caches the handle. | NONE — already lazy. | None; first-open frame unchanged. UNBUILT (no edit). |
| `crates/flamewm-shell/src/quick_controls/host/open_request.rs:19-26` | `ensure()` runs before `project_snapshot`, `fitted_rect`, `prepare_commit` and `present`, so a surface created on first open is projected and fitted before its first map — the proof that lazy creation here is pixel-identical to startup creation. | NONE — confirms the gate. | None. UNBUILT (no edit). |
| `crates/flamewm-shell/src/app/popups.rs:118-122`; `crates/flamewm-shell/src/weather_host.rs:129-158` | Weather data is already fetched on first open (`request_open_fetch` + `push_weather_feed`, `fetch_on_open`), not built at startup. | NONE — already lazy. | None. UNBUILT (no edit). |
| `crates/flamewm-shell/src/app/usage.rs:11-38`; `crates/flamewm-shell/src/usage_host.rs:50-67` | Usage fetch is gated by `set_needed(self.surfaces.is_usage_open())`; no credentials are read and no worker starts while the card is closed. | NONE — already lazy. | None. UNBUILT (no edit). |
| `crates/flamewm-shell/src/runtime/lifecycle.rs:51-104`; `crates/flamewm-shell/src/runtime.rs:554-589` | The three shell-owned popups media, usage and bluetooth still `decode_document` + create their surface at startup in `ShellSurfaces::create`, stored as non-optional `SurfaceHandle` fields. These are the only remaining eager popups. | NONE — not changed (see next row). | Deferring them needs an `Option<SurfaceHandle>` refactor. UNBUILT (no edit). |
| `crates/flamewm-shell/src/runtime/start_status.rs:145-149`; `crates/flamewm-shell/src/runtime/bluetooth_surface.rs:14-29`; `crates/flamewm-shell/src/runtime/panels.rs:188,196-197`; `crates/flamewm-shell/src/app/bluetooth_session.rs:54`; `crates/flamewm-shell/src/app/usage.rs:56,60`; `crates/flamewm-shell/src/app/settings.rs:199,263`; `crates/flamewm-shell/src/app/panels.rs:146,154`; `crates/flamewm-shell/src/app/ipc.rs:221`; `crates/flamewm-shell/src/event_router/media_keys.rs:36,39` | Making those three lazy touches 10 by-value handle call sites across 8 production files plus `runtime.rs` and `lifecycle.rs` — a cross-cutting change far beyond the three-edit budget, unverifiable here without a build. | NONE — deliberately not attempted. | High: cannot be compile- or pixel-verified in this worker; no gate or frame may be altered. UNBUILT (no edit). |
| `docs/swarm/pool/p34-shell-popup-idle.md:24` | Prior precedent records retained popup surfaces as a deliberate native-unload tradeoff and warns that recreating them changes first-frame pixels. Confirms the risk of touching this lifetime in a shared, unbuilt batch. | NONE — reference only. | None. UNBUILT (no edit). |
| `docs/swarm/pool/p82-shell-lazy-popups.md:1-8` | Unit file carried a prior result; this pass reset it to `STATUS: OPEN` with a bare `STARTED` marker, re-audited the source, then finalized it. | EDIT (UNBUILT): status `PARTIAL`, `STARTED` marker expanded to this inventory and verdict. Documentation only — no production file changed. | None — docs only; no code path touched. UNBUILT (no edit to production). |

## Verification

No compiler, test runner, `git`, Python or sleep loop was invoked, so no runtime measurement backs
this result; every row is static source evidence marked UNBUILT. No production file was changed, so
no gate was loosened, no timer was added and no pixel can move. The only edit is this unit file.

PARTIAL — Four of the seven popups (audio, network, calendar, weather) already construct their
documents lazily on first open via `HelperSurfaces::ensure` before projection and present, and weather
and usage data are already fetched only while open, so those paths already satisfy the brief with no
change. The three shell-owned popups (media, usage, bluetooth) still decode their documents at startup
in `ShellSurfaces::create`; converting them to lazy construction is a cross-cutting
`Option<SurfaceHandle>` refactor spanning ten call sites across eight files, exceeds the three-edit
budget, cannot be compile- or pixel-verified in this shared unbuilt checkout, and carries the
first-open-frame risk that p34 records. No production edit was therefore made. Every row is UNBUILT
and awaits the orchestrator's build, tests and visual check.
