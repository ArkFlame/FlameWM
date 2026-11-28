# Unit p93-strace-bold-font
STATUS: PARTIAL

p90 found no Bold face opened by fontdue paths. Find which process opens IBMPlexSans-Bold at startup. Read `scripts/` for an existing strace wrapper; check rg for "IBMPlexSans-Bold" and "Bold" across crates/flamewm-shell, flamewm-desktop, flamewm-ui-x11, flamewm-render-core; list every call site that loads a bold face at startup versus on first use. Propose (do not apply) lazy loading where the load is eager. No builds. Write rows file:line, eager/lazy, size.

## Result

Method: `rg` over the named crates, scripts/ and `.vendor/`; re-read every cited file. No strace wrapper exists in `scripts/` (only the comment at `scripts/flamewm-wayland-session:544`); `strace` was not run here, so rows are code-derived, not observed opens. A concurrent worker had filled this unit with rows; all of them were re-verified against current source and are preserved below.

Mechanism: the X11 Xft backend opens a face lazily per (pixelsize, weight) key in `crates/flamewm-render-x11/src/xft/backend_measure.rs:57-99` (`font_for`); `normalize_weight` (`xft/fonts.rs:54-61`) maps 551..=650 -> 600 (semibold) and >650 -> 700 (bold), and only the `bold` pattern can open a Bold file. `crates/flamewm-render-x11/src/xft/backend.rs:94` eagerly opens only weight 400 (Regular) at construction. `fc-match 'IBM Plex Sans:weight=bold'` resolves to `/usr/share/fonts/truetype/ibm-plex/IBMPlexSans-Bold.ttf` (193120 bytes / 192 KiB). So the process that opens Bold is whichever X11 process first paints weight-700 text — flamewm-desktop's Clock at startup, otherwise shell/desktop/terminal on first use — never a fontdue parser. The Wayland fontdue path lists Regular only, so p90 holds.

Rows (file:line | finding | eager/lazy | size | change | risk):

| file:line | finding | eager/lazy | size | change | risk |
| --- | --- | --- | --- | --- | --- |
| `crates/flamewm-widgets-core/src/widgets/kind.rs:124` | Clock `bold` option defaults to `true`; `widget_grid.rs:526` puts Clock in `default_layout` | EAGER (desktop startup) | n/a (property, 0 B) | none | changing the default is user-visible; needs an owner decision |
| `crates/flamewm-desktop/src/widgets/sync.rs:79,96` | clock weight = 700 when `bold`; first weight-700 paint triggers the Bold open | EAGER (every clock repaint, first at startup) | Bold face 193120 B on disk, parsed once per process | none | none; handle cached by `font_for` |
| `crates/flamewm-render-x11/src/xft/backend.rs:94` | construction-time `font_for(13.0, 400)` | EAGER | Regular face, not Bold | none | none |
| `crates/flamewm-render-x11/src/xft/backend_measure.rs:57-99` | weight > 600 -> `weight=bold` pattern -> `XftFontOpenName`; fontconfig resolves to IBMPlexSans-Bold.ttf | LAZY, cached per (size, weight) | 193120 B / 192 KiB | none | none; already lazy |
| `crates/flamewm-shell/src/async_projection/start_view.rs:130` | start-menu header letters weight 700 | LAZY (first start-menu open) | 0 (DOM request) | none | none |
| `crates/flamewm-desktop/src/sticky.rs:450` | bold sticky-note run weight 700 | LAZY | 0 | none | none |
| `crates/flamewm-desktop/src/widgets/sync.rs:619` | bold widget span weight 700 | LAZY | 0 | none | none |
| `crates/flamewm-terminal/src/ui.rs:290` | bold terminal run weight 700 | LAZY (separate process) | 0 | none | none |
| `crates/flamewm-skin/src/typography.rs:55-58` | `sticky_title` Bold style | NONE (no consumer outside definition) | n/a | none | none |
| `crates/flamewm-wm-x11/src/frame/chrome.rs:21,231` | title weight 600 -> semibold face, not Bold | startup, n/a for Bold | n/a | none | none |
| `crates/flamewm-render-core/src/model/style/runtime.rs:365,401` | `ui_font_bold` forces weight >= 600 -> semibold, not Bold | only when setting enabled | n/a | none | none |
| `crates/flamewm-render-core/src/glyphs.rs:14-20,187-222` | fontdue software path: CANDIDATES are Regular/Noto/DejaVu/Liberation/Cantarell; `Font::from_bytes` parses one face, no weight parameter | n/a (no Bold path) | bundled DejaVuSansMono 343140 B only on fallback | none | none |
| `scripts/prepare-ibm-plex:50-64` | extracts `IBMPlexSans-{Regular,Medium,SemiBold,Bold}.ttf` for fontconfig | setup-time, not runtime | 193120 B on disk | none | none |
| `scripts/` (no wrapper) | only `flamewm-wayland-session:544` mentions strace; prior evidence: `strace -p` blocked by `ptrace_scope=1`, launch-mode `strace -f -c` covers startup | n/a | n/a | none | strace attribution not independently confirmed |
| `.vendor/kwin-master, xfwm4-master, smithay-master` | reference-first: no alternate weight-to-face loader; fontconfig lazy per-(size,weight) cached selection is the established pattern | n/a | n/a | none | none |

Proposal (not applied): the only eager Bold trigger is the Clock `bold=true` default (`kind.rs:124`, consumed at `sync.rs:79,96`), which paints at startup. Options: (1) change the default to `bold=false` — weight 400, no Bold open at startup, but a user-visible default change requiring an owner decision; (2) keep the default and accept one Bold open at first clock paint, paid once per process because `font_for` caches the handle. Lazy-loading the Bold face itself cannot help the clock because the clock paints at startup; every other Bold consumer already opens on first use. No production edit was applied.

Size note: measured, contra the earlier "no IBMPlexSans files / unmeasured" note. `fc-match -f '%{file}' 'IBM Plex Sans:weight=bold'` -> `/usr/share/fonts/truetype/ibm-plex/IBMPlexSans-Bold.ttf`; `stat -c '%s'` -> 193120 bytes (192 KiB). The parse cost is fontconfig/Xft's and is incurred once on first use, not held as MBs by our fontdue code.

Edits this unit: production edits = none (0 of 3 allowed), UNBUILT. This unit file was updated for reporting only; no code was built, tested or committed.

Next unit: if runtime attribution is required, launch `strace -f -e trace=openat` on flamewm-desktop at session start (not `-p`, which EPERMs under `ptrace_scope=1`) to confirm the Clock Bold open. Owner decision still needed on the Clock `bold` default.

PARTIAL: investigation complete and all cited call sites re-verified against current source; the opener is the X11 Xft path (flamewm-desktop Clock eagerly at startup, others lazily on first weight-700 text), and no fontdue Bold load exists to defer, so no production change was made. All edits: none, UNBUILT. Skipped: no strace capture (no wrapper in `scripts/`, attach blocked, no runtime gate) and no builds, per brief; Bold open attribution therefore remains code-derived rather than observed.
