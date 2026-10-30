# OpenCode continuation — 2026-10-28 (short window, 18:29–18:41 -03)

Read-only verification window. **No source edits, no commit, no push** were made by OpenCode.
Evidence logs: `.agents/runtime/opencode-20261028/`.

## Live state at handoff

- HEAD `44ae4277` on `main`; index empty; remote `origin = github.com/ArkFlame/FlameWM`.
- Uncommitted, **owned by the terminated Claude GPU-clip worker** — do not overwrite, do not commit unverified:
  - `crates/flamewm-render-x11/src/app/gpu_frame.rs` (+316/−…): real clip lowering — `lowering_allowed()` no longer
    rejects a non-empty clip stack; `gpu_frame_sources()` maintains a `PushClip`/`PopClip` rect stack;
    `glyph_clip()` / `shadow_clip()` cut text and shadow masks; `visible_under() != Whole` skips images.
    New tests: `clipped_fills_and_thumbs_cut_to_the_visible_part_with_a_square_corner`,
    `a_stroke_cut_by_a_clip_keeps_the_xrender_path`, `glyph_clip_rounds_the_stack_bounds_like_the_text_path`,
    `node_backdrops_keep_the_xrender_path_and_every_one_is_named`.
  - `crates/flamewm-render-x11/src/app/render.rs` (+162/−29): the above wiring **plus the temporary trace below**.
  - `crates/flamewm-render-x11/src/accel/mod.rs` (+23): `trace_cpu_batch()` test hook and one debug test.
- Untracked `target-rel/` (warm release build dir). `.gitignore` has `/.target*/` only, so `target-rel/` is **not**
  ignored and shows up as thousands of untracked files under `-uall`.

## Executed checks (this window)

| Command | Exit | Result |
| --- | --- | --- |
| `CARGO_TARGET_DIR=target-rel cargo check --release --locked -p flamewm-render-x11 --lib` | 0 | 54 crates, 12.05 s (warm) |
| `... cargo test --release --locked -p flamewm-render-core -p flamewm-render-x11 --lib` | 0 | core 90 pass / 1 ignored; x11 375 pass / 3 ignored |
| same combined run, first attempt | 101 | reported `running 378 tests` then failure with no name captured; **not reproduced** in two later runs (`combined-lib-tests.log`, `x11-lib-tests.log`). Treat as unreproduced flake until it fails with a saved log. |

Not run: `cargo fmt --check`, clippy, `cargo check --workspace --all-targets`, `./scripts/check`, Xephyr/nested Wayland,
any GPU activation or screenshot proof.

## Remaining blocker for W1 (accurate, source-read)

`render.rs::gpu_frame_pixels` still runs diagnostic work on the production hot path **before** returning the frame:

- allocates a full-size CPU reference and renders it with `accel::trace_cpu_batch` on **every candidate GPU frame**;
- allocates one more full frame **per op prefix** (`for k in 1..=ops.len()`) on the first frame;
- walks every pixel twice for `max_diff/mean_diff/black_*` and formats every op through `format!`.

Also unsafe regardless of gating: `let probe = (300 * width + 300) * 4;` panics/UB on any frame narrower/shorter than
301 px. `FLAMEWM_RENDER_GPU_FRAME … xrender fallback <reason>` (deduped) and `… gpu paint active` must survive the cleanup.

## Exact next step for Claude

1. In `render.rs::gpu_frame_pixels`, delete the `GPU_TRACE` prefix loop, CPU reference, pixel diff, `kinds` census and the
   `black_seen_once_marker()`/probe block; keep `log_gpu_fallback(...)` and the activation log. No CPU rendering on the off
   path; no coordinate assumptions.
2. Delete `accel::trace_cpu_batch` and its debug test in `accel/mod.rs` if nothing else uses it.
3. Add the regression guard (ordinary GPU path performs no CPU reference render / no per-frame pixel compare) + tiny-frame,
   empty-ops and forced-GPU-failure tests.
4. Then: fmt, focused tests, workspace check, Xephyr run of the **real** document, capture
   `FLAMEWM_RENDER_GPU_FRAME … whole-frame gpu paint active` plus `glGetString(GL_RENDERER)`, CPU-vs-GPU frame diff.
   `NodeBackdrop` and `StrokeClipped` still correctly keep the whole-frame XRender path — that is PARTIAL, not 100%.
5. Only then commit: `perf(render-x11): clip-aware gpu frame paint` (or separate trace-removal commit first if the clip work
   is not visually proven).

Issue #13 remains OPEN. `FINISH_LINE_VERIFICATION.md` is still stale relative to this WIP.

## `tools/verify_rwr_promotion.py` — why `./scripts/check` is RED (read-only finding)

The reference corpus **does exist**: `.vendor/RustWebRender-0.0.9/` is present and contains
`crates/rustwebrender-*/**`. The gate is live, not obsolete; `scripts/check:` calls it as line 2. Fresh run, exit 1,
12 errors in two classes:

1. **Stale accepted pins (expected, needs owner-level re-accept with real delta text).** Hash mismatch on
   `flamewm-render-core/src/{lib,codec}.rs`, `flamewm-render-compiler/src/compile.rs`,
   `flamewm-render-x11/src/{xlib,xft,xrender}.rs`; declared-translation drift on
   `flamewm-render-compiler/src/{css,html}.rs`, `bin/flamewm-render-inspect.rs`; split-item drift on
   `pub struct X11Config`, `impl Default for X11Config`, `fn keyboard_text`.
   These owners were legitimately edited after the 2026-10-09 acceptance. Fix = update each `ACCEPTED_OWNERS` pin with an
   accurate `delta`+`reason`, never delete the pin or the gate.
2. **One stale structural anchor (not a hash).** `required motion dispatch anchor missing:
   crates/flamewm-render-x11/src/app/events.rs: inside: hover == Some(index)`. The vendor original is a struct literal
   (`.vendor/RustWebRender-0.0.9/crates/rustwebrender-x11/src/lib.rs:345`); FlameWM refactored that call to positional
   `action_event(...)` arguments, so the mechanic survives verbatim at
   `crates/flamewm-render-x11/src/app/events.rs:574` as `hover == Some(index),` — only the `inside: ` field label is gone.
   Behaviour is preserved (**not** a motion-dispatch regression), but the anchor text no longer matches.
   Fix = re-anchor the gate to the current exact text and record the translation in the same commit; do **not** drop the
   anchor, because it is the only mechanical proof that motion dispatch still carries the hover-inside flag.

Do not regenerate the archive or touch hashes in this window; this needs a deliberate, reviewed commit.
