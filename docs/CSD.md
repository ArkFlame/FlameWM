# Client-side decorations (CSD) in flamewm-wm-x11

Owner: `crates/flamewm-wm-x11/src/decoration/policy.rs` (`decoration_policy`).
Wiring: `wm.rs` (`read_decoration`, `refresh_decoration`, `extents_for`,
`set_frame_extents`, `begin_client_moveresize`), `frame/*` geometry.

## Decision

| Input | Result |
| --- | --- |
| override-redirect, POPUP/DROPDOWN/TOOLTIP/NOTIFICATION, SPLASH, DESKTOP, DOCK | `None` (no Flame frame) |
| `_MOTIF_WM_HINTS` with flag bit 1 and `decorations == 0` | `None` |
| `_GTK_FRAME_EXTENTS` with any non-zero edge | `ClientSide` (no Flame frame) |
| anything else (NORMAL, UTILITY, Motif title-only, zero GTK extents) | `Server` |

Motif title-only (`MWM_DECOR_TITLE`) keeps the server titlebar. `WM_CLASS`
is never consulted.

## Frame and publication

- `Server`: titlebar and 1px border, as before.
- `ClientSide` and `None`: frame equals the client (zero titlebar, zero
  border). Input children are created but left unmapped, so no resize or
  control handles are active. Paint is skipped.
- `_NET_FRAME_EXTENTS`: `[0,0,0,0]` for CSD and `None`, and
  `[border, border, titlebar+border, border]` for `Server`. GTK's own
  `_GTK_FRAME_EXTENTS` stays client-owned.
- PropertyNotify on `_MOTIF_WM_HINTS` or `_GTK_FRAME_EXTENTS` re-evaluates the
  decision. The client area stays fixed and the frame grows or shrinks by the
  titlebar. Nothing is unmapped or remapped except the input children.

## Client requests

- `_NET_WM_MOVERESIZE`: directions 0-7 start the frame edge/corner press,
  direction 8 starts a move, both through the existing frame reducer and
  pointer grab. Keyboard (9-10) and cancel (11) are ignored.
- `WM_CHANGE_STATE` (IconicState), `_NET_WM_STATE` and `_NET_CLOSE_WINDOW` were
  already handled before this change and were not modified.

## Known limits (rulings)

- Snapping and maximize use the window rectangle. GTK's
  `_GTK_FRAME_EXTENTS` is not subtracted, because the client's invisible
  shadow is part of the window, and subtracting it would leave gaps.
- SPLASH windows are classed as unframed and left client-placed; they are
  not centred by the WM.
- A runtime CSD toggle on a maximized window shifts the client by the
  titlebar height, because maximize geometry is not re-planned.

## Native verification

`tools/flamewm_csd_acceptance.py` starts `flamewm` under a private HOME and
D-Bus on a display the operator already started. It refuses `:97`. Its pure
checks are covered by `tools/test_flamewm_csd_acceptance.py`, including
negative controls: titlebar kept on a motif window, extents published for GTK,
no frame for plain windows, no-movement and half-drag moves.

Run on `:124` with the release binary: exit 0. Motif and GTK frames equal the
client with zero extents. Plain has a 31px titlebar with extents
`[1,1,32,1]`. Motif MOVERESIZE moved exactly (120,60). Plain moved (100,60),
within the one-step slack. Screenshot: `root.png` shows no titlebar on the two
CSD windows and the Flame titlebar on the plain one.
