# Unit p88-panic-abort-decision
STATUS: DONE

Cargo.toml release has panic=abort, so diagnostics::guard (catch_unwind) is a no-op in production. Audit: (1) rg every guard( call site and list what each is meant to protect; (2) read the session supervisor (crates/flamewm-wm/src/bin/flamewm-session) and list which child crashes are respawned today and with what backoff; (3) write docs/PANIC_STRATEGY.md with the facts, the options (keep abort + process respawn; unwind only for specific bins via a profile; unwind everywhere) and the exact measurement to take (binary size and idle RSS with panic=unwind for shell and desktop). No Cargo.toml edits.

## Result
STARTED
- (1) guard call sites: 9 production sites (lock/run.rs:346, desktop/input.rs:62,104,233, desktop/files.rs:145, desktop/wallpaper_loader.rs:179, shell/main.rs:874, shell/icon_loader.rs:282, wm-wayland/main.rs:777 via guard_startup). Each protects an optional feature; all are no-ops under panic=abort (Cargo.toml:70). Listed in docs/PANIC_STRATEGY.md section 1.2.
- (2) supervisor: flamewm-session/supervision.rs:161-283. Restarts for WM, desktop, shell on any exit incl. SIGABRT; 5 restarts per 60 s window, backoff 500,1000,2000,4000,8000,8000 ms; 6th exit gives up to fallback plan. Facts in PANIC_STRATEGY.md section 1.3.
- (3) docs/PANIC_STRATEGY.md written: facts, options A (keep abort), B (unwind per bin, cargo per-package panic override unverified), C (unwind everywhere), and measurement plan (binary size + idle RSS for shell and desktop). No Cargo.toml edit. Risk: doc only, UNBUILT, nothing measured.
- Verification pass rows (file:line — finding — change — risk):
  - `crates/flamewm-session-core/src/diagnostics/failure.rs:273-293, 250-267, 425-453` — doc cited stale lines 249, 226-243, 378-400 for `guard`, hook and tests — corrected those three references in PANIC_STRATEGY.md:11,13,14 — risk: documentation accuracy only; UNBUILT.
  - `docs/PANIC_STRATEGY.md:30` — test-only call sites listed stale lines 379/385/397-399 — corrected to 427, 433, 445-447 — risk: documentation accuracy only; UNBUILT.
  - `docs/PANIC_STRATEGY.md:15` — `.vendor/` precedent had not been recorded; COSMIC apps ship `panic = "abort"`, no KDE/xfce/smithay corpus present — added one reference sentence — risk: read-only reference evidence; UNBUILT.
- Production edits this pass: none. Builds/tests/measurements: not run (per worker rules).

PASS: the audit and decision doc are written from read source and the cited line numbers were re-verified against current source; no builds or measurements were run, Cargo.toml was not edited, and the choice between A/B/C stays open pending the section 3 measurements.
