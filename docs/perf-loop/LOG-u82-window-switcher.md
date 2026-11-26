# u82-window-switcher
Area: Alt+Tab switcher overlay (`crates/flamewm-shell/src/switcher.rs`, `runtime/state.rs`).
Composition estimate before: while the card is open, the largest share of the overlay path was
memory traffic, not paint: every snapshot deep-cloned each preview buffer up to three times
(signal entries -> thumb cache -> controller frame -> `RuntimeImage`), and every repaint re-hashed
the full pixel buffer to build the image source key. With eight rows at roughly 320x180 RGBA that
is ~1.8 MB of copying per Alt+Tab keypress and a full-buffer hash per repaint; the raster/blit
share is small because the card is 8 short rows.

Changes:
- `SwitcherThumbCache` now stores `SwitcherPreview { width, height, generation, Arc<Vec<u8>> }`
  and `resolve` returns one preview per entry, taking fresh pixels with `mem::take` into an `Arc`.
  A reference snapshot (`rgba` empty, generation set) resolves to an `Arc::clone`, so the pixel
  buffer is never copied on an unchanged preview. The generation cache is still rebuilt per
  snapshot, so a closed snapshot keeps no preview memory.
- `SwitcherFrame` carries `previews` aligned with `entries`; entries themselves stay
  metadata-only. Repaint hands the buffer to the document with `image_rgba8_shared`, which stores
  the caller's `Arc`; `replace_image_for_node_shared` short-circuits on `Arc::ptr_eq`, so a repaint
  with unchanged previews performs no copy, no upload, and no node touch.
- Image source key comes from the WM generation (`switcher-thumb-{w}x{h}-g{gen}`) instead of a
  `DefaultHasher` pass over the pixels. Generation 0 (legacy WMs, untracked pixels) keeps the hash
  path; it is no longer cached, matching `SwitcherThumb`'s contract.
- Render only on change: `SwitcherController::apply` compares the new snapshot with the held one
  via `SwitcherFrame::same_content` (labels, selection, preview generation and size, pointer
  identity for legacy pixels) and marks the card dirty only when something visible moved. Idle
  repeated snapshots no longer repaint, and the retained document is untouched.
- No background capture when closed: the shell never captures; it only renders WM snapshots
  (`take_switcher_frame` returns nothing and `tick_switcher` returns before any output lookup when
  the card is closed and no deadline is pending). Thumbnail capture is owned by the WM
  (`flamewm-render-x11`, outside this unit's scoped paths) and is not changed here.

Expected composition shift: overlay-path share of copies/hashes drops to zero for reference
snapshots and one buffer copy for a changed preview; allocation share per Alt+Tab step falls from
O(rows x bytes) to O(rows) `Arc` clones. Visuals and behaviour (hold delay, MRU window, dimming,
scroll window, minimised/other-workspace labels) are unchanged; tests cover reference reuse via
`Arc::ptr_eq`, pruning, legacy pixels and the no-repaint guard.

Out of scope (not owned here): WM-side capture timer that produces the previews.
