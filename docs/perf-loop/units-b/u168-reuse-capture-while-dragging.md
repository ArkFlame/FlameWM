# u168 reuse one capture while a window is dragged (idea, needs a visual decision)

Area: window-backdrop capture during a drag (render/dock_backdrop.rs `capture_dock`, render/backdrop.rs).

Finding: after u162/u166/u167 a drag is ~72-75% window-backdrop capture: ~22 captures of ~26 ms each in a 1.8 s drag. Per-capture cost has no big slice left (gpu_wait 3.4, blur 3.2, remainder 2.1, store 0.3 ms), so the remaining lever is the capture COUNT.

Idea: while a window is dragged over a desktop whose other windows have not committed, capture once a region larger than the window (window rect plus the drag's likely travel) and sample sub-rectangles of it with the GPU source rect, as titlebars already do with the reduced desktop capture. Recapture only when something beneath commits or the window leaves the captured region.
Risk: the blur behind a moving window would lag content beneath it by up to one capture; and a larger capture costs more per capture. Both change how the glass looks in motion, so this needs the user's visual approval before work starts.
Acceptance if approved: drag probe captures per drag down (22 -> a handful), frame diff on a static desktop within 2/byte, no stale blur when a window beneath updates.
