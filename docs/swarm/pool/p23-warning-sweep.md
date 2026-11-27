# Unit p23-warning-sweep
STATUS: DONE

## Result
STARTED

Dead code sweep: run rg on crates (no cargo) for items the last check flagged never used (see /tmp/claude-1000/check5.txt warnings for crates/flamewm-*, ignore third_party); delete truly dead items or gate them behind cfg(test), max 6 edits.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-shell/src/projection/panel.rs:141 | Zero-arg `start_header_ink()` flagged never used; repo-wide rg found only its definition (the other `start_header_ink(accent, light)` is a different fn in settings_projection/palette.rs) | Deleted the dead getter and its doc comment; `START_HEADER_ARGB` and its writer `set_start_header_ink` untouched | UNBUILT — a concurrent worker could add a caller; visuals unchanged because no render path reads the getter |
| crates/flamewm-shell/src/projection.rs:77 | `pub(crate) use panel::{set_start_header_ink, start_header_ink};` re-exported the dead getter | Narrowed to `pub(crate) use panel::set_start_header_ink;` | UNBUILT — removes a `pub(crate)` name; final rg confirms no remaining consumers |
| crates/flamewm-wm-wayland/src/render/mod.rs:844 | `let (mut elements, cache_key)` — outer binding is only read; the separate inner `mut elements` at line 886 feeds `build_elements_into` | Dropped the unnecessary outer `mut` | UNBUILT — compile-only cleanup, no logic or pixel change |

Stale warnings left untouched after re-read: `temp_name`/`is_own_temp`/`cleanup_own_temps` are now used by host.rs and cfg(test); `quarantine_unparsable`/`quarantine_and_report` are absent; `builtin_themes` is now imported under cfg(test); `std::fmt::Write` in projection/start.rs is used by `write!` at line 97. No `.vendor` precedent applies (plain Rust dead-accessor cleanup). No cargo/build/test run, per brief.

PASS — three production edits (within the 3-edit cap) removed the only live dead item plus its unused re-export and one needless `mut`; rg verification confirms no callers and no visual-path change, every edit is UNBUILT and awaits the orchestrator's build.
