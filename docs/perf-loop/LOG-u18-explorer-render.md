# u18-explorer-render

Second-pass scout of Explorer rendering. Earlier units (u01/u02) removed the per-frame template ids,
row text formatting, HOME/statvfs syscalls and menu-offer probing. The remaining hot path is the row
loop of each full projection (every input) plus the glide frames (60/s while scrolling), the status
strip text, and two modal pickers that re-ran filesystem work on every frame.

- Rows: `RowLabels` now also caches the icon family (`view::file_icon` lowercased the extension per
  row per frame) and the `ThumbKey` (`ThumbKey::of` cloned the path and normalised the extension per
  image row per full projection). `project_row_icon`/`row_thumbnail` take the cached key and icon, so a
  full projection no longer allocates per visible row for either. Remote paths never build a key.
- Glide frames: `project_rows(full == false)` no longer re-walks `tab.visible()` up to the scroll
  origin (O(scroll offset) per frame on large folders) nor rebuilds the row window. It moves only the
  slots the last full pass filled (`ROW_OCCUPANCY`) and toggles their visibility. Content, icon and
  background were already skipped on glide frames, so this is the same picture with the scan removed.
- Date: `chrono::Local::now().date_naive()` (time-zone resolution) ran on every projection including
  glide frames; it is now cached for one second and only computed on a full pass.
- Breadcrumbs: rebuilt the component list plus one String and one PathBuf per crumb on every
  projection; cached by current path and handed to `text_ref` (which skips unchanged text).
- Address bar: `display_location` formatted a new string per frame; cached by path. The search field
  placeholder and the two menu toggles ("Show Hidden Files/Type Column") now pass borrowed/static text
  instead of a fresh `format!` each frame.
- Tabs: each tab's displayed name re-allocated per frame while the strip is shown; titles are cached
  by tab path. Tab slots beyond the tab count are still explicitly hidden.
- Status strip: item count, free-space label and the four progress labels were formatted every frame.
  They are cached against their inputs (counts, free bytes, the whole `progress::Snapshot`) and set via
  `text_ref`, so an idle or unchanged strip allocates nothing. The status message itself is borrowed
  from state instead of cloned. While dragging, the transient drop label is still built as before.
- Thumbnail pass: a full pass pruned and re-queued every waiting key on each wake even when it showed
  the same window; the pass now records the listing revision and first visible row and only restarts
  the queue when that window changed (search results still restart it, since the walk re-orders them).
- Sidebar: `fill_sidebar_rows` reuses one `SIDEBAR_ROWS` buffer instead of building a vector plus one
  index vector per group on every projection.
- Open With dialog: the 18 per-frame `format!("openwith-…-{slot}")` ids moved into `IdTables`.
- Convert picker: `offered_formats` walks `PATH` (stat per directory) and probes codecs; the picker
  called it on every projection. The offer and its uppercased labels are now built once per source file
  and reused, and the picker ids are formatted once. Action handling still recomputes the offer, since
  correctness of the chosen format matters more than the single event it costs.

Expected composition shift: full projections allocate roughly 3 allocated values per visible row
fewer and skip ~12 allocations of fixed chrome; glide frames drop an O(origin) filtered scan and the
time-zone lookup per frame. Visuals, labels and offers are unchanged.
