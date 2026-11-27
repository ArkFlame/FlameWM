# Wayland startup hard-fail audit (u26)

Status: UNBUILT (no cargo/build/tests run per brief)

## Step 1 - hard-fail candidate list (exact rg output)

```text
crates/flamewm-wm-wayland/src/drm/input.rs:227:        std::fs::create_dir_all(&dir).unwrap();
crates/flamewm-wm-wayland/src/drm/input.rs:229:        std::fs::write(&path, "pointerSpeed=-50\npointerRawInput=true\n").unwrap();
crates/flamewm-wm-wayland/src/drm/input.rs:235:        std::fs::write(&invalid, "pointerSpeed=101\n").unwrap();
crates/flamewm-wm-wayland/src/probe.rs:542:        let ok = output_bounded("true", &[], Duration::from_secs(5)).expect("true runs");
crates/flamewm-wm-wayland/src/probe.rs:562:            .expect("libseat is still reported");
crates/flamewm-wm-wayland/src/xwayland.rs:775:            .expect("XWayland handlers fire only while the X11 window manager runs")
crates/flamewm-wm-wayland/src/xwayland.rs:1047:            model.get(1).unwrap().rect.y + model.get(1).unwrap().rect.h,
crates/flamewm-wm-wayland/src/xwayland.rs:1059:        let unchanged = model.get(1).unwrap().clone();
crates/flamewm-wm-wayland/src/xwayland.rs:1065:        let snapped = model.get(1).unwrap();
crates/flamewm-wm-wayland/src/xwayland.rs:1077:        let snapped = model.get(1).unwrap();
crates/flamewm-wm-wayland/src/xwayland.rs:1097:        let placed = model.get(1).unwrap();
crates/flamewm-wm-wayland/src/session_lock.rs:141:            .expect("session lock global is created at startup")
```
