# u03-taskmgr changes
- graph: `History` is now a fixed `[f32; SAMPLES]` ring with O(1) push; removed the `Vec::remove(0)` memmove per sample per series; render copies into a stack array.
- model: `cpu_ticks` parses the per-CPU counters into a stack `[u64; 8]` instead of allocating a `Vec` for every core line each sample.
- model: `memory` reads the four `/proc/meminfo` counters it needs in one pass instead of building and hashing a `HashMap` of all fields each sample.
- model: `command` drops the redundant `args.replace('\0', " ")`; `sanitize` already maps NUL/control bytes to spaces, removing one String allocation per refreshed task.
- lib: metrics text and core meters repaint only when a sample arrives or the window resizes (`metrics_dirty`), skipping the per-core `format!` work on hover, clicks, sort and key paints.
- lib: tree indentation uses a static `INDENTS` table instead of `"  ".repeat(depth)` per visible row per paint.
