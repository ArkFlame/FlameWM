# Unit p93-strace-bold-font
STATUS: DONE (static read; strace not run, no runtime capture)

p90 found no Bold face opened by fontdue paths. Find which process opens IBMPlexSans-Bold at startup. Read `scripts/` for an existing strace wrapper; check rg for "IBMPlexSans-Bold" and "Bold" across crates/flamewm-shell, flamewm-desktop, flamewm-ui-x11, flamewm-render-core; list every call site that loads a bold face at startup versus on first use. Propose (do not apply) lazy loading where the load is eager. No builds. Write rows file:line, eager/lazy, size.

## Result

Method: rg over crates and scripts (no strace wrapper exists in scripts/; `strace` is on PATH but was not run, so the rows below are code-derived, not observed opens).

Mechanism: X11 Xft opens a face lazily per (pixelsize, weight) key in `crates/flamewm-render-x11/src/xft/backend_measure.rs:55-95` (`font_for`). `normalize_weight` (`xft/fonts.rs:54-61`) maps 551..=650 to semibold and anything above 650 to `bold`, so only a weight of 651+ opens `IBMPlexSans-Bold`. Wayland fontdue path (`render-core/src/glyphs.rs:14-20`, `wm-wayland/src/font.rs:13`) lists Regular only, so p90's finding holds.

Rows (file:line | load | when | size):
- `crates/flamewm-widgets-core/src/widgets/kind.rs:124` `bold` default true for Clock | EAGER | desktop startup; `widget_grid.rs:526` places Clock in `default_layout` | size unmeasured (no IBMPlexSans files on this machine)
- `crates/flamewm-desktop/src/widgets/sync.rs:79` clock weight = 700 when bold | EAGER (consumer of the row above) | every clock repaint at startup | unmeasured
- `crates/flamewm-skin/src/typography.rs:55-58` `sticky_title` Bold | NONE (no consumer found outside the definition) | never | n/a
- `crates/flamewm-shell/src/async_projection/start_view.rs:130` header letters weight 700 | LAZY | first start-menu open | unmeasured
- `crates/flamewm-desktop/src/sticky.rs:450` sticky note bold runs 700 | LAZY | note with bold text shown | unmeasured
- `crates/flamewm-desktop/src/widgets/sync.rs:619` widget span bold 700 | LAZY | widget text span bold | unmeasured
- `crates/flamewm-terminal/src/ui.rs:290` terminal bold run 700 | LAZY | terminal output with bold | unmeasured (terminal is a separate process)
- `crates/flamewm-wm-x11/src/frame/chrome.rs:21,231` title weight 600 | semibold face, not Bold | startup | n/a for Bold
- `crates/flamewm-render-core/src/model/style/runtime.rs:365,401` ui_font_bold forces weight >= 600 | semibold, not Bold | only when fontBold setting is on | n/a

Proposal (not applied): the eager load is the Clock default `bold=true` (`kind.rs:124`). Options in order of effect: (1) make the clock default `bold=false` (weight 400, no Bold face opened at startup; a user-visible default change, needs a decision); (2) keep the default and accept one Bold open at first clock paint, with the cost paid once per process since `font_for` caches the handle. Lazy-loading the Bold face itself would not help here because the clock paints at startup. The remaining Bold consumers already open lazily on first use.

Size: not measured. `fd` finds no IBMPlexSans files in the repo or the system font dirs, and `scripts/prepare-ibm-plex` only extracts a user-supplied archive. Measure with `stat -c '%s'` on the Bold face once it is available.

Next unit: run `strace -f -e trace=openat -p <flamewm-desktop pid>` with `flock /run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender/../capture.lock` only if a runtime session is allowed, to confirm the clock open. Owner decision needed on Clock `bold` default.
