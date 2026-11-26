# u52-shell-async-audit log

Batch seven over the shell crate (`crates/flamewm-shell/src/**`). `git` and `cargo` are forbidden in this shared
checkout, so the review of the landed slices is source-based and no build was run here: the orchestrator builds,
tests and commits.

## Audit: blocking calls left on the shell UI thread

Grepped the production modules for `std::fs`, `Command::new`/`wait`, and synchronous D-Bus calls:

- Weather (`weather_host`/`weather_provider`, `weather_auto`), usage (`usage_host`), desktop shortcuts
  (`event_router/drag`) and icon rasterization (`icon_loader`) already run on workers; the shell only polls
  their channels in `tick`.
- Quick-control helper pipes are nonblocking (`try_wait`, `WouldBlock`, reactor fds), and polkit reads through the
  reactor `ChildPipe`.
- **Clipboard was the remaining blocking path on the shell UI thread.** `app/clipboard_session.rs` opened a fresh
  `ControlClient` and did a synchronous `call` for List on Super+Shift+V, Select on Return and Clear on Delete. A
  slow/busy compositor stalled the shell for the duration of the bus round trip (plus the connect handshake).

## Next worst hotspots and what changed

- **Clipboard writes moved off the UI thread (`app/clipboard_session.rs`, `control_actions.rs`, `app/timers.rs`).**
  List/Select/Clear now go through the bounded `MutationDispatcher` queue the shell already owns, so the UI thread
  only enqueues. The history query is correlated by queue id and its reply is captured in `drain_results_with_settings`
  into a thread-local outcome queue; `ShellLoop::tick` applies it (`apply_clipboard_outcomes`). The popup now opens
  immediately with the current model and fills in when the reply arrives, so its visuals and keys are unchanged;
  the only difference is that the first frame after open may be the empty/previous list for one bus round trip.
  Refused enqueues become an immediate error outcome (logged), matching the old nonfatal failure path.
- **Dock hover label no longer allocates per hover event (`app/dock.rs`).** `sync_dock_label` built a `String` copy of
  the hovered app name and cloned it again for the equality check on every call. The label name is now borrowed from
  `dock_last` and compared as `&str`; only the single `document.text` write and the stored `label_shown` still own a
  string when the label actually changes. Pointer motion is the bulk of the event stream, so hovering across dock
  slots no longer churns two `String` allocations per event.
- **Weather feed is built once per changed tick (`app/weather.rs`).** `tick_weather` previously called
  `WeatherHost::feed()` three times (tray projection, popup push, control publish) on every changed/refresh tick, and
  `push_weather` cloned it again into its cache: four deep clones of the weather state. The tick now builds one feed,
  projects from it, encodes that same value for the desktop widget and hands it to the cache/send path, removing two
  full state clones per weather update (refresh, location change, popup open).
- **Custom Start icon decode is memoized (`settings_projection/start.rs`).** Every Panel settings projection read and
  decoded `customStartIcon` synchronously on the UI thread. A thread-local cache keyed by path + file length +
  modification time now reuses the 25×25 RGBA raster, so accent/theme/settings re-projections skip the file read and
  the image decode; a changed file (or path) still re-decodes. Same pixels, same reset-on-failure behavior.

## Dead code deleted

- `IconLoader::low_enqueued` and `IconLoader::dropped_low` were never incremented: no low-priority request path
  exists (`enqueue` only takes the visible/high path). Removed both fields and their initializers, plus the
  `shell.icon.low` / `shell.icon.dropped_low` counters in `async_projection/metrics.rs` and their publication in
  `async_projection/icon_cache.rs::note_loader_stats`. The integration icon service keeps its own unrelated counters.

## Not done here / remaining

- `quick_controls/host/*` still does synchronous `ControlClient` calls (snapshot load, scoped helpers) on the
  *helper* process loop. That stalls only the helper's own surfaces, not the shell loop, and the helper is spawned
  off the shell's thread; moving it would be a separate unit.
- `ApplicationCatalog::discover()` in `app/dock.rs` is a one-time lazy scan on the first dock rebuild, and the
  Trash folder probe runs once per dock lifetime; both are bounded startup costs, not per-turn blocking.
