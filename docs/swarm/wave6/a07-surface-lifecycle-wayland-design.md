# a07 compositor surface lifecycle — design

Status: UNBUILT (no git, no cargo, no build and no tests were run by this worker).

## Problem

`State` holds several collections keyed by a surface or a window id. A client surface can be
destroyed without the matching role callback firing, leaving a stale entry that is never reclaimed.
The largest such collection is `surface_outputs`: one `HashMap<ObjectId, String>` entry per
toplevel, popup and layer surface. Entries are written in `handlers.rs:308` (new toplevel), `:361`
(parent change), `:380` (new popup) and `:602` (new layer surface), and removed only in
`handlers.rs:374` (popup destroyed), `:504` (toplevel destroyed), `:615` (layer destroyed) and
`state/outputs.rs:115` (popup on an output being retired). There is no liveness sweep, so any
surface that dies before its role-destroy callback leaves its home-output id in the map forever.

## Every stale-entry place surveyed

| Collection | Where written | Where removed | Verdict |
| --- | --- | --- | --- |
| `surface_outputs` (`state/mod.rs:191`) | `handlers.rs:308,361,380,602` | `handlers.rs:374,504,615`, `state/outputs.rs:115` | **Leak**: no liveness sweep; fixed here |
| `toplevels` (`state/mod.rs:170`) | `handlers.rs:309` | `handlers.rs:505` (`retain`) | Clean on role destroy |
| `foreign_toplevel_handles` (`state/mod.rs:173`) | `state/mod.rs:485` | `state/mod.rs:509` | Clean on role destroy |
| `titles` (`state/mod.rs:272`) | `xwayland.rs:567` (X11 only) | `xwayland.rs:759` | Clean; Wayland windows never insert |
| `x11` (`state/mod.rs:270`) | `xwayland.rs:568` | `xwayland.rs:757` | Clean |
| `unmanaged` (`state/mod.rs:274`) | `xwayland.rs:576+` | `xwayland.rs:745-746` (`retain`) | Clean |
| `locked_outputs` (`state/mod.rs:126`) | `session_lock.rs:188` | `session_lock.rs:280-308`, called `render/mod.rs:844` | Clean on the nested paint path |
| `popups` / layer maps | Smithay `PopupManager` / `layer_map_for_output` | Smithay role-destroy bookkeeping | Owned by Smithay |
| `decorations` (`render/mod.rs:355`) | titlebar/outline raster | `render/mod.rs:356` (`retain`) | Clean each scene build |
| `screencopy_pending` (`state/mod.rs:133`) | `screencopy.rs:251` | `serve`/`fail` each render | Clean each paint |
| `pending_dmabufs` (`state/mod.rs:144`) | `handlers.rs:283` | `dmabuf::settle_pending` each paint | Clean each paint |

Minor id-keyed scalars `kbd_grab` and `pending_configure` are not cleared in `toplevel_destroyed`.
They are single `Option`s, are consumed or become a no-op on the next release/paint, and are outside
the brief's HashMap/Vec-of-surfaces scope, so they were left unchanged.

## Change (three production edits)

1. `crates/flamewm-wm-wayland/src/output_layout.rs`: add `SurfaceOutputs::retain_keys`, a generic
   predicate-based retain beside the existing `retain_outputs`; no new types or deps.
2. `crates/flamewm-wm-wayland/src/state/outputs.rs`: add `State::prune_dead_surface_outputs`, which
   resolves each key with `WlSurface::from_id` against the cached `DisplayHandle` and keeps only
   surfaces that are still `alive`.
3. `crates/flamewm-wm-wayland/src/render/mod.rs`: call the prune on the nested paint path next to
   the existing `prune_dead_locks` sweep.

Tests (in `output_layout.rs`): dead ids are dropped, live ids keep their home output, and a second
sweep is idempotent. The test uses the already-tested `SurfaceOutputs<u32>` seam, so it needs no
Wayland display.

## Reference-first record

- `.vendor/smithay-master/src/desktop/space/mod.rs:427`: `Space::refresh` starts with
  `self.elements.retain(|e| e.alive())`. Choice ported: reclaim dead surface entries on a regular
  refresh rather than relying only on explicit destroy callbacks. `Space` is not a dependency here;
  only the semantic rule is reused.
- `.vendor/cosmic-epoch/cosmic-comp/src/shell/mod.rs:1675` and `:2763`: long-lived surface
  collections are kept with `retain(|s| s.alive())`. Choice ported: one predicate-based retain is
  enough for the whole map, matching the existing `retain_outputs` shape.

## Visuals

Pixel-identical: the sweep only removes dead keys from `surface_outputs`. No live surface's home
output, scale, enter/leave state, geometry or element list changes, and no element is added,
removed or reordered.

## Risks

- The sweep is called from the nested (`render_into`) paint path, where `prune_dead_locks` already
  lives; the DRM path does not call it. DRM windows use the same map, so a client that dies in a DRM
  session keeps its entry until a nested paint. Adding the call to `drm/mod.rs` needed a fourth
  production edit and was left to the orchestrator.
- `WlSurface::from_id` is a Smithay/wayland-server API confirmed in the vendored corpus but not
  previously used in this crate; it is marked UNBUILT because cargo is forbidden by the brief. If
  the resolved API differs, the single expression in `prune_dead_surface_outputs` is the fix point.
- An idle compositor paints nothing, so a stale entry survives until the next paint. It is bounded
  rather than permanent, which is the intended behavior.

## Verification

Not run by this worker (brief forbids git, cargo and builds). The orchestrator must run the crate's
tests and a build before treating these edits as verified.
