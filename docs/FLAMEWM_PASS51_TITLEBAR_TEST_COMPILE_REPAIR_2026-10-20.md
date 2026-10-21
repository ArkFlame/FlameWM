# FlameWM Pass 51 — Titlebar scale test compilation repair

Baseline: FlameWM 0.1.0 Pass 50 integrated source overlay. No new compiler diagnostics were provided with this request. Source inspection found one definite Rust compile error in the new Windows 11-style titlebar test:

`crates/flamewm-wm-x11/src/chrome.rs` test `windows11_square_hover_coverage_at_100_135_200_percent` passed `RESIZE_T` without importing/qualifying it. Rust name resolution would fail with `E0425` on `cargo test` for `flamewm-wm-x11`, even though the library build may succeed.

Fix: call the existing and already imported `resize_thickness_for_titlebar(full)` instead. This also makes the test's rectangle layout use precisely the same per-titlebar-height resize thickness as production `control_backplates_for()`, at 100%, 135%, and 200%.

Files changed: one Rust test line, plus this report and `tools/test_flamewm_pass51_chrome_compile_contract.py`. All Pass 50 production paths remain unchanged. No unrelated warnings were suppressed.

Proof: the new Python guard rejects the original Pass 50 source and accepts Pass 51. Existing source/packaging suites should be run as available. Native `cargo check --workspace --all-targets`, `cargo test -p flamewm-wm-x11`, `./scripts/build-deb` and real X11 visual checks require a Rust/X11 host and were not run in the authoring container (no installed `cargo` or `rustc`). Further failures may emerge; obtain actual compiler output before broader edits.

Apply only Pass 51 over the full runtime-asset-bearing checkout, then run `tar --touch` extraction to avoid stale Cargo timestamp artifacts. Do not reapply Pass 50 or earlier over it.
