# FlameWM Architecture Map

Snapshot: 2026-11-08, `main` at `cb5999d1`, working tree. Edges are normal-kind dependencies between workspace crates from `cargo metadata --no-deps`: 57 crates, 243 edges, **no cycles**. Companion documents: [ARCHITECTURE.md](ARCHITECTURE.md) (product boundary and ownership rules), [QUALITY_SCORECARD.md](QUALITY_SCORECARD.md) (per-crate scores), [GPU_BACKENDS.md](GPU_BACKENDS.md).

## Layers

The three tiers below are this review's assignment, based on each crate's role. No layer config exists in the repo. A dependency from a lower tier to a higher one is a violation.

```text
Layer 2  PRODUCTS (user-facing binaries; depend on everything below)
         shell  desktop  settings  terminal  explorer  finder  task-manager  assistant
         imageviewer  screenshot  archiver-app  apps  lock  x11-canary
                 |                          |
                 v                          v
Layer 1  ADAPTERS AND AUTHORITIES (platform services, WM authority, native I/O)
         wm-x11  wm-wayland  x11-desktop  platform  applications  integrations-linux
         control-dbus  render-x11  ui-x11  ui-document  ui  tray  archiver  audio-helper
                 |
                 v
Layer 0  FOUNDATION AND CONTRACTS (pure models, shared vocabulary, event and render cores)
         api  reactor  profiler  debug  skin  clipboard  ribbons  window-core  session-core
         shell-core  desktop-core  settings-core  control-core  control-wire  dbus-reactor
         image-core  integrations-core  ui-core  ui-layout-core  widgets-core  assistant-core
         downloads-core  render-core  render-compiler  conversion-core  weather  xsettings  remote-fs
```

Ownership rules this map must respect, from [AGENTS.md](../AGENTS.md) and [ARCHITECTURE.md](ARCHITECTURE.md):

- **WM authority stays in layer 1.** `wm-x11` and `wm-wayland` do not depend on any product crate. Shell and desktop depend on `ui-*` and `control-*`, never the reverse.
- **Native event sources live in `reactor`.** `reactor` has 9 dependents and no dependencies. `dbus-reactor` is its D-Bus peer, layer 0.
- **Shared product policy is a leaf.** `api` is the most depended-on crate (fan-in 42) and has no workspace dependencies. This matches the rule that shortcut names and the workspace grid are defined once.

## Hubs and fan-out

Fan-in (dependents) shows what a change ripples into. Fan-out shows what a crate pulls in.

| Role | Crates (fan-in / fan-out) |
|---|---|
| Most depended-on | `api` 42/0, `ui-core` 18/3, `control-core` 14/2, `dbus-reactor` 14/2, `control-dbus` 13/5, `image-core` 13/1 |
| Widest fan-out (candidates to narrow) | `desktop` 1/21, `shell` 1/21, `wm-x11` 1/18, `explorer` 1/17, `wm-wayland` 0/13 |
| Leaves (no workspace deps) | `api`, `reactor`, `profiler`, `skin`, `clipboard`, `debug`, `ribbons`, `assistant-core`, `downloads-core` |

Changes to `api`, `ui-core` or `image-core` rebuild most of the workspace. The build-time measurements in [BUILD_TIME_2026-11-05.md](BUILD_TIME_2026-11-05.md) show a `render-core` touch re-checks 21 units (71 s). Keep layer-0 interfaces stable.

## Domains

| Domain | Crates |
|---|---|
| X11 WM and session | `wm-x11`, `x11-desktop`, `wm`, `render-x11`, `render-core`, `render-compiler`, `window-core`, `session-core` |
| Wayland compositor | `wm-wayland` (Smithay 0.7, GLES renderer, logind and DRM/KMS) |
| Shell and desktop UI | `shell`, `shell-core`, `desktop`, `desktop-core`, `ui`, `ui-x11`, `ui-core`, `ui-layout-core`, `widgets-core`, `ui-document`, `skin`, `tray` |
| Settings and control | `settings`, `settings-core`, `control-core`, `control-wire`, `control-dbus`, `xsettings` |
| Applications | `terminal`, `explorer`, `finder`, `task-manager`, `assistant`, `assistant-core`, `imageviewer`, `screenshot`, `archiver`, `archiver-app`, `apps`, `applications`, `lock` |
| Platform services and Linux integration | `platform`, `integrations-linux`, `integrations-core`, `audio-helper`, `clipboard`, `downloads-core`, `remote-fs`, `weather`, `conversion-core`, `image-core` |
| Event and diagnostics cores | `reactor`, `dbus-reactor`, `profiler`, `debug`, `ribbons` |
| Test tooling | `x11-canary` (10k production LOC, see [QUALITY_SCORECARD.md](QUALITY_SCORECARD.md)) |

## Layer violations

Four edges point from a declared pure-core crate to a layer-1 service. Each breaks the "core is pure" claim in its crate's name.

| Edge | Problem | Fix (size) |
|---|---|---|
| `shell-core` -> `platform` | Pure presentation model imports service facade | Move the shared type down to `api`, or pass it in as data (S) |
| `control-core` -> `platform` | Pure control model imports service facade | Same as above (S) |
| `desktop-core` -> `applications` | Pure model imports app catalog | Pass the catalog entries in as plain values (S) |
| `settings-core` -> `applications` | Pure settings model imports app catalog | Same as above (S) |

Cycles: none. The graph is a DAG, so the layer order above is achievable without breaking a cycle.

## Full crate table

Layer is the tier above. Fan-in and fan-out count workspace edges; LOC counts production code, with test LOC separate. `Depends on` lists direct workspace dependencies without the `flamewm-` prefix.

| Crate | Layer | Fan-in | Fan-out | Files | Prod LOC | Test LOC | Depends on |
|---|---|---|---|---|---|---|---|
| flamewm-api | 0 | 42 | 0 | 32 | 2733 | 820 | - |
| flamewm-assistant-core | 0 | 1 | 0 | 11 | 2304 | 1498 | - |
| flamewm-clipboard | 0 | 5 | 0 | 5 | 825 | 299 | - |
| flamewm-control-core | 0 | 14 | 2 | 3 | 769 | 797 | api, platform |
| flamewm-control-wire | 0 | 6 | 2 | 11 | 2853 | 1124 | api, control-core |
| flamewm-conversion-core | 0 | 2 | 1 | 2 | 1016 | 319 | desktop-core |
| flamewm-dbus-reactor | 0 | 14 | 2 | 1 | 170 | 21 | api, integrations-core |
| flamewm-debug | 0 | 4 | 0 | 1 | 206 | 40 | - |
| flamewm-desktop-core | 0 | 5 | 3 | 13 | 2236 | 834 | api, applications, ui-core |
| flamewm-downloads-core | 0 | 1 | 0 | 1 | 305 | 109 | - |
| flamewm-image-core | 0 | 13 | 1 | 14 | 2286 | 2044 | api |
| flamewm-integrations-core | 0 | 2 | 1 | 6 | 765 | 342 | api |
| flamewm-profiler | 0 | 7 | 0 | 8 | 1039 | 605 | - |
| flamewm-reactor | 0 | 9 | 0 | 2 | 655 | 278 | - |
| flamewm-remote-fs | 0 | 1 | 1 | 12 | 1780 | 718 | api |
| flamewm-render-compiler | 0 | 0 | 3 | 15 | 2504 | 1311 | api, image-core, render-core |
| flamewm-render-core | 0 | 8 | 1 | 25 | 5499 | 4968 | image-core |
| flamewm-ribbons | 0 | 1 | 0 | 14 | 1230 | 1036 | - |
| flamewm-session-core | 0 | 6 | 1 | 5 | 1408 | 1131 | api |
| flamewm-settings-core | 0 | 2 | 4 | 5 | 1215 | 1255 | api, applications, control-core, ui-core |
| flamewm-shell-core | 0 | 2 | 3 | 14 | 2549 | 1871 | api, platform, ui-core |
| flamewm-skin | 0 | 6 | 0 | 19 | 1439 | 613 | - |
| flamewm-ui-core | 0 | 18 | 3 | 14 | 2281 | 1796 | api, skin, ui-layout-core |
| flamewm-ui-layout-core | 0 | 1 | 1 | 1 | 394 | 62 | api |
| flamewm-weather | 0 | 2 | 1 | 6 | 694 | 403 | api |
| flamewm-widgets-core | 0 | 1 | 2 | 6 | 2025 | 784 | api, desktop-core |
| flamewm-window-core | 0 | 2 | 1 | 5 | 506 | 585 | api |
| flamewm-xsettings | 0 | 1 | 1 | 4 | 705 | 764 | api |
| flamewm-applications | 1 | 9 | 2 | 6 | 1765 | 884 | api, session-core |
| flamewm-archiver | 1 | 1 | 0 | 18 | 1514 | 1333 | - |
| flamewm-audio-helper | 1 | 0 | 2 | 2 | 155 | 236 | api, integrations-linux |
| flamewm-control-dbus | 1 | 13 | 5 | 2 | 678 | 746 | api, control-core, control-wire, dbus-reactor, platform |
| flamewm-integrations-linux | 1 | 7 | 6 | 44 | 13901 | 6861 | api, dbus-reactor, debug, image-core, integrations-core, profiler |
| flamewm-platform | 1 | 6 | 1 | 52 | 7678 | 5417 | api |
| flamewm-render-x11 | 1 | 2 | 5 | 93 | 24099 | 11215 | api, image-core, profiler, reactor, render-core |
| flamewm-tray | 1 | 1 | 2 | 6 | 1077 | 540 | api, dbus-reactor |
| flamewm-ui | 1 | 1 | 1 | 30 | 442 | 21 | ui-core |
| flamewm-ui-document | 1 | 6 | 5 | 5 | 878 | 789 | api, image-core, render-core, skin, ui-core |
| flamewm-ui-x11 | 1 | 11 | 7 | 9 | 1876 | 577 | api, reactor, render-core, render-x11, ui, ui-core, ui-document |
| flamewm-wm | 1 | 1 | 7 | 10 | 2314 | 825 | api, control-core, control-dbus, dbus-reactor, session-core, wm-x11, xsettings |
| flamewm-wm-wayland | 1 | 0 | 13 | 67 | 15654 | 5749 | api, applications, control-dbus, control-wire, image-core, integrations-linux, platform, reactor, render-core, session-core, skin, ui-core, window-core |
| flamewm-wm-x11 | 1 | 1 | 18 | 93 | 20318 | 10722 | api, applications, clipboard, control-dbus, control-wire, debug, image-core, integrations-linux, platform, profiler, reactor, render-core, render-x11, shell-core, skin, ui-core, window-core, x11-desktop |
| flamewm-x11-desktop | 1 | 1 | 2 | 8 | 2245 | 943 | api, applications |
| flamewm-apps | 2 | 0 | 8 | 1 | 73 | 0 | desktop, explorer, finder, settings, shell, task-manager, terminal, wm |
| flamewm-archiver-app | 2 | 0 | 4 | 4 | 657 | 261 | api, archiver, ui-core, ui-x11 |
| flamewm-assistant | 2 | 0 | 9 | 13 | 2686 | 1338 | api, assistant-core, control-core, control-dbus, dbus-reactor, reactor, render-core, ui-core, ui-x11 |
| flamewm-desktop | 2 | 1 | 21 | 45 | 10018 | 2681 | api, applications, clipboard, control-core, control-dbus, control-wire, conversion-core, dbus-reactor, debug, desktop-core, downloads-core, image-core, integrations-linux, profiler, reactor, session-core, ui-core, ui-document, ui-x11, weather, widgets-core |
| flamewm-explorer | 2 | 1 | 17 | 52 | 8770 | 4484 | api, applications, clipboard, control-core, control-dbus, conversion-core, dbus-reactor, desktop-core, image-core, integrations-linux, profiler, remote-fs, settings-core, skin, ui-core, ui-document, ui-x11 |
| flamewm-finder | 2 | 1 | 8 | 18 | 3408 | 977 | api, applications, control-core, control-dbus, dbus-reactor, reactor, ui-core, ui-x11 |
| flamewm-imageviewer | 2 | 0 | 7 | 5 | 1007 | 390 | api, control-core, control-dbus, dbus-reactor, image-core, ui-core, ui-x11 |
| flamewm-lock | 2 | 0 | 10 | 33 | 5491 | 2808 | api, control-core, control-dbus, dbus-reactor, image-core, reactor, render-core, ribbons, ui-document, ui-x11 |
| flamewm-screenshot | 2 | 0 | 2 | 6 | 2120 | 702 | api, clipboard |
| flamewm-settings | 2 | 1 | 10 | 61 | 7842 | 5406 | api, control-core, control-dbus, control-wire, dbus-reactor, integrations-linux, session-core, settings-core, ui-core, ui-x11 |
| flamewm-shell | 2 | 1 | 21 | 142 | 21749 | 13273 | api, applications, control-core, control-dbus, control-wire, dbus-reactor, debug, desktop-core, image-core, integrations-linux, platform, profiler, reactor, session-core, shell-core, skin, tray, ui-core, ui-document, ui-x11, weather |
| flamewm-task-manager | 2 | 1 | 7 | 6 | 1694 | 407 | api, control-core, control-dbus, dbus-reactor, ui-core, ui-document, ui-x11 |
| flamewm-terminal | 2 | 1 | 9 | 40 | 8163 | 4176 | api, clipboard, control-core, control-dbus, dbus-reactor, image-core, profiler, ui-core, ui-x11 |
| flamewm-x11-canary | 2 | 0 | 0 | 27 | 10121 | 157 | - |
