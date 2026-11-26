# Log u60-lock-ribbons-tray

Area: `flamewm-{lock,ribbons,tray,clipboard}` — wakeups, polling, memory caps,
startup cost, per-message and per-frame work.

Reviewed the recent perf slices touching this area by reading the current code
and the recorded logs (u14 finder/tray, u17 settings/lock, u40 misc apps,
u41 desktop dead code): the lock loop is still event driven with the frame
pacer parked on a condvar outside Ribbons, the tray watcher owns no thread or
timer, and the clipboard keeper blocks on the X socket instead of polling.
No behavioural regression was found. One latent cap bug was fixed and six
composition-level costs were removed.

## Bug / cap fix

- `tray/watcher.rs` (`WatcherState::request`, `request_layout`): a pending
  request was keyed only by D-Bus serial, so an item that re-announces itself
  (`NewIcon`, `NewToolTip`, ...) or never answers a `GetAll`/`GetLayout` could
  accumulate one slot per request. With `MAX_PENDING = 256`, enough chatty
  items could fill the budget permanently and silently stop property refresh
  for *every* tray item (including healthy ones). A new request now drops the
  superseded pending entry for the same (kind, item) before it is recorded, so
  the budget is bounded by `3 * MAX_ITEMS` and one item cannot starve others.
  A stale serial's late reply is simply ignored, exactly as before.

## Optimizations

- `tray/watcher.rs` (`prop_value`, `pixmaps`): `IconPixmap` was decoded by
  cloning the whole decoded `MessageItemArray` into a `Vec<MessageItem>`, then
  mapping that into a second `Vec<u8>`. The array is now read in place, the
  declared `width * height * 4` size is validated against the element count and
  `MAX_PIXMAP_BYTES` *before* the bytes are copied, and the image bytes are
  filled into one exactly-sized buffer. Expected shift: one full array clone
  plus one intermediate allocation removed per icon update, and a hostile
  oversized reply can no longer be materialised first.
- `tray/item.rs` (`ItemProps::from_owned_props`) + `tray/watcher.rs`
  (`method_return` for `Pending::Props`): the `GetAll` reply was projected by
  building a `BTreeMap<String, PropValue>` and then cloning the kept values out
  of it — a second copy of every icon pixmap buffer (up to 2 MiB per item at
  the documented cap). The new owning projection moves the strings and pixmap
  vectors out of the map (`remove`), so each reply keeps one copy of the icon
  data. `from_props` stays as the borrowing convenience form used by tests and
  external callers.
- `tray/watcher.rs` (`props_from_entries` + `projected`): only the eight
  properties `ItemProps` projects are decoded now. Unused title/icon/menu
  payloads no longer allocate strings in a map that is immediately dropped.
- `tray/watcher.rs` (`method_call`, `signal`, `register_item`): interface,
  member, sender and path were converted to owned `String`s just to compare
  them against constants. The dbus wrapper types deref to `str`, so the
  comparisons and `keys_for_sender` now borrow from the message. Expected
  shift: no per-message allocation on the tray's bus path (every item signal
  and property reply previously allocated two to four short strings).
- `clipboard/lib.rs` (`text_from_bytes`): `get_text`/`get_primary_text` always
  paid a losing-`Cow` copy (`from_utf8_lossy(..).into_owned()`) even though
  clipboard text is valid UTF-8 virtually always. The buffer now moves into the
  `String` (`String::from_utf8`) and only invalid bytes take the lossy path —
  identical output, one fewer full-payload copy per paste/keeper fetch.
- `clipboard/codec.rs` (`file_uri`, `push_file_uri`, `uri_list`,
  `gnome_files`): each path built a temporary `String` via the `fmt` machinery
  (`write!` per escaped byte) and copied it into the payload; `uri_list`
  additionally collected into a nested `String`. One buffer is now appended to
  directly with a fixed uppercase hex table, so encoding N paths does N
  appends and no per-path temporary. Output bytes are unchanged
  (`gnome_files`/`uri_list` tests still assert the exact payloads).
- `lock/document.rs` (`decoded_background` + `builtin_background`): the
  built-in WebP wallpaper was decoded again for every output and every time the
  lock re-showed its background after DPMS or a suspend. The decoded pixels are
  now cached once per process in a `OnceLock` and each output only pays its own
  cover-scale/crop. Startup and resume cost drop from one full decode per
  surface to one per session, with identical pixels.

Considered and skipped: `RibbonFrames::frame` zeroes a fresh full-surface RGBA
buffer per animation frame (30 fps), but `RuntimeImage` is moved into the
document layer (outside this unit's owned paths) and `RgbaImage`'s borrow API
cannot reclaim it, so a reusable buffer would need an API change to
`flamewm-ui-document`; the rasterizer already writes every output pixel, so
only the allocation, not the fill, could be saved. The static `splat`/`taps`
loops keep their reused scratch buffers.

## Dead code

Static scanning (`pub fn` names in the owned paths, then reference count across
the workspace) found no unreachable function or unused helper in these crates;
the only removed code is the now-unused intermediate decode in `pixmaps` and the
`std::fmt::Write` import in `codec.rs`.

## Files touched

- crates/flamewm-tray/src/watcher.rs
- crates/flamewm-tray/src/item.rs
- crates/flamewm-clipboard/src/lib.rs
- crates/flamewm-clipboard/src/codec.rs
- crates/flamewm-lock/src/document.rs

UNVERIFIED: no `cargo`/`git` was permitted for this unit, so this is reviewed
by reading and formatted with `rustfmt --check` only. Compilation and runtime
behaviour were not executed. Public types, payload bytes, node ids, colour and
pixel output and error texts are unchanged.
