# u111-error-paths

Worker-death fallbacks across explorer, shell, terminal, task manager. No feature or visual change;
each path that used to panic, spin or wait forever now degrades and reports once.

- shell/main: the one-shot startup-settings socket handler returned `Continue` after the worker's
  write end closed, so the fd read EOF on every reactor turn. It now retires the fd on EOF
  (`FdAction::Remove`). This is the largest idle-wakeup share of the four: one permanently readable
  source keeps the whole shell loop out of `poll` sleep for the session lifetime.
- shell/icon_loader: the icon wake pipe is drained per wake. A dead worker left the reader at EOF
  forever; `wake_drain` now reports EOF and `drain_icons_pending` unregisters the fd. A `poll`
  disconnect also releases the keys whose replies can never arrive and logs once, so placeholders
  stop waiting on a dead channel.
- shell/icon_loader + shell/main + weather_host: a refused worker thread is reported once instead
  of panicking (`std::thread::spawn`) or failing silently; weather/search/usage stay offline.
- explorer/workers: `JobPool::submit` no longer panics through `std::thread::spawn` when every lane
  is gone and the OS refuses another thread; `thumb_worker` stops decoding once the UI result
  channel is closed instead of draining the queue into a dead receiver. Failed pool lanes and a
  refused glide ticker (app/scroll) are reported once; the list keeps working without glide frames.
  Profiler and heap-trim threads in main are best-effort.
- explorer/main: profiler and heap-trim spawns use `Builder` so thread exhaustion cannot abort the
  window; both are optional.
- terminal/main: settings fetch, profiler window and heap trim use `Builder`; a refused settings
  thread keeps the cached theme instead of aborting before the first frame.
- task-manager: the sampler and settings threads use `Builder`; a sampler panic is logged at join
  and the wake drain turns a disconnected sample channel into one status line ("Process sampling
  stopped; showing the last sample") instead of silently repainting nothing. The last table stays.

Expected composition shift: idle wakeups (share: reactor turns, poll restarts) drop to zero for a
dead worker and for the completed one-shot settings source; no render or layout share is added.
