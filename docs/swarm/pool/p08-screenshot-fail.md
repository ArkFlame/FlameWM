# Unit p08-screenshot-fail
STATUS: PARTIAL

Screenshot tool (rg -l screenshot crates): full disk, unwritable directory, unsupported pixel format or failed readback must report once and continue, never panic; audit and fix.

## Result
STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-screenshot/src/capture.rs:615-621 | Short/corrupt XFixes cursor reply on `--cursor`: `blend_cursor` indexed `cursor.cursor_image[sy*width+sx]` with no length check, so a failed readback panicked with an out-of-bounds index. | Checked `width*height`, then return `short XFixes cursor image reply` once; caller reports the error and exits normally. Valid replies take the unchanged pixel path. | UNBUILT (no cargo allowed in this unit); error path writes no pixels, so visuals stay pixel-identical. |
| crates/flamewm-screenshot/src/wayland.rs:75,154-160,312 | `zwlr-screencopy` `Buffer` with an unsupported pixel format was ignored; `receive()` then waited forever for `format.is_some()`, so it never reported and never continued (including v1/v2 and multi-format offers). | Track `buffer_seen` for every `Buffer` event; when no announced format is `Xrgb8888`/`Argb8888`, return `unsupported compositor buffer format` once. Supported-format dispatch is unchanged. | UNBUILT (no cargo allowed in this unit); only the no-valid-format branch is new, so pixel output for valid formats is unchanged. |
| crates/flamewm-screenshot/src/main.rs:263-279,378-408; crates/flamewm-screenshot/src/portal.rs:132-145; crates/flamewm-screenshot/src/capture.rs:219-293 | Audit of the other named cases: full disk and unwritable directories already fail through `new_file`/`default_output` with `map_err` plus partial-file cleanup; unsupported X11 depth and short `GetImage`/SHM replies already return `Err`. | No change needed; portal `answer` already logs once and continues serving. | UNBUILT (static audit only); no behavior changed. |

PARTIAL — audit complete and both reachable defects fixed (cursor readback bounds check and Wayland unsupported-format bailout), but every edit is UNBUILT because this unit forbids cargo; full disk, unwritable directory, X11 pixel format and X11 readback paths were verified by inspection only and already reported errors without panicking.
