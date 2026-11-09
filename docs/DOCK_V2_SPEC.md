# Dock v2 spec (from LordOS / starling / LingmoOS review)

Constraints: no perf/memory regression; blur/surface look unchanged; no per-frame work at idle;
UI never WM authority (state via typed facade in flamewm-shell-core `dock.rs`); DOCK_DESIGN §3 "no animation" stays
the DEFAULT, so every motion item is an opt-in setting, default off.

## Deliverables (each independent, one gate each)
D1 Trash slot (ADAPT LordOS). Last slot after a separator; opens Trash via existing explorer/file-open path; icon swaps
   empty/full by checking `~/.local/share/Trash/files` on dock rebuild + inotify-free: re-check only when dock is
   revealed (no 2 s polling). Settings `dock.showTrash` default true. Files: shell-core dock.rs, ui/shell/dock.html, platform settings.
D2 Drag reorder of pinned items (ADAPT LordOS). Pointer-drag within pinned group; commit on release via existing pin
   facade (`dock.pinned` order); live-preview with `transform: translate` only, no relayout per move. Drag-out beyond
   48 px and release = unpin (cue: icon dims). Tests: pure reorder fn in dock.rs.
D3 Hover icon scale (ADAPT starling/LordOS). Setting `dock.hoverScale` off|subtle(1.15)|macos(1.5 cosine, 150 px radius),
   default off. CSS `transform: scale()` on icon only, transform-origin bottom/edge-aware; pure scale fn in dock.rs
   with tests; <=24 slots. Must produce region damage only (isolated absolute nodes) — verify with damage test.
D4 Auto-hide mode "when covered" (ADAPT LordOS): `dock.autoHideMode` always|covered|never; covered = hide only when a
   window overlaps dock rect. WM computes (x11 wm), publishes bool through existing dock state facade.
D5 Hover name label (ADOPT starling): reuse tooltip path, no new per-frame work.
D6 Glass fill polish (ADAPT): top-lit gradient via existing gradient_background, 1px inner rim token, accent running
   dot with rim (`dockIndicator`/`dockIndicatorRim` tokens). No new shadow/blur. Screenshot compare vs
   /home/linsaftw/Downloads/desktop-overview-dark.png: target radius ~20px, 9px padding, 8px gap at 54px icons scaled to our 44px default.
D7 Launch bounce (ADAPT, opt-in `dock.launchBounce` default off): CSS transform hop 22px/700ms, stops on first window; timer only while pending.
REJECT: live re-layout magnify, poof/bounce-on-remove, per-element blur, QuickJS/React runtime, dock live previews, 2 s polling.

## Order
D6 -> D5 -> D1 -> D2 -> D3 -> D4 -> D7. Each: settings key + Settings UI toggle, tests, `scripts/gate flamewm-shell-core flamewm-shell flamewm-platform` (+wm-x11 for D4), then runtime check on isolated Xvfb (101+), PSS shown vs hidden unchanged within noise, screenshot review at 4 edges.

## Other competitor items queued (not dock)
- LingmoOS: light/dark token pairs; push-based accent/dark updates through settings facade; cached launcher wallpaper blur; async status icons if icon_loader is sync.
- starling process: see docs/competitors/STARLING.md §7. Features: session reattach for terminal, audited default-deny agent broker (evaluate later).
