# u96-explorer-thumbs log

Area share: in a folder of photos the thumbnail worker is the largest explorer CPU slice; per decode it
is dominated by the whole-file read, the full-resolution decode and the area downscale. The UI thread
only reads the LRU, so the win is spending less of that slice on rows that already scrolled away.

- thumbs.rs `ThumbQueue`: added an `in_flight` slot beside the job deque. `pop` marks the key it hands
  to the decoder; `retain` clears the mark when the new projection no longer shows that row and also
  returns the abandoned key, so `start_pass_keeping` frees its pending mark and the row can be
  requested again when it scrolls back. `wants` is the decoder's cancellation check; `release` clears
  the mark after a decode finishes. Previously only queued jobs were cancelled, so a scroll over large
  photos still paid for the decode already running.
- thumbs.rs `render_thumbnail_while`: the worker now re-checks `wants` after the 64 KiB header probe,
  before reading the rest of the file and before the raster decode/scale. A row that left the view
  costs one prefix read instead of a whole-file read plus a full-resolution decode plus a downscale.
  The public `render_thumbnail` keeps its `Option<Vec<u8>>` contract for the pipeline tests.
- thumbs.rs `ThumbRender`: a cancelled decode is a distinct outcome, never `None`, so it is not cached
  as a permanent failure and the UI keeps the type icon only until the row reappears.
- workers.rs `thumb_worker`: cancelled decodes send no result and no wake byte, so a burst of
  abandoned decodes no longer schedules projections nothing changed.
- thumbs.rs `fit_canvas`: a square source (the common photo) fits to exactly 18x18, so the area-scaled
  buffer is moved into the cache as the canvas instead of allocating a second 18x18 canvas and copying
  it row by row.

Expected composition shift: thumbnail worker time per scroll drops by the share of decodes whose row
leaves the view mid-decode (largest on fast scrolls over big images); the UI stays unchanged. Cache hit,
failure caching, LRU byte cap, header-first rejection, the 40 ms wake coalescing and every visual rule
are preserved.
