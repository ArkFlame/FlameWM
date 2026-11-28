# Unit p84-imageviewer-large-image
STATUS: PARTIAL

Crate flamewm-imageviewer: 100 MP image: decode with downscale (imagesize first, reduce at decode) instead of full RGBA; free previous image on navigate; refuse above a byte cap with a message. Max 3 edits.

## Result
STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-imageviewer/src/decode.rs:14,20-22,26-36 | No bounded decode target existed; a 100 MP source became a ~400 MiB full RGBA. | UNBUILT — imported `decode_bytes_at`, added `MAX_DECODE_EDGE = 4096` and `decode_target` so the header probe picks a reduced edge; small images still decode at natural size. | Float rounding of the shorter edge and formats `imagesize` cannot probe fall through to natural decode; no compile proof. |
| crates/flamewm-imageviewer/src/decode.rs:152-174 | The header probe only rejected oversized images; it never requested a reduced decode. | UNBUILT — probe once, keep the existing `MAX_PIXELS` bomb gate, then call `decode_bytes_at(..., ScaleFilter::Area)` when a reduced target is selected. | `decode_bytes_at` still decodes natural size inside `flamewm-image-core`; the viewer no longer retains a full raster, but peak RSS inside the core is unchanged. |
| crates/flamewm-imageviewer/src/decode.rs:301-313 | Navigating away left the previous decoded raster and its reduced copy resident. | UNBUILT — `activate` retains only the new path in the cache and clears `reduced`. | All prefetched neighbours are dropped, so back-navigation re-decodes; displayed pixels are unchanged. |
| crates/flamewm-imageviewer/src/decode.rs:145-149; crates/flamewm-imageviewer/src/lib.rs:294-300,443 | The byte cap and its on-screen error text were already present. | No change — preserved the existing gate and message path. | Existing behaviour, not exercised here. |
| crates/flamewm-imageviewer/src/decode.rs:515-542 | No regression coverage for the reduced target or the navigate free. | UNBUILT — added `large_header_selects_a_reduced_decode_target` and `activate_frees_cached_images_except_the_new_active_one`. | Tests were not run (cargo forbidden for this unit). |

Reference-first: `.vendor/cosmic-epoch/cosmic-viewer/viewer-core/src/loader.rs:241-347` reads image dimensions first and uses a scaled decode instead of a full raster; `.vendor/cosmic-epoch/cosmic-files/src/large_image.rs` bounds decode work to `max_size`. No `git`, `cargo`, `python`, builds or sleep loops were run.

PARTIAL — a 100 MP image is now decoded at a bounded edge via `imagesize` + `decode_bytes_at`, navigation releases the previous decoded image, and the existing byte-cap refusal still surfaces its message. Every production edit above is UNBUILT and the two new tests are unrun because this unit forbids builds, tests, git and cargo; compilation, pixel identity and RSS impact remain for the orchestrator to verify.
