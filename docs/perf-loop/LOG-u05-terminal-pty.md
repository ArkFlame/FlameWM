# u05-terminal-pty log

- session.rs `read_loop`: drain the kernel with FIONREAD after each blocking read and merge it into one `Output` event of up to 32 KiB, so a flood (`cat`, build logs) costs one allocation, one channel message, one parser span and one banner check per batch instead of one per few-KiB read.
- suggest.rs `escape`/`escape_into`: replace the four-pass `replace` chain with one pass appending into a caller buffer, cutting four string allocations per history seed line and per recorded command.
- suggest.rs `compact_lines`/`shell_history_lines`: write counts and escaped commands into the reused buffer instead of `format!`, removing one allocation per store line at startup and compaction.
- suggest.rs `LineInput::insert_str`: splice the whole insertion once instead of `Vec::insert` per character, so a large paste is linear instead of quadratic.
- suggest.rs `History::sync`/`head_matches`: borrow the store path instead of cloning the PathBuf on every one-second refresh, and compare the 64-byte head in a stack array instead of a per-sync heap vector.
- settings.rs `mono_fonts`: cache the `fc-list` result in a `OnceLock`, so reopening the font picker no longer forks a subprocess and waits on it on the UI thread.
