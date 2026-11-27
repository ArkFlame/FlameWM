# u03 — Desktop idle wakeup/repaint audit (retry)

**Scope:** `crates/flamewm-desktop/src` except `projection.rs`, plus this file. Read-only audit of
widget sampling, timers, redraw triggers, wallpaper cache and icon grid, with safe cuts that do not
change visuals. RULES: no `git`, no `cargo`, no Python, no sleep loops, no builds or tests; shared
checkout, orchestrator commits. Every code edit below is **UNBUILT** (no compiler verification was
permitted in this pass).

## Findings

| # | Area | File:line | Cost | Change | Risk | Status |
|---|------|-----------|------|--------|------|--------|
| 1 | Widget redraw / timers | `widgets/render.rs:161-189` (time-only fast path), `widgets/sync.rs:391-430` (`sync_calendar`) | A calendar in the time-only fast path rewrote its whole week grid (~42 cells × text/foreground/background) at 1 Hz whenever a seconds clock existed, although only the date can change its output. | Capture the stored key's previous time and skip `Calendar` slots in the fast path when year/month/day are unchanged; a day rollover still redraws, and the key already proves all non-time inputs equal. | Low — calendar output is a pure function of date plus widget styling; styling changes take the full-sync path, and `WallTime` is `Copy`. | UNBUILT |
