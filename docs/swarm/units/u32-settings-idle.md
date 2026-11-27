# u32 — Settings app idle and page-switch cost audit

**Status: UNBUILT.** No `git`, `cargo`, build, test, Python or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. Every cost
below is a code-path observation, not a fresh runtime sample.

## Ownership — listed before any edit

- `crates/flamewm-settings/src/lib.rs` (one `obscured` state field).
- `crates/flamewm-settings/src/app_impl_1.rs` (field init, obscurity helpers, page-only projection,
  background-poll projection gates, page-switch dispatch).
- `crates/flamewm-settings/src/runtime.rs` (visibility turn, wake reprojection, panels clone gate).
- This log.

Not edited: `nav.rs`, `actions.rs`, `consts.rs`, `view.rs`, `app_impl_2.rs`, `app_impl_3.rs`,
`launch.rs` (excluded by the brief), and every other file in the settings crate. Route evidence:
`flamewm-source-first`, `flamewm-parallel-execution`, `flamewm-rust-systems`,
`flamewm-performance-proof`, `no-adhoc-scripts`. Governance read: `AGENTS.md`, `Cargo.toml`
(workspace version 0.1.7), `.opencode/{HARNESS,ROUTES}.md`, `.agents/{README,CONTRACTS,JOBS}.md`,
`docs/ARCHITECTURE.md`.

## Rows — file:line, cost, change, risk

| file:line | cost | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-settings/src/runtime.rs:94` (before edit) | `UiControllerEvent::Visibility` was ignored; bus wakes kept calling `settle` → `sync`, which walks and formats every node of every page while the settings window is fully obscured. | Set the new obscurity state on the visibility turn; on uncover, `settle` once. `reproject` still refreshes the authoritative section on every wake but returns before `settle` while hidden, so state arrives on time and the document is repainted once when it can be seen. | Low — matches the existing terminal precedent (`flamewm-terminal/src/main.rs:266-271`); state paths are unchanged and the uncover turn writes every node from the refreshed snapshots. |
| `crates/flamewm-settings/src/app_impl_1.rs:475-522` | `poll_background` called `sync`/`settle` for completed theme and wallpaper jobs even while the window was obscured (full node walk per completion). | Kept the job polling and authoritative writes; gated only the two projection calls on `!self.obscured`. Uncover projects them together with the deferred reprojections. | Low — completions still change settings immediately; only invisible document writes move to the uncover frame. |
| `crates/flamewm-settings/src/runtime.rs:158` | Every bus wake cloned the whole `PanelsSnapshot` (`panels_before`) even for Settings/Displays/Shortcuts wakes, where `panels_refresh_changed` ignores it. | Clone only when `scope == SettingsScope::Panels`; otherwise pass `None`, which keeps `panels_refresh_changed`'s other-scope branch identical. | Low — the comparison result and settle decision are unchanged. |
| `crates/flamewm-settings/src/app_impl_1.rs:162-164` | `SettingsAction::Page` / `AppearanceTab` were followed by the same `settle` as a settings write, so each page switch walked the whole compiled document although only nav ink/marker and page visibility could change. | Dispatch those two actions to a new `sync_page`: clears the inline error, moves the nav marker, writes nav foreground/ink, writes the appearance-tab and page `visible` flags, and runs `sync_startup`/`sync_defaults` when entering the lazy XDG pages. All other projectors are page-independent and their document writes are already equal-value no-ops. | Medium — `sync_page` duplicates a small subset of the excluded `sync_nodes` (app_impl_2.rs:700-773); if a future page adds a new page-dependent node there, this list must be extended. Current node set and ordering were read from the excluded owner and matched exactly. |
| `crates/flamewm-settings/src/lib.rs:103`, `app_impl_1.rs:13`, `app_impl_1.rs:766` | No per-window flag recorded whether the document could be seen. | Added `obscured: bool` (default `false`), `set_obscured` (returns true only on the hidden→visible transition) and `is_obscured`. | Low — default preserves existing behaviour for every test that constructs `SettingsApplication::new`. |
| `crates/flamewm-settings/src/runtime.rs:29-52`, `crates/flamewm-settings/src/control.rs:28-51` | Audited timers/wakeups: the session-bus signal fd and the wallpaper-chooser fd are the only registered sources; both are reactor-readable, there is no `Duration`/`Instant` timer loop, no per-frame poll and no wake while idle. The drain only queues typed `SettingsWake`s. | None — a periodic wake would have to be invented; no safe cut exists in a source that already has only event-driven wakes. | n/a — observation only. |
| `crates/flamewm-settings/src/app_impl_2.rs:28-69,530-545` (read-only evidence) | `poll_theme_jobs` and `poll_open_uri` call `try_recv` on every event/wake; with no job in flight both return immediately without allocation or control traffic. | None — the gating above already removes the invisible document writes; removing the `try_recv` itself would delay background completions. | n/a — excluded file, no edit. |
| `crates/flamewm-settings-core/src/lib.rs:314-364` (read-only evidence) | Each toggle is one authoritative `ApplySettings` round trip returning a new full `SettingsSnapshot`; the settings app cannot avoid that payload from inside this crate. | None — protocol contract owned by another crate. | n/a — out of scope. |

## Verification

UNBUILT by contract: no compiler, test runner, `git` or Python was invoked. Visual identity is argued
from the source of the excluded `sync_nodes` (`app_impl_2.rs:691-1070`) and the document's
equal-value write suppression (`flamewm-render-core/src/model/document.rs:409-491`). The
`runtime.rs` source-scanning tests keep their asserted strings (`application.reproject(scope`,
`application.poll_background(document)`) intact; they were not executed here.

PARTIAL — all three requested cost areas were audited and safe cuts were applied where the owned
files allow it: hidden-window document writes and the per-wake panels snapshot clone are gone, and
page switches no longer rebuild the whole document. No measurement was permitted, the page-only
projector duplicates a small node list owned by an excluded file, and the authoritative snapshot
returned for every toggle stays a protocol-level cost; the orchestrator's build and tests must
confirm both compilation and identical visuals before any performance claim is made.
