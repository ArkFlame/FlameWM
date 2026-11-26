# u81-clipboard-dnd

Area: clipboard, drag and drop, selection. `crates/flamewm-wm-wayland/src/protocols/clipboard.rs`,
`crates/flamewm-wm-wayland/src/handlers.rs`, `crates/flamewm-clipboard/src/codec.rs`.

Composition examined: a paste of a large selection, a selection change, and a history re-select.
The dominant shares were the per-paste payload copy and UTF-8 validation on the compositor loop,
then the extra full-text copy made when the same selection entered the history. Both are removed by
sharing one allocation between the selection slot and the history.

## Changes (composition-share ranked)

1. **Paste serving: no payload copy on the compositor thread.** `TextSlot` now stores
   `Arc<[u8]>` instead of `Vec<u8>`, and `send_selection` calls the new `TextSlot::bytes`, which
   clones the reference count. Before, every paste did `slot.text(...)` followed by `to_vec()`,
   copying up to `MAX_TEXT_BYTES` (1 MiB) on the event loop before the writer thread ran. Expected
   shift: the largest per-paste loop share (a full-payload memory copy) becomes a refcount bump;
   the socket write stays off the loop in `write_off_loop`.

2. **Lazy mime conversion: cached UTF-8 verdict.** `Kept` caches whether the bytes are valid UTF-8
   when the read completes. `text`/`bytes` matching and `offers` no longer run
   `std::str::from_utf8` over the whole payload on each request. Before, a 1 MiB selection was
   rescanned for every paste and on every re-publish just to decide whether other text targets may
   be served. Expected shift: removes an O(payload) scan per paste from the loop; the single scan
   now happens once, off the loop, at read completion.

3. **Bounded history: shared entries and a byte cap.** `TextHistory` stores `Arc<[u8]>` shared with
   the slot, so keeping an entry and re-selecting it copies a refcount instead of the text. A
   `MAX_HISTORY_BYTES` (8 MiB) cap evicts oldest entries alongside the existing
   `HISTORY_LEN` (20) cap, bounding retained clipboard RAM; previously up to 20 full-size entries
   (20 MiB) could stay resident. Expected shift: lower steady-state clipboard memory, and no
   full-text copy when a selection enters the history or is chosen from it.

4. **`flamewm-clipboard` text publishing: one copy for non-ASCII text.** `codec::text_payloads`
   now builds only the `UTF8_STRING` payload when the text is not ASCII. Before, it always cloned
   the whole text for `UTF8_STRING` and then discarded that clone when `STRING` was not offered.
   Expected shift: halves the encoding allocation for large non-ASCII clipboard text on the owner
   thread.

Kept: password-hint handling, INCR readers, `MAX_TEXT_BYTES` limit, the 500 ms read timeout and the
20-entry history length. Tests in `protocols/clipboard.rs` were updated for the shared-bytes
history and extended with a byte-cap eviction test.

No `cargo` or `git` was run in this unit; the orchestrator runs the build and tests.
