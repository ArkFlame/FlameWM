# Unit p54-window-core-alloc
STATUS: PARTIAL

Crates flamewm-window-core and flamewm-ui-document: per-event allocations in hot paths (rg clone/to_string/format! in hover, move, resize paths); borrow or reuse. Output identical. Max 3 edits.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-window-core/src/geometry_memory.rs:59-65 | `remember` re-remembered an existing app by removing the entry and pushing a fresh `app_id.to_owned()` String on every geometry change. | Reuse the removed entry's stored `String` (equal by value to `app_id`); keep remove+push so eviction order and serialized bytes are unchanged. | UNBUILT — no static risk found; identical id value, ordering, and output. |
| crates/flamewm-window-core/src/geometry_memory.rs:75-87 | `serialize` allocated one `String` per entry through `format!` + `collect()` on the debounced persist path. | `write!` the unchanged format literal into one reused `String` with a function-local `std::fmt::Write` import. | UNBUILT — byte-identical format string and newline; no visual change. |

PARTIAL — `rg` over both crates found no per-event `clone`/`to_string`/`format!` allocations in the hover/move/resize paths themselves: `focus.rs` and `template.rs::text_ref` already use borrow APIs, `match_rule`/`keep_visible`/`snap_*`/`edge_resize`/`keyboard_move_resize` are allocation-free, and the remaining hits are error strings, tests, and debounced persistence. The two safe allocation removals above are applied but UNBUILT (builds/tests forbidden for this worker), so no measured performance claim is made; the named per-event allocations live in callers outside these two crates.
