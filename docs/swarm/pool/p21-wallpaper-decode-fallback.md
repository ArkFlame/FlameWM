# Unit p21-wallpaper-decode-fallback
STATUS: DONE

crates/flamewm-desktop and flamewm-lock wallpaper loading: undecodable or missing wallpaper falls back to a solid theme colour with one failure record; no panic, no black flash; audit and fix.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-desktop/src/wallpaper_loader.rs:296 | On a first undecodable/missing wallpaper, `poll()` kept `DefaultBlackWatermark`, which `project_background` paints black plus watermark: a black flash. Previously presented images were retained correctly. | On `LoadResult::Failure`, paint `Solid(DEFAULT_SOLID)` only when the current presentation is still `DefaultBlackWatermark`; retain any image/solid already on screen. Diagnostic still recorded once. | UNBUILT. First-failure pixels change from black to theme solid (intended); successful decodes and all existing retention tests are pixel-identical. |
| crates/flamewm-desktop/src/main.rs:814 | The wake callback consumed the `PollOutcome` before `refresh_settings_projection` polled again, so its `Failed` diagnostic log branch never ran: the failure record was stored but never emitted. | Emit the single failure record from the wake callback on `PollOutcome::Failed` by borrowing the outcome; the consumed completion cannot be logged a second time. | UNBUILT. Extra `eprintln!` on failure only; borrow keeps the later `matches!` valid. |
| crates/flamewm-lock/src/document.rs:100 | Audit only: `background_image` already falls back through `decoded_background(...).unwrap_or_else` to reference navy (default) or a flat configured colour; `decode_file_shared`, `cover_rgba`, built-in WebP and `post_process` all use `Result`/`Option`, no `unwrap`/`expect`/`panic!` on the decode path. | None — no defect found; no edit made, visuals untouched. | None. |

PASS — desktop first-load failures now fall back to the solid theme colour instead of black while retaining any valid wallpaper, and the single failure diagnostic is emitted from the wake callback that actually observes it; the lock-side fallback and panic audit needed no change. All edits are marked UNBUILT because the brief forbids cargo/builds; verification is left to the orchestrator's batch build.
