# Unit p10-wayland-no-xwayland
STATUS: DONE

crates/flamewm-wm-wayland: when the Xwayland binary is missing or crashes, skip X11 app support with one failure record and keep the session; check restart-loop bounds; fix gaps.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-wm-wayland/src/xwayland.rs:346-352` | `XWaylandEvent::Error` and `X11Wm::start_wm` errors only printed a message; `autostart` stayed pending with `xwm` absent, so `desktop_pending()` stayed true and the content deadline aborted the whole VT session instead of keeping it. | Both paths now call `State::xwayland_unavailable`, which emits one `stage=xwayland status=fail` line, clears `autostart`, and keeps the Wayland session. Missing-binary spawn failure already had this treatment in `main.rs`; this closes the crash/startup-error gap. | UNBUILT - brief forbids cargo/builds; verified visibility by inspection (`stage_line`, `Stage`, `StageStatus` are `pub(crate)`). |
| `crates/flamewm-wm-wayland/src/xwayland.rs:358` (new method) | No shared skip-and-record path existed for X11 unavailability after spawn. | Added `State::xwayland_unavailable(cause: impl Display)`: one structured failure record, `continuing,no_x11_client,autostart_skipped` detail, explicit autostart drop that disarms the deadline. | UNBUILT - compile/format not run per brief. |
| `crates/flamewm-wm-wayland/src/main.rs:1034` | `xwayland_unavailable_detail` was private to `main.rs`, so the event-loop path could not reuse the exact wrapper-facing detail tokens. | Made it `pub(crate)` so both failure paths emit one identical, test-covered detail format; no gate or token loosened. | UNBUILT - brief forbids builds. |

Restart-loop bounds checked, no edit: smithay's X11 display-slot search is bounded to `0..33` (`.vendor/smithay-master/src/xwayland/x11_sockets.rs:26`), and the XWayland calloop source disables itself after Ready/Error (`.vendor/smithay-master/src/xwayland/xserver.rs:361-399`), so no respawn loop was added. Visuals untouched.

PASS - X11 unavailability from a missing binary, a startup crash (`XWaylandEvent::Error`), or a WM start failure now skips X11 app support with exactly one structured failure record and keeps the session; the existing spawn-failure path, deadline logic and wrapper detail tokens are preserved, and restart-loop bounds were confirmed bounded. All three edits are marked UNBUILT because the brief forbids cargo, builds and tests; compilation remains for the orchestrator's batch build.
