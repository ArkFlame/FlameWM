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
