# Unit p88-panic-abort-decision
STATUS: DONE

Cargo.toml release has panic=abort, so diagnostics::guard (catch_unwind) is a no-op in production. Audit: (1) rg every guard( call site and list what each is meant to protect; (2) read the session supervisor (crates/flamewm-wm/src/bin/flamewm-session) and list which child crashes are respawned today and with what backoff; (3) write docs/PANIC_STRATEGY.md with the facts, the options (keep abort + process respawn; unwind only for specific bins via a profile; unwind everywhere) and the exact measurement to take (binary size and idle RSS with panic=unwind for shell and desktop). No Cargo.toml edits.

## Result
STARTED
- (1) guard call sites: 9 production sites (lock/run.rs:346, desktop/input.rs:62,104,233, desktop/files.rs:145, desktop/wallpaper_loader.rs:179, shell/main.rs:874, shell/icon_loader.rs:282, wm-wayland/main.rs:777 via guard_startup). Each protects an optional feature; all are no-ops under panic=abort (Cargo.toml:70). Listed in docs/PANIC_STRATEGY.md section 1.2.
- (2) supervisor: flamewm-session/supervision.rs:161-283. Restarts for WM, desktop, shell on any exit incl. SIGABRT; 5 restarts per 60 s window, backoff 500,1000,2000,4000,8000,8000 ms; 6th exit gives up to fallback plan. Facts in PANIC_STRATEGY.md section 1.3.
- (3) docs/PANIC_STRATEGY.md written: facts, options A (keep abort), B (unwind per bin, cargo per-package panic override unverified), C (unwind everywhere), and measurement plan (binary size + idle RSS for shell and desktop). No Cargo.toml edit. Risk: doc only, UNBUILT, nothing measured.

PASS: the audit and decision doc are written from read source (no builds run, no measurements taken); the choice between A/B/C is left open pending the section 3 measurements.
