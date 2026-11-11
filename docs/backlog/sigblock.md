# SIGINT/SIGTERM ownership and GPU-thread mask audit

## Current-source finding

The Wayland compositor entry point is `crates/flamewm-wm-wayland/src/main.rs` (`fn main`, backend setup, and event-loop startup). The active Wayland crate and the renderer crate contain no explicit `sigprocmask`, `pthread_sigmask`, or signal-set installation for SIGINT/SIGTERM. The only direct `std::thread::spawn` in the compositor crate found by the source search is in `src/probe.rs`; the renderer's GPU work is routed through its per-thread GPU state (`crates/flamewm-render-x11/src/accel/mod.rs`), while the existing GPU benchmark's worker spawn is an example, not compositor startup.

Consequently this static source audit cannot identify a repository-owned thread that currently owns SIGINT/SIGTERM, nor establish the actual inherited mask of GPU threads. POSIX threads inherit their creator's signal mask, but these sources do not set or query that mask. The process launcher/runtime environment and dependency-created threads are outside this evidence.

## Safe next slice

Add a bounded, Linux-only startup diagnostic/test at the compositor process boundary: query and record the calling thread's SIGINT/SIGTERM mask before backend/thread startup, then query the actual GPU worker thread at its creation boundary. Keep signal disposition/handling unchanged; do not unblock signals or take ownership in this diagnostic slice. If the intended contract is instead to block these signals, first name the sole signal-waiting owner and establish its lifetime before applying the mask before *any* thread creation. Regression coverage should assert the chosen mask contract on both the initiating thread and a spawned worker, without relying on GPU hardware.

## Limits

Static audit only. No signal behavior, inherited mask, dependency thread behavior, or runtime ownership was measured. `scripts/gate flamewm-wm-wayland` is a compile/test gate, not runtime signal evidence.
