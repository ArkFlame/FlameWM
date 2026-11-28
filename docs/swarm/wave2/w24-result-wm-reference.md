# w24: reference — second WM, startup-scan window death, missing extensions

## Answer

FlameWM already classifies all three startup hazards (`crates/flamewm-wm-x11/src/wm/construct.rs:22-149`,
`startup_failure.rs:38-58`), but one case was still fatal: a client destroyed between `QueryTree` and
the `_NET_WM_WINDOW_TYPE` query during the adoption scan aborted startup. xfwm4 prevents that race by
grabbing the server around the whole scan; icewm swallows `BadWindow` from `GetWindowAttributes`.
This audit ports the icewm behavior with one minimal edit at `wm/manage.rs:24-31`; no visual change,
no new dependency, no gate change. All other reference deltas below are **specs only**, not ported.

## Active source evidence

| file:line | finding |
| --- | --- |
| `crates/flamewm-wm-x11/src/wm/event_loop.rs:54-59,71-87` | Connect and `become_wm` are tagged `connect`/`become-wm`; `Wm::new` and `scan_existing` are also tagged `become-wm`/`scan`, so a fatal scan error stops startup with a classified line. |
| `crates/flamewm-wm-x11/src/wm/construct.rs:209-218,271-281` | Second-WM detection is an `Access` error on the root `change_window_attributes` (substructure-redirect) request; `second_wm_fault` turns it into a fatal, operator-readable record. No `WM_S<n>` selection query or ownership verification exists. |
| `crates/flamewm-wm-x11/src/wm/construct.rs:22-29,62-88,101-111,258-269` | Every optional extension is queried independently; absent -> one blast-radius record, failed query -> `extension-query`/Unknown, never a false "absent". The report is rendered once at `:278-280`. |
| `crates/flamewm-wm-x11/src/wm/construct.rs:113-140` | RandR absence/selection failure degrades to root geometry with a record; a failed RandR query is distinct from absence. |
| `crates/flamewm-wm-x11/src/wm/construct.rs:142-149` | `second_wm_fault` emits "another window manager is already running ... exits 1" for `ErrorKind::Access` only. |
| `crates/flamewm-wm-x11/src/wm/manage.rs:4-56` | `scan_existing`: `QueryTree`, then cookies for attributes/geometry; reply failures at `:19-22` are skipped. Before this edit, `window_kind(...)?` at the former `:24` propagated a `BadWindow` from `GetProperty` and killed startup. |
| `crates/flamewm-wm-x11/src/wm/manage.rs:24-31` | **Edited**: the type-query error goes through `ignore_destroyed_window_race`; a destroyed-race error skips that window, any other error still fails the scan. |
| `crates/flamewm-wm-x11/src/wm/manage.rs:48-52` | Map/reparent failures are already filtered by `is_destroyed_window_race` after the outcome, so the new pre-outcome guard closes the only unguarded request in the scan. |
| `crates/flamewm-wm-x11/src/wm/mod.rs:42-73` | `is_destroyed_window_kind` covers `Window|Drawable|Pixmap|Match`; `is_destroyed_window_race` handles `ReplyOrIdError`; `ignore_destroyed_window_race` handles `ReplyError` and logs once. |
| `crates/flamewm-wm-x11/src/wm/events.rs:8-19` | The same race filter is applied per event, so a post-startup death cannot end the session either. |
| `crates/flamewm-wm-x11/src/wm/tests.rs:45-78` | Existing unit test constructs `GetProperty` `X11Error`s and asserts both helpers accept `Window|Drawable|Match` and reject `Value`; it covers the helper reused by the new call site without a new test. |
| `crates/flamewm-wm-x11/src/wm/startup_fault_tests.rs:71-86,223-230,249-260` | Tests pin the fatal second-WM record, the scan race predicate, and the Unknown-vs-absent extension distinction. |

## Reference evidence (read only, `.vendor/**`)

| path | finding |
| --- | --- |
| `.vendor/xfwm4-master/src/screen.c:56-68,90-122` | Builds `WM_S%d`, calls `XGetSelectionOwner`, and refuses to start if an owner exists: prints the running WM's name and a `--replace` hint (`:101-102`); optional replace path installs a `StructureNotify` mask on the old owner under an error trap (`:107-113`) and sets its own selection at `:116`. |
| `.vendor/xfwm4-master/src/screen.c:124-153` | After replacing, waits up to 15 s for the old owner's `DestroyNotify` before adopting windows; timeout is a fatal warning. |
| `.vendor/xfwm4-master/src/hints.c:1236` | Re-reads `XGetSelectionOwner` to confirm the manager atom is still owned. |
| `.vendor/xfwm4-master/src/client.c:2178-2204` | Startup adoption does `XSync` + `grab_server` around `XQueryTree`/`XGetWindowAttributes`/`clientFrame`, then ungrabs and syncs — a structural prevention of the query/act race. |
| `.vendor/xfwm4-master/src/display.c:218-324` | Queries SHAPE, SYNC, RENDER, RANDR and XRes independently; each absence is a `g_warning`, each success records event/error bases, and SYNC additionally checks a **minimum version** (`:248-258`) instead of mere presence. |
| `.vendor/icewm-master/src/wmapp.cc:79-117` | `registerProtocols1`: `XGetSelectionOwner(WM_S<n>)`, `die(1, "A window manager is already running, use --replace ...")` at `:91-92`, creates an override-redirect owner window, sets the selection at `:104`, re-reads it at `:106-107` to fail closed if ownership was lost, then waits for the old owner's `DestroyNotify` (`:109-117`). |
| `.vendor/icewm-master/src/wmapp.cc:756-762,764-781` | During initialization a `BadAccess` on `ChangeWindowAttributes` is another WM -> exit 1. `BadWindow` marks a known window destroyed; crucially, `GetWindowAttributes` errors are swallowed at `:779-781` so a scan race is not fatal. |
| `.vendor/icewm-master/src/wmmgr.cc:1218-1328` | `manageClients` calls `grabServer` (`:1229`) before reading root children (`:1274`), adopts each via `manageClient` (`:1309`), deletes a client that failed to get a frame (`:1310-1312`), and ungrabs at `:1326`. |
| `.vendor/icewm-master/src/wmapp.cc:1334,1412-1413` | XKB is queried with `XkbQueryExtension`; `--extensions` reports extension state on demand. |
| `.vendor/kwin-master/src/**` | This checkout is a Wayland-era snapshot (`main_wayland.cpp`, no `main_x11.cpp`); searches for `KSelectionOwner`, `selectionOwner`, `XQueryTree` and X11 `BadWindow` startup handling returned no X11 WM matches. No kwin X11 precedent can be cited from this corpus — uncertainty recorded, not inferred. |

## Concrete deltas worth porting

1. **PORTED — skip clients that die during the adoption type query.** Reference: icewm
   `.vendor/icewm-master/src/wmapp.cc:779-781` (swallow `BadWindow` from `GetWindowAttributes`),
   icewm `.vendor/icewm-master/src/wmmgr.cc:1229-1326` (server grab plus per-client cleanup).
   FlameWM before: `wm/manage.rs:24` (`window_kind(...)?`) turned a `GetProperty` `BadWindow` into a
   fatal `scan` failure via `wm/event_loop.rs:86-87`. After: `wm/manage.rs:24-31` routes it through the
   existing `wm/mod.rs:57-73` helper; `wm/tests.rs:63-77` already covers that helper. No gate loosened:
   only destroyed-window error kinds are skipped; `Access`, `Value`, connection errors and
   `IdsExhausted` still abort.

2. **CANDIDATE — explicit `WM_S<n>` ownership check with identity and replace hint.** Reference:
   xfwm4 `.vendor/xfwm4-master/src/screen.c:90-122` and icewm
   `.vendor/icewm-master/src/wmapp.cc:79-117`. FlameWM
   `wm/construct.rs:209-218,271-277` only infers a second WM from `BadAccess` on the root event-mask
   request and never owns/verifies `WM_S<n>`. A future change should query the owner before claiming
   the root, read the incumbent's `_NET_WM_NAME` for the message, and fail closed when ownership
   verification fails. Not ported here: it needs a new atom, a `--replace` policy decision and an X
   server to validate, so it exceeds a 3-edit, UNBUILT slice.

3. **CANDIDATE — grab the server for the whole startup scan.** Reference: xfwm4
   `.vendor/xfwm4-master/src/client.c:2178-2204`, icewm
   `.vendor/icewm-master/src/wmmgr.cc:1229-1326`. FlameWM `wm/manage.rs:4-56` only grabs inside
   `manage()` (`manage.rs:435` in the structural transaction). A whole-scan grab removes the race
   class instead of filtering it, but nests with the inner grab and changes request ordering; needs
   isolated-X evidence, so it remains a spec.

4. **CANDIDATE — report extension versions and a minimal supported version.** Reference: xfwm4
   `.vendor/xfwm4-master/src/display.c:227-230,244-269` (records `shape_version`, rejects too-old
   SYNC). FlameWM `construct.rs:62-88,101-111` records only `present` and a blast radius. Adding a
   version check would widen `ExtensionQuery` and its tests; no active FlameWM path needs a minimum
   version today, so no change.

5. **ALREADY AHEAD — keep the query-failure distinction.** xfwm4/icewm only report a boolean absence;
   FlameWM `construct.rs:72-88` distinguishes `Absent` from `Unknown(query error)`, and
   `construct.rs:219-225` stops before extension queries when the connection is lost so no false
   "absent" records appear. Preserve this if deltas 2-4 are implemented.

## Edit record (UNBUILT)

| file:line | finding | change | risk | status |
| --- | --- | --- | --- | --- |
| `crates/flamewm-wm-x11/src/wm/manage.rs:24-31` | `_NET_WM_WINDOW_TYPE` query during `scan_existing` propagated `BadWindow` and aborted startup. | Route the `ReplyError` through `ignore_destroyed_window_race` and `continue`; other errors still propagate. | If the helper's error-kind set is too broad, a genuinely bad scan error could be skipped; it is the same set already used for events and post-outcome scan errors. No visual or protocol-visibility change. | UNBUILT |

No test file was changed: the reused helper already has adjacent coverage at
`crates/flamewm-wm-x11/src/wm/tests.rs:45-78`, including a `GetProperty` `X11Error` case.

## Gaps and residual risks

- This is static source evidence only. No `git`, `cargo`, build, test or isolated-X run was performed;
  the edit is UNBUILT and no runtime claim is made.
- The `WM_S<n>` and whole-scan-grab deltas remain unported; interoperability with a second WM still
  relies on the root `BadAccess` path.
- kwin X11 precedent is unavailable in this vendor snapshot; do not infer KWin X11 behavior from it.
- Reference paths are evidence only, never dependencies or runtime inputs.
