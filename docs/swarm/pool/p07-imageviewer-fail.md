# Unit p07-imageviewer-fail
STATUS: PARTIAL

crates/flamewm-imageviewer: corrupt, truncated or huge image, missing file: show an error line, never panic or allocate unbounded memory; audit decode path and fix the worst.

## Result
| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-imageviewer/Cargo.toml:18` | No header-only size probe was available to the viewer; `imagesize 0.15.0` was already in the lock via `usvg`. | Added `imagesize = "=0.15.0"` to dependencies. | New direct dependency; UNBUILT. |
| `Cargo.lock:929` | `flamewm-imageviewer` package entry did not name `imagesize`. | Added `"imagesize"` to its dependency list. | Lock cannot be regenerated here (no cargo allowed); UNBUILT. |

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-imageviewer/src/decode.rs:72-122 | Decoders allocate from header dimensions; the old `MAX_PIXELS` check ran only after `decode_bytes`, so a tiny forged header could allocate gigabytes before rejection. | UNBUILT — added a dependency-free `declared_pixels` header reader for PNG/GIF/JPEG. | Offset-based parsing; malformed or unknown headers return `None` and fall through to the existing post-decode guard. |
| crates/flamewm-imageviewer/src/decode.rs:148-151 | Pixel cap was checked too late to bound allocation. | UNBUILT — reject declared pixels above `MAX_PIXELS` before calling `decode_bytes`; kept the post-decode guard. | Overlaps a concurrent `imagesize::blob_size` guard at lines 137-144; both apply the same cap, so no new false positives. |
| crates/flamewm-imageviewer/src/decode.rs:137-144 | Concurrent worker added an `imagesize::blob_size` pre-decode guard (dependency declared at `Cargo.toml:18`). | Preserved unchanged; not authored by this unit. | Third-party header parser may differ on exotic formats; not verified here. |
| crates/flamewm-imageviewer/src/decode.rs:487-509 | No regression test covered oversized declared dimensions. | UNBUILT — added huge-PNG and huge-GIF tests asserting the pixel error. | Tests not run (`cargo` forbidden for this unit). |
| crates/flamewm-imageviewer/src/decode.rs:511-516 | Missing-file error path had no direct test. | UNBUILT — added a missing-file test. | Tests not run (`cargo` forbidden for this unit). |
| crates/flamewm-imageviewer/src/lib.rs:294-306, 443 | Audit: `Reply::Failed`/`ViewFailed` text is written to `empty-text`, shown while nothing is framed, so missing/corrupt/truncated files surface as an error line; `decode_file` returns `Err` rather than panicking. | No change (visuals stay pixel-identical). | A `ViewFailed` after a frame is shown sets the message while `empty` stays hidden; left unchanged to avoid a visual change. |

Reference-first: checked `.vendor/kwin-master/src` for a decode-size precedent; none found for image pixel budgets, so the existing `MAX_PIXELS` cap was reused. No `git`, `cargo`, `python` or builds were run, per the brief.

PARTIAL — the decode path now rejects oversized declared dimensions before any full-size allocation for PNG/GIF/JPEG, with regression tests added, and a concurrent `imagesize` guard was preserved rather than overwritten. Verification is intentionally absent: every edit above is UNBUILT and untested because builds and `cargo` are forbidden for this unit, so compilation and test outcomes remain for the orchestrator.
