# u07-shell-icons-dock
- dock: cache the 24 slot/icon/dot document node ids once; `project_dock` no longer heap-allocates 72 `format!` strings per redraw.
- dock: `running_apps` dedupes app ids with a `HashSet<&str>` instead of O(n^2) string comparison, keeping window order.
- main: `poll_icon` compares the wanted icon name by borrowed `&str` instead of cloning it on every wake while a load is in flight.
- main: `project_icon` spawns the `IconLoader` lazily and stops cloning `icon_root` on every toast that needs a named icon.
- main: the tick closure takes one `RefCell` borrow of the loop state per turn instead of five separate `borrow_mut` calls.
