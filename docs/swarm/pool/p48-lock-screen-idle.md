# Unit p48-lock-screen-idle
STATUS: DONE

Crate flamewm-lock: while locked and idle, CPU must be near zero: audit redraw loops, clock redraw cadence (minute boundary only), cursor blink; fix.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-lock/src/run/tick.rs:103`, `crates/flamewm-lock/src/run/paint.rs:33-42` | The 1 s reactor tick reaches `repaint` every turn, but it returns before any `with_document`/`redraw` unless the `PaintKey` differs; `FramePacer` sleeps on a condvar while Ribbons is off (`run.rs:341,349-353`) and the Wayland loop blocks 1 s when not authenticating (`wayland.rs:606-648,832-844`). No continuous redraw loop exists. | None needed; documented the idle-redraw invariant in `repaint`'s doc comment. **UNBUILT** | Comment only; no visual change and no gate loosened. |
| `crates/flamewm-lock/src/native_time.rs:8-14`, `crates/flamewm-lock/src/run.rs:264-274`, `crates/flamewm-lock/src/wayland.rs:292-300,725-730` | `LocalTime` has no seconds field, so the X11 `PaintKey.time` and the Wayland `shown_minute` both change only at the minute boundary; `view.rs:36-39` also avoids formatting the hidden date. Clock redraw cadence is already minute-only. | None needed; same documenting comment in `run/paint.rs`. **UNBUILT** | Clock text is unchanged; at most one repaint per minute. |
| `crates/flamewm-lock/src/document.rs:562`, `ui/lock/lock.css:67-69` | `lock-caret` is shown only while the password mask is empty, with no blink timer and no CSS animation, so no periodic repaint is driven by a caret. Reference precedent: `.vendor/vela-main/shell/src/main.cpp:300` states carets do not blink; `.vendor/flamewm-c-reference/src/yinputline.h:109` shows a blink timer only where blinking is intended. | None needed; same documenting comment in `run/paint.rs`. **UNBUILT** | None; the caret stays steady and pixel-identical. |

Static audit with `rg`/`Read` only: no `git`, `cargo`, build, test, Python or sleep-loop command was run. The sole production file touched is `crates/flamewm-lock/src/run/paint.rs`, a doc comment; it is marked UNBUILT.

PASS — the three idle-CPU areas are already correct: there is no continuous redraw loop, the clock repaints only on the minute boundary, and the caret is steady instead of blinking. The one edit pins that invariant in a comment and changes no pixels or gates; it is UNBUILT pending the orchestrator's build, tests and visual check.
