crates/flamewm-wm-x11/src/output_scale.rs:171:        let plan = plan_scale(150, 1920, 1080).unwrap();
crates/flamewm-wm-x11/src/output_scale.rs:181:        let plan = plan_scale(125, 1920, 1080).unwrap();
crates/flamewm-wm-x11/src/output_scale.rs:189:        let plan = plan_scale(110, 1920, 1080).unwrap();
crates/flamewm-wm-x11/src/output_scale.rs:198:        let plan = plan_scale(200, 1, 1).unwrap();
crates/flamewm-wm-x11/src/runtime/mod.rs:435:            let host = host_guard.as_mut().expect("host initialized");
crates/flamewm-wm-x11/src/decoration/cache.rs:56:        let image = catalog_icon_image(&raster).expect("valid worker raster");
crates/flamewm-wm-x11/src/decoration/policy.rs:237:        let extents = gtk_frame_extents(&[20, 20, 20, 20]).expect("four values");
crates/flamewm-wm-x11/src/window_config.rs:201:        std::fs::create_dir_all(&dir).expect("scratch dir");
crates/flamewm-wm-x11/src/window_config.rs:208:        std::fs::write(dir.join(RULES_FILE), "app = xterm\nworkspace = 2\n").expect("write");
crates/flamewm-wm-x11/src/window_config.rs:212:        std::fs::write(dir.join(RULES_FILE), "app = xterm\nworkspace = 0\n").expect("write");
crates/flamewm-wm-x11/src/window_config.rs:216:        std::fs::remove_file(dir.join(RULES_FILE)).expect("remove");
crates/flamewm-wm-x11/src/blue_light.rs:297:            let delay = next_refresh_delay(scheduled(), now).expect("scheduled");
crates/flamewm-wm-x11/src/blue_light.rs:309:        let (conn, screen) = x11rb::connect(None).expect("DISPLAY");
crates/flamewm-wm-x11/src/blue_light.rs:313:            .expect("screen resources")
crates/flamewm-wm-x11/src/blue_light.rs:316:            .expect("an active crtc");
crates/flamewm-wm-x11/src/blue_light.rs:317:        let size = gamma_size(&conn, crtc).expect("gamma size");
crates/flamewm-wm-x11/src/blue_light.rs:321:                .expect("request")
crates/flamewm-wm-x11/src/blue_light.rs:323:                .expect("reply");
crates/flamewm-wm-x11/src/blue_light.rs:333:        assert!(apply_at(&conn, root, warm, 12 * 60).expect("apply"));
crates/flamewm-wm-x11/src/outline_tune.rs:203:        std::fs::write(&path, "width=2\n").expect("write tune file");
crates/flamewm-wm-x11/src/outline_tune.rs:205:        state.sync(path.to_str().expect("utf-8 temp path"));
crates/flamewm-wm-x11/src/outline_tune.rs:208:        std::fs::write(&path, "width=3\n").expect("rewrite tune file");
crates/flamewm-wm-x11/src/outline_tune.rs:211:        state.sync(path.to_str().expect("utf-8 temp path"));
crates/flamewm-wm-x11/src/outline_tune.rs:215:        state.sync(path.to_str().expect("utf-8 temp path"));
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:357:            usize::try_from(ch).unwrap(),
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:382:        let last = rects.last().expect("client rows");
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:390:        let middle = rects[usize::try_from(ch / 2).unwrap()];
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:407:            usize::try_from(client_h).unwrap(),
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:417:        let last = client[usize::try_from(client_h).unwrap() - 1];
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:420:        assert_eq!(i32::from(last.x), i32::try_from(inset).unwrap());
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:423:            i32::try_from(client_w - inset).unwrap()
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:426:        let middle = client[usize::try_from(client_h / 2).unwrap()];
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:479:        for rect in [rects[0], *rects.last().expect("rows")] {
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:526:        let last = mask.last().expect("client rows");
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:614:        assert_eq!(exact.len(), usize::try_from(h).unwrap());
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:617:            let index = usize::try_from(row).unwrap();
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:642:            let (_, start, end) = band[usize::try_from(u32::from(titlebar)).unwrap() + index];
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:645:                i32::try_from(start).unwrap(),
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:650:                i32::try_from(end).unwrap(),
crates/flamewm-wm-x11/src/frame/geometry_shape.rs:663:        let first = usize::try_from(u32::from(titlebar)).unwrap();

## u25 audit — X11 session start path (file:line, failure, fix)

crates/flamewm-wm-x11/src/wm/event_loop.rs:54 — `x11rb::connect(None)?` aborts with no DISPLAY — REQUIRED (no X server means no X11 session); left as-is.
crates/flamewm-wm-x11/src/wm/event_loop.rs:66 — `become_wm(...)?` aborts when another WM holds the root selection — REQUIRED (ICCCM single-WM ownership); left as-is.
crates/flamewm-wm-x11/src/wm/construct.rs:10-33 — RandR `query_extension` / `randr_select_input` previously aborted on absent extension — ALREADY CONVERTED: `match` on query, `eprintln!` with "blast radius: no live output hotplug/CRTC change notifications, root geometry is used", continues.
crates/flamewm-wm-x11/src/lib.rs:958-970 — `randr_get_screen_resources_current()?` aborted when RandR resources unavailable — ALREADY CONVERTED: match + "blast radius: no per-output geometry/modes, falling back to root geometry", `(0, Vec::new())` fallback.
crates/flamewm-wm-x11/src/lib.rs:973-978 — `randr_get_output_primary()?` aborted on missing reply — ALREADY CONVERTED: `.ok().and_then(...).map_or(0, ...)`; falls back to primary 0.
crates/flamewm-wm-x11/src/wm/mod.rs:818-837 — `composite_query_version` / `_NET_WM_CM_S<screen>` selection-owner lookup could abort when compositor absent — ALREADY CONVERTED: `.ok()` chains, returns `false` (frame-band ring fallback); no extension is mandatory.
crates/flamewm-wm-x11/src/wm/raw_keys.rs:307-312 — `query_extension(b"XInputExtension")?.reply()?` aborted when XInput absent — ALREADY CONVERTED: logs "XInput absent; raw escape keys disabled" and returns `Ok(())`; caller at wm/event_loop.rs:80-82 also logs any remaining `Err` and continues.
crates/flamewm-wm-x11/src/wm/construct.rs:77-93 — `ExternalDecorationRenderer::open("IBM Plex Sans")` (glyph atlas + cursor theme) aborted when font/theme/atlas missing — ALREADY CONVERTED: `.ok()` makes renderer optional, outline-overlay availability probed, cursor define is `let _ =`; frame-band ring fallback.
crates/flamewm-wm-x11/src/wm/event_loop.rs:67-69 — clipboard keeper spawn `Err` — ALREADY CONVERTED: "clipboard keeper unavailable" log, session continues.
crates/flamewm-wm-x11/src/wm/event_loop.rs:93-95 — window-rules watch registration `Err` — ALREADY CONVERTED: "window rules hot reload unavailable" log, session continues.
crates/flamewm-wm-x11/src/runtime/mod.rs:143 — `connect_control_session` — ALREADY OPTIONAL: returns `Option`, missing session bus is retried lazily with capped backoff.
crates/flamewm-wm-x11/src/runtime/mod.rs:324-328 — `register_settings_wake` `Err` — ALREADY CONVERTED: `inspect_err` logs "XSettings wake not registered", `.ok()` drops the wake.
crates/flamewm-wm-x11/src/atoms.rs:143-150 and crates/flamewm-wm-x11/src/lib.rs:139-148 — `intern_atom` / reply `?` on startup — REQUIRED core X atoms; left as-is.
crates/flamewm-wm-x11/src/runtime/mod.rs:145-159,168-175 — `LinuxIntegrationRuntime::connect()?`, `start()` error, five `register(...)?` calls, `AudioHelper::connect(...)?` hard-fail on missing system bus/NetworkManager/Bluetooth/PowerProfiles/PulseAudio — NOT CONVERTED: these are system-service integrations, outside the named optional set (X11 extension / atlas / cursor theme / compositor). Flagged as the strongest remaining "laptop no-start" candidate; converting them needs `Option<ProviderRegistration>` plumbing through runtime/mod.rs:498-521 and is left for a separate verified slice.

Status: UNBUILT

## Step 2 — startup-path audit (narrow retry)

Scope: `crates/flamewm-wm-x11/src`, production paths. Every `expect(`/`unwrap(` hit in the
Step 1 list resolves to a `#[cfg(test)] mod tests` block (blue_light.rs:306,
switcher_thumb_x11.rs:76, geometry_shape.rs:306, outline_tune.rs:148,
window_config.rs:191, output_scale.rs:156, lib.rs:1136+) or to an internal
invariant assert (runtime/mod.rs:435, wm/controllers.rs:54,
wm/chrome_runtime.rs:101). None are on X11 startup. The five startup categories below already degrade gracefully;
there is no remaining hard-fail to convert, so no code file was changed.

| # | category | site | failure record / blast radius | action |
|---|----------|------|-------------------------------|--------|
| 1 | RandR extension query + event selection | wm/construct.rs:10-33 | `query_extension` and `randr_select_input` use `match` / `if let Err`; prints "RandR ... blast radius: no live output hotplug/CRTC change notifications, root geometry is used" | already log-and-continue; no change |
| 2 | RandR screen-resource load | lib.rs:953-970 | `randr_get_screen_resources_current` uses `match`; prints "RandR screen resources unavailable ...; blast radius: no per-output geometry/modes, falling back to root geometry" | already log-and-continue; no change |
| 3 | Composite / XInput extension query | wm/mod.rs:818-837, wm/raw_keys.rs:308 | Composite query uses `.ok()` chains with an "automatic frame redirect unavailable" fallback; XInput `?` is wrapped by the `select_raw_keys` guard in wm/event_loop.rs:80-82 which prints "raw escape-key feed unavailable" | already log-and-continue; no change |
| 4 | atom / cursor / theme load | atoms.rs:78-150, wm/construct.rs:51,77-87 | 61 `InternAtom` replies are propagated as a required invariant; renderer open uses `.ok()` and cursor define uses `let _ =`, so a missing theme never aborts startup | no safe standalone conversion: the atom set is a required fixed struct, not an optional capability |
| 5 | selection owner (`_NET_WM_CM_S<n>`) | wm/mod.rs:828-836 | `get_selection_owner` is an `.ok()` chain; failure degrades to "no automatic frame redirect", frames stay unredirected | already log-and-continue; no change |

UNBUILT — no cargo, build, or test was run in this unit (unit rules forbid it).
No code file was edited in Step 2; `wm/pointer.rs` and `wm/counters.rs` were not touched.

PARTIAL — Step 1 is complete and the Step 2 audit covers all five requested
startup categories, but zero conversions were made: each category is already
log-and-continue with a one-line blast-radius record, and the only remaining
startup `?` calls are required invariants (atom set, root event-mask ownership)
or are already wrapped by a logging guard. Converting those would change
correct behavior, so the safe scope for this retry is empty; changes are
UNBUILT and need orchestrator verification.

## Step 3 — startup-fallback compile fix (UNBUILT)

The Step 1 fallback edits left `flamewm-wm-x11` not compiling: x11rb request
methods return `ConnectionError` while cookie `.reply()`/`.check()` return
`ReplyError`, so `.and_then(|cookie| ...)` could not unify the error types
(wm/construct.rs:10-33, lib.rs:959-976). The `Ok(resources)` match arm in
`display_snapshot` also scoped `resources` too narrowly for the later
`resources.modes` use (lib.rs:1002). Fixed by mapping the request error with
`.map_err(ReplyError::from)` before each `.and_then`, importing `ReplyError`
in lib.rs, and carrying the mode list out of the match as `resource_modes`.
Failure behaviour is unchanged: an absent or failed RandR stage logs its
blast-radius message and falls back to root geometry instead of aborting.

UNBUILT — no cargo, build, or test was run in this unit (unit rules forbid it).
The orchestrator must verify the fix with a build.
