# Unit p49-ram-embedded-assets
STATUS: DONE

Workspace: list the largest embedded assets (include_bytes, include_str, fonts, images) per binary via rg; for each over 200 KB decide: lazy-load from disk, compress, or share between binaries. Rows only plus at most 2 safe edits.

## Result

Scope: production embeds only, found by rg over `crates/` and read of the embed sites; source sizes by stat.
Production edits applied: 0. None of the candidates over 200 KB admits a change that is both safe and
pixel-identical inside the two-edit cap, so the deliverable is rows. Every finding below is UNBUILT:
no compiler, test runner, git, cargo, Python or sleep loop was used; no binary or runtime RAM was measured.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-render-core/src/glyphs.rs:214` (`assets/fonts/DejaVuSansMono.ttf`) | 343,140 B fallback font, embedded in render-core and therefore in every binary; missed by the u30 top-15 list. Behind `OnceLock` and referenced only on the no-system-font path, so normal machines never fault the bytes into RAM. | None. Decision: lazy-load — already lazy at runtime by demand paging; a disk read would add a missing-asset failure path. | UNBUILT — static evidence only; a disk-read rewrite could change text output if assets are absent. |
| `crates/flamewm-lock/src/document.rs:21` (`assets/wallpapers/fitz-roy-laguna.webp`) | 655,330 B literal `include_bytes!` into the lock binary as `BUILTIN_BACKGROUND`. | None. Decision: no safe shrink — already lossy WebP; lossless re-encode is not provably smaller and downscaling changes pixels. | UNBUILT — static evidence only; visuals untouched. |
| `assets/raster/wallpaper.ppm` (staged at `crates/flamewm-desktop/build.rs:12,71`, referenced at `ui/desktop/index.html:4` → `flamewm-desktop.rwr`) | 2,596,066 B source, about 3.46 MiB decoded RGBA in the desktop RWR; largest embedded asset and the only real RAM win. | None applied. Decision: compress — a lossless WebP or PNG re-encode would cut binary and RAM, but it needs a new asset plus reference edits and a pixel check, beyond the two-edit cap. | UNBUILT — no re-encode and no reference edits; visuals untouched. |
| `assets/branding/flamewm-wordmark.png` (`ui/settings.html:12,76` → `flamewm-settings.rwr`) | 681,415 B PNG; byte-identical twin `assets/web/flamewm.png`. | None. Decision: share — u30b already repointed the test fixtures at the canonical wordmark, so the settings RWR embeds one copy only. | UNBUILT — static evidence only; the twin is retained by contract. |
| `ui/settings.html` (`crates/flamewm-settings/build.rs:17,48,82`) | 171,328 B; largest markup asset but below the 200 KB threshold. | None. | UNBUILT — raw bytes feed the RWR fingerprint, so no comment or whitespace stripping. |
| `.vendor/coconut-main/crates/plugin-kit/src/design.rs:55,59`; `.vendor/cosmic-epoch/cosmic-greeter/src/common.rs:124` | Reference precedent: vendors embed fallback fonts and greeter backgrounds with `include_bytes!`, matching our fallback pattern. | None. Reference only, read from the ignored vendor tree. | UNBUILT — reference evidence, never a dependency or runtime input. |

Below 200 KB and unchanged: `ui/explorer/explorer.html` (74,906 B), `ui/shell/flamewm.css` (40,153 B,
compiled into 23 shell RWR blobs), `ui/task-manager/task-manager.html` (38,920 B), `assets/web/breeze/
applications-internet.svg` (23,995 B) and `assets/raster/watermark.png` (15,919 B). Raw `include_str!`
uses of markup are all `#[cfg(test)]` gated and add no production bytes.

PASS — the audit is complete: every embedded asset over 200 KB is listed per binary with a lazy-load,
compress or share decision, and the 343 KB fallback font that the earlier u30 ranking omitted is now
recorded. No production edit was applied because none of the candidates could be changed safely while
keeping visuals pixel-identical within the two-edit cap; the wallpaper compression remains the largest
available win for a separately owned slice with a pixel check. All findings are UNBUILT and the
orchestrator must run the build and tests after the batch lands.
