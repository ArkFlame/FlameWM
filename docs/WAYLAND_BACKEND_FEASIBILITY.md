# Wayland Backend Feasibility

Status: feasibility analysis only. This document does not claim Wayland support, does not
schedule Wayland work, and does not delay the X11 release. Source truth: the file references
below, checked against the tree at 2026-10-14. Governing rules: `docs/ARCHITECTURE.md` (one owner
per mechanism; `flamewm-reactor` owns native event sources; UI never becomes WM authority) and
`AGENTS.md`.

## 1. Current state (verified)

X11 coupling was counted with word-boundary identifier matching (`grep -P` with
`(?<![A-Za-z0-9_])` / `(?![A-Za-z0-9_])` guards) for `x11rb`, `xcb`, `XWindow`, `xproto` and `_NET_`
on lines under `crates/*/src/**/*.rs`, excluding lines whose first token is `//`. The count is the
number of files with at least one match. `_NET_` matches include atom-name string literals.
Substring matches on crate paths (`flamewm_ui_x11`) and hex literals (`0xcbf29ce…`) are not counted.

| Class | Crates (files with word-boundary X11 matches) |
|---|---|
| No matches | `flamewm-platform`, `flamewm-reactor`, `flamewm-ui-core`, `flamewm-ui`, `flamewm-ui-x11`, `flamewm-window-core`, `flamewm-shell`, `flamewm-shell-core`, `flamewm-integrations-core`, `flamewm-integrations-linux`, `flamewm-desktop`, `flamewm-desktop-core`, `flamewm-lock`, `flamewm-settings`, `flamewm-settings-core`, `flamewm-api`, `flamewm-render-core`, and all other `crates/*` not listed below |
| X11-coupled | `flamewm-x11-canary` (11 files), `flamewm-wm-x11` (9), `flamewm-render-x11` (4), `flamewm-xsettings` (2), `flamewm-x11-desktop` (2), `flamewm-wm` (1) |

The `x11`/`X11`/`xlib` module and type names are not in this token set, so crates whose only
coupling uses those names (for example `flamewm-ui-x11`, `flamewm-lock`) show no matches here.

Corrections to the earlier count: under word-boundary matching `flamewm-shell` (previously 18
files), `flamewm-render-x11` (previously 31), `flamewm-wm-x11` (previously 15) and the other
prior counts are superseded by the table above. `flamewm-render-core` has no matches; its former
match was the hex literal `0xcbf29ce…` (`src/lib.rs:42`), which the word-boundary rule rejects.

Wayland: zero references (`wayland|smithay|layer-shell|xdg-shell`) under `crates/` and `docs/`,
excluding this document.
No Wayland dependency exists in the workspace.

**Correction to the brief: a window-management port already exists.** `crates/flamewm-api/src/ports.rs`
defines `WindowPort` (L59), `WorkspacePort` (L166), `DisplayPort` (L183), `ShortcutPort` (L190),
`WorkAreaPort` (L196), `MainLoopPort` (L213), `InputPort` (L231), `ApplicationPort` (L235),
`SessionPort` (L241), `BackgroundPort` (L246), `TrayPort` (L251), and the aggregate `EnginePorts`
(L264). `flamewm-platform/src/host.rs:41` is the `PlatformHost<E: EnginePorts>` struct (impl at
L57). `WindowPort` implementors are `flamewm-wm-x11/src/lib.rs` (L367) and
`flamewm-x11-desktop/src/lib.rs` (L655); `DisplayPort` implementors are `flamewm-wm-x11/src/lib.rs`
(L898) and `flamewm-x11-desktop/src/lib.rs` (L1081); and test
fakes (`flamewm-wm-x11/src/runtime.rs` L1062, `flamewm-control-core/tests/platform_control.rs` L100).
Therefore the seam is an extension of `EnginePorts`, not a new invention.

Other existing traits (not window-management): `flamewm-reactor/src/lib.rs:241` `WallClockSource`;
`flamewm-platform/src/services/` (`display.rs`, `windows.rs`, `workspaces.rs`, `panels.rs`,
`session.rs`, ...); `flamewm-lock/src/x11.rs:143` `DpmsCalls`, `src/pam.rs:79` `PamSession`,
`src/power.rs:19` `PowerPort`; `flamewm-x11-desktop/src/shortcuts.rs` `KeycodeLookup` (L165),
`GrabBackend` (L260); `flamewm-xsettings/src/manager.rs:57` `ManagerPort`, `src/lib.rs:210`
`SelectionOwner`; `flamewm-render-x11/src/external_decoration.rs:460` `CursorThemeOwner`;
`flamewm-ui-x11/src/template.rs:37` `UiDocumentAccess`; `flamewm-shell/src/taskbar/tasks/icon_geometry.rs:37`
`IconGeometrySink`.

The reactor is calloop-backed (`flamewm-reactor/Cargo.toml` L10; `src/lib.rs` `FdReadiness` L20,
`Reactor` L49). Native fds enter through it.

## 2. Ownership matrix

| Wayland concern | Current owner (source) | Needed seam |
|---|---|---|
| xdg-shell toplevel | `flamewm-wm-x11` frame/client/classifier; WM authority per ARCHITECTURE §2 | Existing `WindowPort`/`WindowSnapshot`; a Wayland provider implements it. Needs toplevel lifecycle and configure/ack mapping to `WindowFeature` |
| xdg-shell popup | Shell popups: `flamewm-shell/src/popup_controller.rs`, `popup_role.rs`, drawn via `flamewm-ui-x11` `SurfaceRuntime` | New positioner/anchor contract in the shell surface port; WM never owns popup presentation |
| layer-shell (panel, OSD, desktop) | Shell surfaces in `flamewm-shell` and `flamewm-desktop`; panels via `PanelService` (`flamewm-platform/src/services/panels.rs`) | New `SurfaceRolePort` (layer, anchor, exclusive zone). Maps the existing `WorkAreaPort` reservations to exclusive zones |
| session lock (ext-session-lock) | `flamewm-lock/src/x11.rs` (root grabs, key decoding), `pam.rs`, `power.rs` | Lock UI behind `PamSession`/`PowerPort`; a Wayland lock provider replaces only the grab/surface layer |
| outputs/scale (wl_output, xdg-output) | `flamewm-platform/src/display.rs`, `DisplayPort`, `flamewm-platform/src/scale.rs` | Existing `DisplayPort`/`DisplaySnapshot`; provider supplies output geometry and scale events |
| input seat / XKB | `InputPort` (`flamewm-api/src/input.rs`), `flamewm-x11-desktop/src/shortcuts.rs` (`KeycodeLookup`, `GrabBackend`) | Keymap/seat provider implementing `KeycodeLookup`-equivalent; `GrabBackend` maps to compositor key bindings |
| data-device clipboard and DnD | None. X11 selections exist only in `flamewm-xsettings/src/lib.rs` (`SelectionOwner`) | New `ClipboardPort`/`DragPort`; no current owner, so no duplication risk |
| XWayland | None. A compatibility path would live in `flamewm-wm-x11` | Explicitly a non-goal for the first phases (see §5) |
| activation/focus | `flamewm-wm-x11` (focus/activation, ARCHITECTURE §2); `WindowPort` | Activation token handling inside the provider; WM remains sole focus authority |
| renderer and damage/buffers | `flamewm-render-x11` (xcomposite, xrender, xlib); backend-neutral model in `flamewm-render-core` | New render backend (wl_shm or dmabuf) consuming `flamewm-render-core` paint output; `flamewm-render-core` is already X11-free |
| sessions, login, security | `flamewm-session-core`, `SessionPort` (`flamewm-api/src/ports.rs` L241), `flamewm-lock` | Existing `SessionPort`; no Wayland-specific change in the first phases |
| native event sources | `flamewm-reactor` (calloop) | Wayland display fd registered through `Reactor`; no second event loop |

## 3. Proposed seam (DESIGN, not existing API)

Proposal only; nothing below exists in the tree. It extends the existing `EnginePorts` rather than
replacing it.

```rust
// Design sketch. Names are proposals.
pub trait WindowManagerBackend: EnginePorts {
    fn kind(&self) -> BackendKind;              // X11 | Wayland
    fn surfaces(&self) -> &dyn SurfaceRolePort; // layer-shell roles, popups, lock surface
    fn clipboard(&self) -> &dyn ClipboardPort;  // selections and DnD
    fn render_target(&self) -> RenderTargetKind; // XRender | Shm | Dmabuf
}
```

Rules: `flamewm-platform` (`PlatformHost`) stays generic over the trait. Shell/UI crates depend only on
`SurfaceRolePort`, never on a backend. The backend remains the only WM authority.

## 4. Phased milestones (each with its test)

1. **P0, ruling and guard.** Record in ARCHITECTURE that `EnginePorts` is the WM seam. Test: a
   `cargo tree -p flamewm-platform -p flamewm-api -p flamewm-reactor` check that prints no `x11rb`,
   `x11`, or `xcb` edges. Executed evidence required before any claim.
2. **P1, conformance tests.** Run one shared conformance suite against `FakeEngine` and the
   `X11Desktop` adapter for `WindowPort`/`DisplayPort`. Test: existing `flamewm-wm-x11` runtime tests
   pass unchanged.
3. **P2, surface role port.** Add `SurfaceRolePort` with a fake. Test: shell projection tests drive
   the fake with no X11 in the test binary (verified by dependency tree).
4. **P3, Wayland spike outside the default build.** New crate, not a workspace default member, with
   headless smoke test. Test: executed headless run reporting client count and output geometry.
   No release claim.
5. **P4, render and lock providers.** Replace only the render target and lock grab layer behind their
   ports. Test: lock unit tests (`flamewm-lock`) with fake PAM and power ports.

Each milestone is independently revertible. The X11 path is untouched by P0–P2.

## 5. Non-goals

- No Wayland support claim. Nothing here is shipped, tested at runtime, or promised.
- No X11 release delay. X11 milestones and releases proceed independently.
- No XWayland in the first phases.
- No new event loop; `flamewm-reactor` remains the only native event source owner.
- No UI-side WM authority; shell/UI never choose focus, stacking, or workspace state.
- No change to `.vendor/**`, dependency versions, or security settings.

## 6. Risks and open items

- Layer-shell semantics (exclusive zones, keyboard interactivity) are richer than the current
  `WorkAreaPort` reservation model; mapping needs a design review before P2.
- Lock behaviour under ext-session-lock differs from X11 root grabs; P4 needs its own runtime evidence.
- Runtime checks need a real compositor session and are USER-OWNED until executed.

## 7. Files cited

All paths exist in the tree (checked 2026-10-14): `docs/ARCHITECTURE.md`, `AGENTS.md`,
`crates/flamewm-api/src/ports.rs`, `crates/flamewm-api/src/input.rs`, `crates/flamewm-platform/src/host.rs`,
`crates/flamewm-platform/src/display.rs`, `crates/flamewm-platform/src/scale.rs`,
`crates/flamewm-platform/src/lib.rs`, `crates/flamewm-platform/src/services/panels.rs`,
`crates/flamewm-reactor/src/lib.rs`, `crates/flamewm-reactor/Cargo.toml`,
`crates/flamewm-ui-core/src/menu.rs`, `crates/flamewm-shell-core/src/lib.rs`,
`crates/flamewm-desktop-core/src/lib.rs`, `crates/flamewm-render-core/src/lib.rs`,
`crates/flamewm-wm-x11/src/lib.rs`, `crates/flamewm-wm-x11/src/runtime.rs`,
`crates/flamewm-x11-desktop/src/lib.rs`, `crates/flamewm-x11-desktop/src/shortcuts.rs`,
`crates/flamewm-xsettings/src/lib.rs`, `crates/flamewm-xsettings/src/manager.rs`,
`crates/flamewm-lock/src/x11.rs`, `crates/flamewm-lock/src/pam.rs`, `crates/flamewm-lock/src/power.rs`,
`crates/flamewm-shell/src/popup_controller.rs`, `crates/flamewm-shell/src/popup_role.rs`,
`crates/flamewm-shell/src/taskbar/tasks/icon_geometry.rs`,
`crates/flamewm-render-x11/src/external_decoration.rs`, `crates/flamewm-ui-x11/src/template.rs`,
`crates/flamewm-session-core/src/lib.rs`, `crates/flamewm-control-core/tests/platform_control.rs`.
