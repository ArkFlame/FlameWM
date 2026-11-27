# Unit p54-window-core-alloc
STATUS: OPEN

Crates flamewm-window-core and flamewm-ui-document: per-event allocations in hot paths (rg clone/to_string/format! in hover, move, resize paths); borrow or reuse. Output identical. Max 3 edits.

## Result

STARTED
