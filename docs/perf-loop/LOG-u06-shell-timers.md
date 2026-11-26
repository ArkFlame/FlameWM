# u06-shell-timers
- notification_center: `view` builds groups in one newest-first pass over `history.entries()`, dropping the per-row `entries().find(seq)` scan (O(groups*rows*entries) -> O(entries)) and the duplicate `groups()` walk/alloc.
- quick_controls/tick: the reprojection turn projects from the live `QuickControlSnapshot` under a scoped borrow instead of deep-cloning the whole snapshot (system/audio/network/weather/calendar state) per system snapshot change.
- quick_controls/projection: the network popup borrows `snapshot.network_query` instead of cloning the query struct on every network reprojection.
- quick_controls/run: the profiler window timer is registered only when `FLAMEWM_PROFILE=1`, so an idle popup host no longer wakes every 60 s to flush a disabled profiler.
- launch_feedback: the four pending-dot ids come from a static table; the 50 ms animation tick no longer heap-allocates four `format!` strings per frame change.
- osd: styles and card values share one `with_document` pass per show (volume and workspace cards), halving document writes on every OSD appearance; the backdrop stays a runtime property set after the pass.
