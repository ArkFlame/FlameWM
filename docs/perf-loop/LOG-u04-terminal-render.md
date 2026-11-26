# u04-terminal-render log

- screen.rs `take_dirty`: return `Dirty::All` before scanning and allocating the unused per-row list, so wholesale changes cost no row collection.
- screen.rs `resize`: early-return when columns and rows are unchanged, so a repeated same-size ConfigureNotify no longer rebuilds the dirty bitmap and forces a full-grid redraw.
- screen.rs `resize`: batch the shrink with one `drain` and the grow with one `splice` (plus reverse), replacing per-row front `remove`/`insert` shifts.
- screen.rs `scroll_down`: rotate the region in place and fill the wrapped row, removing one row removal, insertion and cell-vector allocation per line (IL, DL, reverse index).
- screen.rs `erase_lines`: fill the retained row buffer instead of allocating a fresh cell vector per erased row.
- screen.rs `switch_alt`: clear and resize the retained alternate lines instead of reallocating the whole alternate grid on every screen switch.
- screen.rs: delete the now-unused `grid_mut` helper.
- main.rs keyboard: request a frame only for key presses, since `App::key` ignores releases, so every key release no longer triggers a projection pass.
- main.rs drag: return `false` for drag traffic, which mutates no app state, removing a useless projection per drag event.
- main.rs startup: compute the backdrop before constructing `App` and move the snapshot and packaged root into it, removing one owned snapshot clone and one duplicate root lookup.
