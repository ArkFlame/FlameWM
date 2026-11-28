# w22 — Embedded assets audit: `include_bytes!`, `include_str!`, fonts, icons, wallpapers

**Status: UNBUILT.** No `git`, `cargo`, Python, build or test command was run in this shared checkout.
Every byte count below comes from `wc -c` (cross-checked with `ls -l`) on files that exist in this
tree; generated `OUT_DIR/*.rwr` sizes are **unmeasured** because `target/**` contains zero `.rwr`
files and no build was allowed. No binary size, RSS, decoded-pixel size or runtime identity was
measured. Production edits applied: **0** (this is an audit; the two-edit cap was not needed).

## Route and evidence ledger

- Route: `flamewm-source-first` and `flamewm-reference-mining` loaded; workspace `AGENTS.md`,
  `scripts/version` (read, never run — it calls Python), `.opencode/HARNESS.md`, `.opencode/ROUTES.md`
  and `.agents/{README,CONTRACTS,JOBS}.md` read. `scripts/version` itself was not executed.
- Commands: `rg` over `crates/` for `include_bytes!` / `include_str!`; `wc -c` and `ls -l` on every
  literal asset named below; `sha256sum` on duplicate candidates; `find target -name '*.rwr'`;
  `read` of each embed site and of `packaging/install-manifest.txt`; `rg` over `Cargo.toml` for the
  crate-to-binary owner map.
- Reference-first: `.vendor/smithay-master/anvil/src/cursor.rs:9,26-40` embeds a fallback cursor
  (`FALLBACK_CURSOR_DATA`) and only uses it when the configured theme cannot be loaded — the exact
  precedent for our `BUNDLED_PREVIEW`. `.vendor/coconut-main/crates/plugin-kit/src/design.rs:55,59`
  embeds TTFs for the same fallback reason. Both are ignored, read-only evidence; neither is a
  dependency or runtime input. No clone was needed.

## 1. Literal bundled assets embedded in production code

| file:line | asset | bytes | embedded by | note |
| --- | --- | ---: | --- | --- |
| `crates/flamewm-render-core/src/glyphs.rs:216` | `assets/fonts/DejaVuSansMono.ttf` | 343,140 | every binary that links `flamewm-render-core` | Last-resort font, behind `OnceLock` on the no-system-font path (`:201-221`). Demand-paged: a machine with system fonts never faults these bytes into RAM. |
| `crates/flamewm-lock/src/document.rs:21` | `assets/wallpapers/fitz-roy-laguna.webp` | 655,330 | `flamewm-lock` | `BUILTIN_BACKGROUND`, used when `lockBackgroundImage` is empty (`:20-22`). |
| `crates/flamewm-lock/src/document.rs:24` | `assets/web/breeze/lock/system-suspend.svg` | 801 | `flamewm-lock` | Power icon, `POWER_ICONS[0]`. |
| `crates/flamewm-lock/src/document.rs:25` | `assets/web/breeze/lock/system-reboot.svg` | 524 | `flamewm-lock` | Power icon. |
| `crates/flamewm-lock/src/document.rs:26` | `assets/web/breeze/lock/system-shutdown.svg` | 714 | `flamewm-lock` | Power icon. |
| `crates/flamewm-lock/src/document.rs:27` | `assets/web/breeze/lock/system-switch-user.svg` | 1,864 | `flamewm-lock` | Power icon. |
| `crates/flamewm-lock/src/document.rs:29` | `assets/web/lock/avatar-outline.svg` | 338 | `flamewm-lock` | `AVATAR_OUTLINE`. |
| `crates/flamewm-integrations-linux/src/themes/cursor.rs:56` | `assets/cursors/cursors/left_ptr` | 166,688 | every binary that links `flamewm-integrations-linux` | `BUNDLED_PREVIEW` for a theme with no usable cursor; `OnceLock` + `REPORTED` (`:44-60`), same shape as smithay anvil. |
| `crates/flamewm-terminal/src/fetch.rs:12` | `assets/web/flamewm-icon.svg` | 985 | `flamewm-terminal` | `IMAGE_LOGO` (repointed from the byte-identical Start asset by u30b). |

Test-only literal embeds (not shipped in any production binary): font re-embed at
`glyphs.rs:563` (same 343,140 B); `flamewm-desktop/src/wallpaper_loader.rs:794,965`
(`testdata/wallpaper-test.webp`, 36 B); `flamewm-image-core` fixtures — `first-frame-4x4.gif`
48 B (`:44,53` of `gif_frame.rs`), `baseline-16x16.jpg` 638 B, `gray-16x16.jpg` 333 B,
`progressive-16x16.jpg` 522 B, `cmyk-16x16.jpg` 362 B, `wide-4097x1.jpg` 1,654 B,
`wide-16385x1.jpg` 4,215 B (`jpeg.rs:90-95`), plus the WebP fixture at `webp.rs:44`; the
media-store fixture re-embeds at `flamewm-integrations-linux/src/media_store.rs:670,678,902,917,987,996,1008,1018`.

## 2. Generated artifacts embedded by `include_bytes!` (sizes UNMEASURED)

Build scripts compile HTML/CSS into `OUT_DIR/*.rwr`; the runtime embeds those bytes. No `.rwr`
exists in `target/**`, so sizes are not available without a build. Source inputs that feed them
were measured: `ui/settings.html` 171,328 B, `ui/desktop/desktop.css` 17,544 B,
`ui/shell/flamewm.css` 40,153 B.

| file:line | artifact(s) | embedded by |
| --- | --- | --- |
| `crates/flamewm-shell/src/runtime.rs:37,39,40,42,44,45,47,48,49,52,54,55,57` | 13 shell RWR blobs (panel, vertical panel, dock, dock-menu, dock-label, start, task-menu, media, usage, launcher, clipboard, polkit, bluetooth) | `flamewm-shell` |
| `crates/flamewm-shell/src/launch_feedback.rs:20` | `flamewm-launch-feedback.rwr` | `flamewm-shell` |
| `crates/flamewm-shell/src/switcher.rs:26` | `flamewm-switcher.rwr` | `flamewm-shell` |
| `crates/flamewm-shell/src/main.rs:40,44` | notification, notification-center RWRs | `flamewm-shell` |
| `crates/flamewm-shell/src/projection/audio.rs:234,872`; `network.rs:256`; `weather.rs:441,455`; `projection/panel/template_tests.rs:8,524` | audio, network, panel, battery, vertical-panel RWRs (test sites marked) | `flamewm-shell` |
| `crates/flamewm-desktop/build.rs:12,71,117`; `src/main.rs:851`; `src/projection.rs:1219` | `flamewm-desktop.rwr`, includes `assets/raster/wallpaper.ppm` (2,596,066 B) and `assets/raster/watermark.png` (15,919 B) | `flamewm-desktop` |
| `crates/flamewm-settings/src/view.rs:8`; `crates/flamewm-explorer/src/main.rs:40`; `crates/flamewm-finder/src/lib.rs:55`; `crates/flamewm-task-manager/src/lib.rs:28`; `crates/flamewm-imageviewer/src/lib.rs:31`; `crates/flamewm-archiver-app/src/window.rs:28` | settings, explorer, finder, task-manager, imageviewer, archiver RWRs | their respective binaries |
| `crates/flamewm-assistant/src/lib.rs:54`; `src/osd_host.rs:24` | assistant + assistant-osd RWRs | `flamewm-assistant` |
| `crates/flamewm-terminal/src/main.rs:31,32,33` | three terminal tier RWRs | `flamewm-terminal` |
| `crates/flamewm-lock/src/document.rs:18` | `flamewm-lock.rwr` | `flamewm-lock` |

`include_str!` (66 files in `crates/`) reads source text, not a separate font/icon/wallpaper asset:
verified sites are `crates/flamewm-settings/src/view.rs:56`, `crates/flamewm-ui-core/src/css_contract.rs:193,226`,
`crates/flamewm-shell/src/taskbar/context_menu.rs:348`, `crates/flamewm-desktop/src/projection.rs:677,694,828,843`
and `src/wallpaper_loader.rs:764` — all `#[cfg(test)]`-gated CSS/HTML/Rust-source reads. No
`include_str!` of a binary asset was found.

## 3. Duplicates embedded in more than one binary

| duplicate | bytes | evidence | risk |
| --- | ---: | --- | --- |
| `DejaVuSansMono.ttf` | 343,140 per binary | two source sites (`glyphs.rs:216` production, `:563` test); direct `flamewm-render-core` dependents verified at `flamewm-assistant/Cargo.toml:18`, `flamewm-lock:17`, `flamewm-render-x11:11`, `flamewm-render-compiler:13`, `flamewm-ui-x11:19`, `flamewm-wm-x11:25`, `flamewm-wm-wayland:32`, `flamewm-ui-document:15`, and their binary consumers (shell, desktop, explorer, finder, settings, task-manager, imageviewer, terminal, wm) | The exact binary count cannot be confirmed without `cargo tree` (forbidden here) — reported as a verified dependency set, not a measured copy count. |
| `left_ptr` cursor | 166,688 per binary | one site (`cursor.rs:56`); direct `flamewm-integrations-linux` dependents at shell:21, wm:14, desktop:23, explorer:21, wm-wayland:28, audio-helper:17, imageviewer:15, settings:16, finder:18, archiver-app:14, wm-x11:17 | Same unmeasured-copy caveat; the asset is also installed at `packaging/install-manifest.txt:28` and inside `assets/` (`:26`). |
| `flamewm-icon.svg` / `flamewm-start.svg` | 985 each, same SHA `3ff460d93a31f238…` | terminal embeds the former (`fetch.rs:12`); shell RWR embeds the latter (`ui/shell/dock.html:10`) — genuine cross-binary byte duplicate | Byte-identical, so pixels match; consolidation needs changes in two owners and a pixel check. |
| `flamewm-wordmark.png` / `assets/web/flamewm.png` | 681,415 each, same SHA `3de9b2c383935cb0…` | one embedded copy via `ui/settings.html` into the settings RWR; u30b repointed the test fixtures | Duplicate is on disk only, **not** in two binaries; the twin is retained by contract. |
| `assets/raster/wallpaper.ppm` | 2,596,066 | staged only by `flamewm-desktop/build.rs:12,71` | No second binary embed found; also installed under `assets/` (`install-manifest.txt:26`). |

## 4. Lazy-load candidates from the install dir (no visual change)

`packaging/install-manifest.txt:26-28` installs `assets/` to `/usr/share/flamewm/assets/` and the
cursor tree to `/usr/share/icons/FlameWM-Breeze-Dark/cursors`, so the source files are available at
runtime. Every candidate below is a **spec, not an edit**; retaining the embedded fallback keeps
pixels identical only while the installed file is present and valid.

| candidate | bytes | lazy-load shape | risk / decision |
| --- | ---: | --- | --- |
| Lock wallpaper `fitz-roy-laguna.webp` (`document.rs:21`) | 655,330 | try `/usr/share/flamewm/assets/wallpapers/fitz-roy-laguna.webp` on first lock, fall back to the embedded bytes | Best simple candidate: same WebP bytes, so the decoded image is pixel-identical. Only wins binary size if the embedded copy is dropped, which adds a missing/corrupt-file failure path. Keep fallback → no visual change, no size win. **No edit applied.** |
| Desktop wallpaper `wallpaper.ppm` (RWR) | 2,596,066 | read from `/usr/share/flamewm/assets/raster/wallpaper.ppm` at runtime instead of inlining into the compiled document | Largest theoretical win, but the bytes are part of the RWR fingerprint/contract (`build.rs:108-117`); moving them out needs a product-code change plus a pixel check. Out of scope for a safe three-edit cap. |
| Settings wordmark PNG (RWR) | 681,415 | reference `/usr/share/flamewm/assets/branding/flamewm-wordmark.png` instead of inlining | Same RWR-contract problem; no safe edit. |
| Fallback font (`glyphs.rs:216`) | 343,140 | read `/usr/share/flamewm/assets/fonts/DejaVuSansMono.ttf` on the fallback path | Already demand-paged behind `OnceLock`; a disk read adds a failure path and can change text output if the asset is absent. **Keep embedded.** Matches coconut precedent. |
| Fallback cursor (`cursor.rs:56`) | 166,688 | read the installed `left_ptr` on the fallback path | Same as font; already lazy and once-parsed, matches smithay precedent. **Keep embedded.** |
| Lock power icons + avatar | 4,241 total | read from installed asset tree | Too small to justify a new I/O/failure path. **Keep embedded.** |
| Generated `OUT_DIR/*.rwr` blobs | unmeasured | load from install dir | Not currently packaged; would need a packaging change. No edit. |

## Decision and verdict

PASS — the audit is complete: every literal bundled asset has a `file:line` and a `wc -c` byte
count, every generated `include_bytes!` embed has an owner binary, the cross-binary duplicates and
byte-identical twins are listed with hashes, and the lazy-load candidates are ranked with their
visual-risk conditions. The only genuine cross-binary byte duplicates are the fallback font, the
fallback cursor, and the 985 B icon pair; the largest single asset is the 2,596,066 B desktop
wallpaper. **Zero production edits** were applied because no candidate admits a pixel-identical
change inside a three-edit cap. All findings are UNBUILT: the orchestrator must run the build and
tests after the batch lands.
