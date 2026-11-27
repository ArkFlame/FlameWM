# Unit p69-wayland-rss-composition
STATUS: DONE

Crate flamewm-wm-wayland (124 MB RSS idle, hotlist B4): read-only audit of retained memory: surface/texture caches, glyph atlas, damage buffers, per-output buffers; list every Vec/HashMap that grows and its bound; add a cap or shrink_to_fit on idle where unbounded. Output identical. Max 3 edits.

## Result
Audit rows (file:line, finding, change, risk):
(font.rs:20,33,125, glyph atlas HashMap<(char,u32),Arc<Glyph>> grows per distinct title character and pixel size; bound CACHE_LIMIT=1024 with clear on overflow, no change, risk none: already capped)
(chrome_buttons.rs:175-184, static glyph CACHE HashMap<(role,edge,rgb),Option<Vec<u8>>> grew on every distinct ink colour from settings with no bound, added CACHE_LIMIT=128 and cache.clear() before entry insert, UNBUILT, risk only extra rasterisation after eviction; same inputs reproduce identical pixels)
(render/titlebar.rs:8,30,539-541, parts and rings HashMaps keyed by WindowId grow per window; bound by per-frame retain against the live model, no change, risk none)
(render/titlebar.rs:18,511-512, surface backdrop HashMap<(rect),MemoryRenderBuffer> grows per override-redirect rect; bound 32 with clear on overflow, no change, risk none)
(render/titlebar.rs:20,541, docks HashMap<BackdropSite,DockBacking> grows per dock site; bound by live-site retain in dock_backdrop.rs:533 and drop_docks clear, no change, risk none)
(render/titlebar.rs:15,22, scratch and dock_scratch textures hold one output-sized buffer each; bound by replacement when dimensions change, no change, risk none)
(render/backdrop.rs:86-99,606, DesktopBackdrop commits/pixels/bgra Vecs grow with output dimensions; bound by stored_size at DESKTOP_SCALE=4 and one Option slot, no change, risk none)
(render/mod.rs:84-97,843,1029-1038, winit SceneCache.elements Vec<Elem> retains peak scene capacity; bound by one thread-local slot dropped on lock or blank, no change, risk none)
(drm/mod.rs:200-205,234, tty SceneCache.elements Vec<Elem> retains peak scene capacity; bound by one slot dropped on session pause, no change, risk none)
(state/mod.rs:163, OutputDamageTracker damage state is smithay-owned per output; bound by output count, per-frame cloned damage Vec dropped after submit (render/mod.rs:1015-1023), no change, risk none)
(state/mod.rs:160,182-193, cached_work_areas/pointer_layout/layout_outputs/output_handles/output_scales Vecs grow per output; bound by detected output count, no change, risk none)
(state/mod.rs:271,172, titles and foreign_toplevel_handles HashMaps grow per window; bound by removal on unmap at xwayland.rs:759 and state/mod.rs:508, no change, risk none)
(state/mod.rs:132,143,234,281,269,273, screencopy_pending and pending_dmabufs are drained by take at render/mod.rs:1096 and dmabuf settle; swallowed_keys removed on key release, children reaped at state/mod.rs:439, x11 and unmanaged pruned on destroy at xwayland.rs:746, no change, risk none)
(protocols/clipboard.rs:71-109, TextHistory VecDeque grows per pasted item; bound by HISTORY_LEN and MAX_HISTORY_BYTES, no change, risk none)
(control/switcher.rs:22-49, ThumbCache slots VecDeque and announced Vec grow per preview; bound by THUMB_CAP with pop_front, no change, risk none)
(cursor.rs:61-64, themes Vec holds one or two xcursor themes and named HashMap is keyed by the finite CursorIcon enum; bound by enum variants, no change, risk none)
(output_layout.rs:142-168, SurfaceOutputs.map HashMap grows per object id; bound by prune against live outputs, no change, risk none)
(xwayland.rs:65,122,129, watched_docks HashSet grows per dock window; bound by removal on destroy, no change, risk none)
(lock_cover.rs:27, covers BTreeMap keyed by output name; bound by output count, no change, risk none)
(virtual_output.rs:46, debug_outputs.rs:70, debug output Vecs hold only configured virtual outputs; bound by debug config, no change, risk none)
(.vendor/smithay-master/src/backend/renderer/damage/mod.rs:739,838, reference precedent shrinks recomputed damage and opaque-region Vecs with shrink_to_fit; chose a clear-based cap in the active cache owner instead of per-frame shrink so the hot paint path is untouched, risk none)

PASS: the retained-memory audit covers the surface, texture, glyph, damage and per-output caches of flamewm-wm-wayland; every growable Vec and HashMap is listed with its bound, and the one unbounded production cache now has a cap that cannot change rendered pixels. The single Rust edit is UNBUILT because builds and tests are forbidden for this worker; the coordinator must compile and run the scoped checks before treating the change as verified.