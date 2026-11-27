# Unit p44-font-missing-everywhere
STATUS: PARTIAL

Crates flamewm-render-core, flamewm-terminal, flamewm-render-x11: no system font found: use an embedded fallback glyph set and one report; never panic on empty font. Verify every find_font user.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-render-core/src/glyphs.rs:145-222` | `find_font()` is private with exactly two callers: `with_run` (line 300) and the test `test_font` (line 549). Both consume `Option<Font>`; neither unwraps. Every system candidate failing reaches the bundled DejaVuSansMono `OnceLock` fallback added by u37; a missing or unparseable font yields `None`, never a panic. | Verified only; no edit. | None. |
| `crates/flamewm-render-core/src/glyphs.rs:206-211` | The one-shot `flamewm: no system font found` record lived inside the `FALLBACK` closure, so a bundled asset that failed to parse emitted no diagnostic at all — the case that most needs one. | Moved `NO_FONT_LOG` (`Once` + `eprintln!`) just above the closure. It still fires once per process, only after all system candidates failed, with the message and `blast_radius` tag unchanged. | UNBUILT — stderr-only; font choice, raster path and caches are unchanged, so visuals stay pixel-identical. |
| `crates/flamewm-terminal/src/ui.rs:134-153`, `crates/flamewm-terminal/src/glyphs.rs:1-3` | Terminal has no `find_font` user: it publishes the configured family to the document, and box/block glyphs are built as filled rectangles independent of any font file. u47 confirmed its `unwrap`/`expect` calls are all in test code. | Verified only; no edit. | None. |
| `crates/flamewm-render-x11/src/xft/backend_measure.rs:83-100`, `crates/flamewm-render-x11/src/xft/fonts.rs:119-145`, `crates/flamewm-render-x11/src/app/mod.rs:524-548` | Xft tries preferred, IBM Plex Sans, Noto Sans, DejaVu Sans and sans-serif; when none opens, `font_for` returns `Err`, callers log one fallback record and use the width estimate or core X path. NULL font handles are checked before use. | Verified only; no edit. | None. |
| `docs/swarm/units/u35-font-icon-fallback.md`, `docs/swarm/units/u37-fallback-font.md`, `.vendor/**` | u35 added the single missing-font records and u37 the embedded DejaVuSansMono fallback; `.vendor/` was scanned for a KDE/xfce/smithay precedent and none supersedes the local fontdue + Xft design. | Read-only evidence; no edit. | None. |

PARTIAL — the embedded fallback, one-report rule and no-panic contract were already in place across the three crates; this unit closed the remaining gap by hoisting the existing one-shot record above the bundled-font parse so a corrupted asset still reports once. Every `find_font` user was checked and is `Option`-safe. No build or test was run by contract, so the single edit is UNBUILT and the orchestrator must verify `flamewm-render-core` after the batch lands.
