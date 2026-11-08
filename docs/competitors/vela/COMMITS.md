# Vela commit history

Source: `gh api repos/vela-desktop/vela/commits --paginate`, 218 commits, 2026-09-26 to 2026-11-08.
Licence: GPL-3.0. Ideas only; no code copied.

## Subject prefixes

| Prefix | Count |
|---|---|
| fix | 58 |
| ci | 20 |
| feat | 19 |
| chore | 16 |
| docs | 10 |
| test | 7 |
| shell | 6 |
| renderer | 5 |
| security | 4 |
| packaging | 3 |

## Commits worth evaluating for FlameWM

Evaluated critically. Porting means the idea only.

| Commit | Idea | FlameWM relevance |
|---|---|---|
| `cd5ff9f223` fix(compositor): leave a pointer an app locked where it is (#192) | Keep pointer position when an app holds a lock | Medium. Verify wayland pointer constraints. |
| `4a7e007e04` fix(compositor): resize borders only where visible (#191) | Resize hit zones only where visible | Medium. Matches `frame/layout.rs`. |
| `ad1867ddaa` fix(compositor): per-output fullscreen stacking (#158) | Fullscreen per output | Medium. Verify with two outputs. |
| `b3b7e162ac` fix: place dialogs on the correct monitor (#105) | Dialog placement | Medium. Matches `wm/outputs.rs`. |
| `3d54eb2f39` feat(xwayland): native resolution for fullscreen games (#167) | Expose native resolution | Low-medium. Needs XWayland work. |
| `2acc493463` fix(audio): prevent volume wheel stalls on readback (#175) | Avoid stalls on hardware readback | Low. Audio path differs. |
| `9acfcb38d7` fix(renderer): keep queued captures on timer re-arm | Capture queue correctness | Low. FlameWM is not a Vulkan compositor. |
| `8c32356dfd` fix(vulkan): detect device loss | Device-loss handling | None. No Vulkan in FlameWM. |
| `c8d2eb248c` Fix notification placement on primary display (#93) | Notification placement | Low-medium. Not checked. |
| `d9055d58e0` fix(compositor): redraw new Xwayland buffers with empty damage | Damage on new buffers | Low. Not checked. |

## Not relevant

Vela-specific work: the Ubuntu and Fedora packaging, the NVIDIA and Intel GPU CI runners, the updater, Night Light gamma, Caffeine timers, the Vela polkit agent, the Start menu, and the Vela Bluetooth daemon.

## Evidence limits

- These are commit subjects only. Diffs were not read.
- No runtime test was run for any row.
