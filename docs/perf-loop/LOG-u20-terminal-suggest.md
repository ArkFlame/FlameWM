# u20-terminal-suggest log

Second pass over terminal suggest/settings/session. u05 already batched PTY reads and removed the early history
allocation chain; the remaining cost is per-frame suggestion work, per-store-line decoding, and per-keystroke copies
in the settings overlay.

- suggest.rs `LineInput`: keep the typed line in one `String` with a byte cursor (kept on character boundaries)
  instead of a `Vec<char>`. A paste is now one `insert_str` splice instead of a char-by-char collection plus a
  second collected copy, arrow/backspace keys move by character boundary without shifting 4-byte units, and the
  line costs one byte per byte instead of four. `text()` used to re-encode every char into a fresh `String`; it now
  copies the buffer straight, and the grid's ghost read (`ghost()` runs in the paint path) is one memcpy instead of
  a char decode loop.
- suggest.rs `Trie`: drop the `ids: HashMap<String, usize>` side index, which stored a second copy of every command
  string (about 8k entries, roughly 0.5-1 MiB per terminal window at the cap) and cost one extra allocation per
  command at first load. Each node now records `exact: Option<usize>`, so an exact use lookup is the same
  O(prefix length) walk as the suggestion lookup, with no duplicated text and no hash map to build, probe or drop.
- suggest.rs `unescape` returns `Cow<str>`: a store line without an escape sequence is scanned in place. Seeding,
  compaction, purging and the once-a-second cross-window sync no longer allocate one string per line.
- suggest.rs `secret_bearing`: only build the lowercase copy when the command actually has an uppercase byte. The
  filter runs on every store line at load and on every recorded command, and shell commands are overwhelmingly
  already lowercase, so the allocation is skipped in the common case.
- suggest.rs `purge_filtered`: scan the lossy decode as a borrow instead of `into_owned()`, which copied the whole
  (up to 1 MiB) store on every window and tab open.
- suggest.rs `History::sync`: reuse an owned `read_buf` for the tail read, so the once-a-second refresh on typing
  allocates nothing.
- suggest.rs `History::record`: `append` returns the store size from the already-open file descriptor, replacing a
  separate path stat; the path is borrowed for the append instead of cloned into a `PathBuf` per Enter.
- settings.rs `render`: the row builder trims the `String` it just built instead of `clip()` allocating a second
  copy of every row (about 20 rows per frame while the picker is open).
- settings.rs opacity tab: the 100..5 step list is computed (`opacity_at`) instead of allocating a `Vec<u8>` per
  rendered frame and per key press; `opacity_options()` keeps its public shape for callers.
- settings.rs `handle_key`: take `Mode` with `mem::replace` instead of cloning it, and borrow the selected theme
  from the slice instead of cloning the whole `Theme` (name + palette) on every key. Theme names are cloned only on
  the key that actually renames, copies, applies or deletes.

Expected composition shift: terminal typing no longer pays a per-frame line re-encode for the ghost, per key for the
settings overlay, or per store line at load; steady-state RSS per terminal window drops by the removed duplicate
command index. No visual or behavioral change: same suggestion ranking, same filters, same picker modes and keys.

Session: `read_loop`'s batch `Vec` is moved into each `SessionEvent::Output`, so its allocation cannot be pooled
without changing the event type (owned outside this unit); the FIONREAD drain and pacer from u05 remain correct,
so session.rs is unchanged here.
