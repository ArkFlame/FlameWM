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

## Step 2 — startup-path audit (narrow retry)

Scope: `crates/flamewm-wm-x11/src`, production paths. Every `expect(`/`unwrap(` hit in the
Step 1 list resolves to a `#[cfg(test)] mod tests` block (blue_light.rs:306,
switcher_thumb_x11.rs:76, geometry_shape.rs:306, outline_tune.rs:148,
window_config.rs:191, output_scale.rs:156, lib.rs:1136+) or to an internal
invariant assert (wm/controllers.rs:54, wm/chrome_runtime.rs:101). None are on
X11 startup. The five startup categories below already degrade gracefully;
there is no remaining hard-fail to convert, so no code file was changed.

| # | category | site | failure record / blast radius | action |
|---|----------|------|-------------------------------|--------|
| 1 | RandR extension query + event selection | wm/construct.rs:10-33 | `query_extension` and `randr_select_input` use `match` / `if let Err`; prints "RandR ... blast radius: no live output hotplug/CRTC change notifications, root geometry is used" | already log-and-continue; no change |
| 2 | RandR screen-resource load | lib.rs:953-970 | `randr_get_screen_resources_current` uses `match`; prints "RandR screen resources unavailable ...; blast radius: no per-output geometry/modes, falling back to root geometry" | already log-and-continue; no change |
| 3 | Composite / XInput extension query | wm/mod.rs:818-837, wm/raw_keys.rs:308 | Composite query uses `.ok()` chains with an "automatic frame redirect unavailable" fallback; XInput `?` is wrapped by the `select_raw_keys` guard in wm/event_loop.rs:80-82 which prints "raw escape-key feed unavailable" | already log-and-continue; no change |
| 4 | atom / cursor / theme load | atoms.rs:78-150, wm/construct.rs:51,77-87 | 61 `InternAtom` replies are propagated as a required invariant; renderer open uses `.ok()` and cursor define uses `let _ =`, so a missing theme never aborts startup | no safe standalone conversion: the atom set is a required fixed struct, not an optional capability |
| 5 | selection owner (`_NET_WM_CM_S<n>`) | wm/mod.rs:828-836 | `get_selection_owner` is an `.ok()` chain; failure degrades to "no automatic frame redirect", frames stay unredirected | already log-and-continue; no change |

UNBUILT — no cargo, build, or test was run in this unit (unit rules forbid it).
No code file was edited; `wm/pointer.rs` and `wm/counters.rs` were not touched.

PARTIAL — Step 1 is complete and the Step 2 audit covers all five requested
startup categories, but zero conversions were made: each category is already
log-and-continue with a one-line blast-radius record, and the only remaining
startup `?` calls are required invariants (atom set, root event-mask ownership)
or are already wrapped by a logging guard. Converting those would change
correct behavior, so the safe scope for this retry is empty; changes are
UNBUILT and need orchestrator verification.
