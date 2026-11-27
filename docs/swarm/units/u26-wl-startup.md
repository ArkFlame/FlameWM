# Wayland startup hard-fail audit (u26)

Status: UNBUILT (no cargo/build/tests run per brief)

## Step 1 - hard-fail candidate list (verbatim command output)

```text
crates/flamewm-wm-wayland/src/xwayland.rs:775:            .expect("XWayland handlers fire only while the X11 window manager runs")
crates/flamewm-wm-wayland/src/xwayland.rs:1047:            model.get(1).unwrap().rect.y + model.get(1).unwrap().rect.h,
crates/flamewm-wm-wayland/src/xwayland.rs:1059:        let unchanged = model.get(1).unwrap().clone();
crates/flamewm-wm-wayland/src/xwayland.rs:1065:        let snapped = model.get(1).unwrap();
crates/flamewm-wm-wayland/src/xwayland.rs:1077:        let snapped = model.get(1).unwrap();
crates/flamewm-wm-wayland/src/xwayland.rs:1097:        let placed = model.get(1).unwrap();
crates/flamewm-wm-wayland/src/probe.rs:542:        let ok = output_bounded("true", &[], Duration::from_secs(5)).expect("true runs");
crates/flamewm-wm-wayland/src/probe.rs:562:            .expect("libseat is still reported");
crates/flamewm-wm-wayland/src/drm/input.rs:227:        std::fs::create_dir_all(&dir).unwrap();
crates/flamewm-wm-wayland/src/drm/input.rs:229:        std::fs::write(&path, "pointerSpeed=-50\npointerRawInput=true\n").unwrap();
crates/flamewm-wm-wayland/src/drm/input.rs:235:        std::fs::write(&invalid, "pointerSpeed=101\n").unwrap();
crates/flamewm-wm-wayland/src/session_lock.rs:141:            .expect("session lock global is created at startup")
```

## Step 2 - classification

Runtime items from the list (not inside `#[cfg(test)]`): `session_lock.rs:141`, `xwayland.rs:775`.
All other listed lines live in test modules (`drm/input.rs` pointer-settings test, `probe.rs` tests,
`xwayland.rs` stacking tests) and are not on the startup path.

Adjacent startup-path hard-fail outside the keyword filter, included because the brief names the
builtin cursor explicitly: `cursor.rs:162`.

## Step 2 - change rows (one row appended per change)

| # | Site | Conversion | Failure record (blast radius) |
| --- | --- | --- | --- |
| 1 | `crates/flamewm-wm-wayland/src/cursor.rs:162` | `expect("checked above")` -> `if let Some(named) = ...and_then(Option::as_ref)`; on `None`, log and fall through to the builtin arrow (`&self.fallback`) | A missing/undecodable theme icon degrades to the builtin arrow: pointer visuals only, no input, focus, layout or WM authority impact. |

## Step 2 - audited, not converted (deferred, no row counted)

- `crates/flamewm-wm-wayland/src/session_lock.rs:141` - `SessionLockHandler::lock_state` must return
  `&mut SessionLockManagerState`; Smithay's type has private fields and no `Default`, and
  `State::session_lock` is initialized in `main.rs` (`init_session_lock`), which this brief forbids
  editing. Blast radius if the invariant broke: any lock client request panics the compositor.
  Needs a wider fallback-state refactor, not a narrow retry.
- `crates/flamewm-wm-wayland/src/xwayland.rs:775` - `XwmHandler::xwm_state` must return `&mut X11Wm`;
  `disconnected()` sets `self.xwm = None` and no replacement handle is constructible. "Continue
  without XWayland" requires gating handler delegation, outside the allowed paths. Blast radius if
  the invariant broke: a late X11 event panics the compositor. Deferred.
- "Next DRM node" has no `expect`/`unwrap`/`exit`/`panic` site in this crate's source; DRM node
  selection in `main.rs` is out of scope for this worker.

## Verification

- `rg -n 'expect\(|unwrap\(\)|process::exit|panic!' crates/flamewm-wm-wayland/src ...` - Step 1 list above.
- No build, test, clippy or runtime check run (explicitly forbidden by the brief). UNBUILT.

PARTIAL: The audit list is written verbatim and one genuine startup-path hard-fail (`cursor.rs:162`,
the builtin-cursor fallback named in the brief) was converted to log-and-degrade with its blast
radius recorded. The two other runtime items in the list are Smithay handler methods whose trait
signatures require `&mut` returns with no constructible fallback; converting them safely would
need a `State`/initialization refactor that touches forbidden `main.rs`, so they are recorded as
deferred rather than changed. No DRM-node hard-fail exists in this crate. The result is UNBUILT
per the brief.
