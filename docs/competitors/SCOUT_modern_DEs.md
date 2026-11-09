# Scout: modern DE reference (non-effect UI/UX, reliability, perf)

Scope: `.vendor/cosmic-epoch`, `.vendor/creamui-main`, `.vendor/coconut-main`, `.vendor/nox-main` (read-only).
Excluded by user constraint: glass, blur, shadow, opacity and other effect additions.
Method: grep and short excerpts only; no builds, no runtime runs. Items marked UNVERIFIED were not confirmed in FlameWM source.

Notes on the vendor set:
- `cosmic-epoch`: `cosmic-settings/src` is not at that path in this snapshot. Libcosmic theme tokens are not vendored, so no cosmic spacing/radius constants were found. Settings keyboard flow was not examined.
- `creamui-main`: the theme crate (`crates/theme/src/lib.rs`) is the richest source of token and accessibility ideas.
- `coconut-main`: island and dock model, notification API. Mostly a plugin architecture reference.
- `nox-main`: Spanish-language Windows voice assistant. Only README and screenshots. No transferable UI system. REJECT all.
- Licence: `LICENSE` files were not read in this scout. Before copying any code, check each vendor licence. Treat every ADAPT row as "re-derive the spec, do not copy code".

FlameWM anchors: `crates/flamewm-skin/src/{metrics,typography}.rs`, `crates/flamewm-skin/src/recipes/*.rs`, `crates/flamewm-integrations-linux/src/notifications.rs`, `crates/flamewm-wm-x11/src/runtime.rs`.

| # | Idea | Source (vendor path:line) | FlameWM today (path:line) | Gap | Effort | Perf/mem impact | Licence risk | Verdict + reason |
|---|---|---|---|---|---|---|---|---|
| 1 | Named radius/spacing/control token set | creamui-main/crates/theme/src/lib.rs:261-292 (radius 8/12/20, spacing 4/8/16, per-control radii) | flamewm-skin/src/recipes/menu.rs:21 radius 12 literal; decoration.rs:8,118 radius 12 literal; no spacing scale found in skin/metrics.rs | Radii and spacing are literals per recipe, so they drift | M | none | check LICENSE (creamui) | ADAPT: one token table in skin, recipes reference names |
| 2 | Typed text-role scale | creamui-main/crates/theme/src/lib.rs:199 `Typography` struct | flamewm-skin/src/typography.rs:33-61 sizes 13/12/12/21/15/13 | Six sizes with no ratio; 12 and 13 near-duplicate | S | none | low (spec only) | ADAPT: collapse to 4 roles (caption, body, title, display) |
| 3 | Corner-style factor applied to all radii | creamui-main/crates/theme/src/lib.rs:315-330 `with_corners` scales every radius | flamewm-skin/src/recipes/decoration.rs:91-95 `corner_radius(floating, style)` | Only window frames use the style knob, not menus/controls | S | none | check LICENSE | ADAPT: route menu/popup radius through the same style factor |
| 4 | Per-urgency notification timeout caps | cosmic-epoch/cosmic-notifications/src/app.rs:283-286 (`max_timeout_urgent`, default 3000 ms) | flamewm-integrations-linux/src/notifications.rs:27 single `DEFAULT_TIMEOUT` 6 s; :804-813 spec mapping | No urgency-based cap; critical toasts and info toasts share one timer | S | none | GPL-3.0 likely (verify) | ADAPT: urgency-aware default and cap, spec behaviour kept |
| 5 | Bounded D-Bus calls around notification server | cosmic-epoch/cosmic-notifications/src/subscriptions/notifications.rs:384-398 (100 ms and 500 ms `tokio::time::timeout`) | flamewm-integrations-linux/src/portal_file_chooser.rs:28 only 300 s timeout seen; notification path timeouts UNVERIFIED | A hung client can stall notification handling | S | low; avoids blocking | check LICENSE | ADAPT: wrap client-facing D-Bus replies with short deadlines |
| 6 | OSD duration split (transient vs. persistent) | cosmic-epoch/cosmic-osd/src/components/osd_indicator.rs:149 (3 s), :165 (1 s) | flamewm-wm-x11/src/runtime.rs:802-1001 has no OSD duration; OSD owner UNVERIFIED | One duration cannot fit volume, brightness and display-number OSDs | S | none | GPL-3.0 likely | ADAPT: 1 s for value OSDs, 3 s for identifiers |
| 7 | OSD placement by content kind | cosmic-epoch/cosmic-osd/src/components/osd_indicator.rs:195-213, :266-269 (top margin 0 for display number, bottom 48 for others) | UNVERIFIED (OSD crate not located in this scout) | Placement is not separated by OSD kind | S | none | GPL-3.0 likely | ADAPT: same split as row 6 |
| 8 | Notification expiry deadline, not polling | coconut-main/crates/api/src/notifications.rs (model, not read in full) | flamewm-integrations-linux/src/notifications.rs:121 `expires_at`, :311 `expire(now)`, :134 `next_deadline` | Already present | - | already has | - | REJECT: duplicate of existing deadline model |
| 9 | Spec `expire_timeout` -1/0 semantics | cosmic-epoch/cosmic-notifications/src/subscriptions/notifications.rs:336-339 | flamewm-integrations-linux/src/notifications.rs:804 | Already present | - | - | - | REJECT: duplicate |
| 10 | Island grouping for dock sections | coconut-main/crates/plugin-kit/src/island.rs; crates/core/src/dock.rs | flamewm-shell-core/src/dock.rs (section model not read) | Visual group separation in dock | M | none | check LICENSE | ADAPT: group with spacing and a divider, no fill |
| 11 | Island chrome with opacity | coconut-main/crates/plugin-kit/src/chrome.rs `apply_opacity` | n/a | Effect, excluded | - | - | - | REJECT: effect, violates constraint |
| 12 | Panel autohide wait and transition as config | cosmic-epoch/cosmic-panel/cosmic-panel-config/src/panel_config.rs:632-638 | UNVERIFIED (taskbar autohide not checked) | Autohide tuning | S | none | GPL-3.0 likely | ADAPT only if FlameWM has autohide; else REJECT |
| 13 | Panel slide animation 300 ms | cosmic-epoch/cosmic-panel/cosmic-panel-bin/src/space/panel_space.rs:1577 | n/a | Motion, not requested | - | - | - | REJECT: motion is effect-adjacent, outside requested scope |
| 14 | Frame-timing harness | creamui-main/crates/bench/src/metrics.rs:7 `measure()`; bench/examples/animation_pacing.rs | flamewm-profiler (crate exists per routes); performance-proof route | Ad-hoc timing vs. repeatable metric | M | measurement only | check LICENSE | ADAPT: reuse the shape of `FrameMetrics` in flamewm-profiler |
| 15 | Validated accessibility prefs (text scale 0.75-2.0, high contrast) | creamui-main/crates/theme-loader/src/accessibility.rs:15-34 (validate), :39-51 (idempotent apply) | UNVERIFIED (grep for text_scale not run in FlameWM) | No validated text-scale knob found in this scout | M | none | check LICENSE | ADAPT: validate range and make apply idempotent; add tests |
| 16 | Live theme change listener | creamui-main/crates/theme-watch/src/lib.rs:52-91 `ThemeListener` | flamewm-desktop/src/settings_sync.rs, flamewm-xsettings (UNVERIFIED for live reload) | Settings change propagation not confirmed | M | small wakeups only | check LICENSE | ADAPT: only if settings_sync lacks push; verify first |
| 17 | Keyboard focus rules for settings | cosmic-epoch cosmic-settings (path absent) | flamewm-skin/src/recipes/settings.rs (grep returned no focus rule) | No evidence | - | - | - | REJECT: no vendor evidence in this snapshot |
| 18 | Launcher debounce | cosmic-epoch/cosmic-launcher (grep found no `Duration::from_millis`) | n/a | No evidence | - | - | - | REJECT: no evidence |
| 19 | Dock active-window feedback | coconut-main README "centered running-app dockbar with active-window feedback" | flamewm-skin/src/recipes/taskbar.rs (UNVERIFIED) | Active state not checked | S | none | check LICENSE | UNVERIFIED: check taskbar active state before deciding |
| 20 | Pointer-state precedence (pressed > hover) | none in vendor | flamewm-skin/src/recipes/window_control.rs:44-49 `from_pointer` already | Already present | - | - | - | REJECT: FlameWM already has it |
| 21 | Heap trim after startup | none in vendor | flamewm-wm-x11/src/runtime.rs:998 `STARTUP_HEAP_TRIM_DELAY` 10 s | Already present | - | - | - | REJECT: already present |
| 22 | Visual dev notes for design tokens | coconut-main/DEVELOPMENT.md, CODESTYLE.md (no token or radius text found) | n/a | No evidence | - | - | - | REJECT: no evidence |
| 23 | Voice assistant UI | nox-main/README.md | n/a | Off-scope product | - | - | - | REJECT: unrelated product |
| 24 | Speech status overlay | nox-main/docs/panel.png (image, not read) | n/a | Off-scope | - | - | - | REJECT: unrelated product |
| 25 | Rounded-corner style for popups | creamui-main/crates/theme/src/lib.rs:291 menu radius 10 | flamewm-skin/src/recipes/menu.rs:21 radius 12 | Menu radius differs from token | S | none | check LICENSE | ADAPT: fold into row 3 |

## Top 5 by user-visible win / effort

1. **Named type scale and spacing/radius token set** (rows 1, 2). Effort M. Source: creamui theme token tables.
   Implementation: add a `Radius` and `Space` token table and a four-role `TextStyle` set in `crates/flamewm-skin/src/typography.rs` and `crates/flamewm-skin/src/metrics.rs`. Replace literals in `crates/flamewm-skin/src/recipes/menu.rs:21`, `recipes/decoration.rs:8`, and `recipes/taskbar.rs`. Add a unit test in `flamewm-skin` that asserts each recipe reads from the token table. Do not change pixel output until the tests pass; keep current values as the initial token values.

2. **Per-urgency notification timeouts** (row 4). Effort S. Source: cosmic-notifications `app.rs:283-286`.
   Implementation: in `crates/flamewm-integrations-linux/src/notifications.rs`, replace the single `DEFAULT_TIMEOUT` (line 27) with a `fn default_timeout(urgency: u8) -> Duration` used at lines 813 and 1048. Low urgency keeps 6 s, normal 6 s, critical gets a longer cap. Extend the existing test `expire_timeout_follows_urgency_policy` (line 1210) to cover the new mapping. Spec `-1` and `0` handling at line 804 stays unchanged.

3. **OSD duration and placement split** (rows 6, 7). Effort S. Source: cosmic-osd `osd_indicator.rs:149,165,266-269`.
   Implementation: locate the OSD owner first (not found in this scout, see row 6 UNVERIFIED). Add two constants next to it: value OSDs use 1 s, identifier OSDs use 3 s. Place identifier OSDs top-centre and value OSDs bottom-centre with a fixed offset. Add a test for the duration selection. Runtime check required before claiming the placement works.

4. **Bounded D-Bus calls in the notification path** (row 5). Effort S. Source: cosmic-notifications `notifications.rs:384-398`.
   Implementation: in `crates/flamewm-integrations-linux/src/notifications.rs`, wrap each client-facing reply with a short deadline (100 ms for object lookup, 500 ms for signal emit), following the pattern at `portal_file_chooser.rs:42` `connect_with_timeout`. Add a test with a stalled fake bus. Runtime D-Bus behaviour is UNVERIFIED until run in a real session.

5. **One corner-style knob for all radii** (rows 3, 25). Effort S. Source: creamui `with_corners` at `theme/src/lib.rs:315-330`.
   Implementation: extend `corner_radius` in `crates/flamewm-skin/src/recipes/decoration.rs:95` into a shared `radius_for(role, style)` helper in `flamewm-skin`, and route `recipes/menu.rs:21` through it. Keep `DecorationStyle::Squared` returning 0 so the existing tests at `decoration.rs:112-124` still pass.

## Gaps not closed by this scout

- UNVERIFIED: FlameWM OSD owner, taskbar active state, notification D-Bus timeouts, live theme reload, and text-scale knobs. Each needs a targeted grep before any row is marked ADOPT.
- Vendor licences were not read.
- No builds or runtime checks were run, so no row is a runtime claim.

Risk: rows 1 and 3 touch shared token values; a wrong default changes every window. Verify with a pixel or screenshot comparison before merge.
