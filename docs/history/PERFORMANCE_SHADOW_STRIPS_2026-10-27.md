# FlameWM 0.1.2 — X11 shadow memory and selection CPU pass (2026-10-27)

## Scope and plan

Source authority: supplied `flamewm_1105_27_10_26.tar.gz`. Active runtime is Rust, not the historical IceWM C++ code. Changes are restricted to the X11 WM's frame-shadow source and its pure tests. Existing X11 motion-event coalescing, retained renderer invalidation, mouse/window drag mechanics, per-window shape, layout, animations, desktop logic and Wayland compositor remain unchanged.

Source investigation:

- `wm/shadow.rs::refresh_shadow` previously retained an entire BGRX desktop snapshot per floating window, even though the shape excludes all pixels under the window. `shadow::shadow_rect` dimensions are roughly `(width+10)*(height+10)*4` bytes per window.
- `wm/shadow.rs::on_root_rubber_changed` previously recomposed the union bounding rectangle of the old/new desktop selection overlay across every intersecting window shadow. Most of that rectangle could be under the frame and invisible.
- `frame/shadow.rs::shade_shadow` previously multiplied the RGB channels for every pixel in the full rectangular shadow snapshot before the X Shape mask removed the interior.
- Window movement already uses `ShadowAction::Move` without shade/readback. The X11 event pump already merges contiguous motions, and frame-chrome painting is paced. Reimplementing those mechanisms would add risk with no demonstrated benefit.

Implementation sequence:

1. `frame/shadow_rubber.rs`: derive exactly two nonoverlapping right/bottom shadow strips from the already authoritative frame and shadow geometry; preserve the existing full-buffer `compose_region` as the reference oracle.
2. Capture only these strips from the full desktop snapshot into `ShadowBands` after the initial readback; release the full CPU-side unshaded snapshot after upload. The X server pixmap and X Shape remain unchanged.
3. Replace live selection recomposition with only the intersection of existing old/new damage and the visible strips. Maintain precisely the previous pixel values through shared `compose_pixels` logic.
4. Restrict the initial shading pass to the exact X Shape row spans. Leave the pure full-frame shader as the pixel-parity reference.
5. Add Rust unit tests for strip coverage, retained bytes, compositing equivalence for selection entry/move/exit, and per-visible-pixel initial-shading equivalence.

## Resource estimates (not live measurements)

The calculations below estimate only **retained CPU-side raw shadow pixels**. Xorg pixmaps, transient full-frame readback/upload, shared libraries and other processes are excluded. The original shadow geometry adds 10 pixels to each dimension and produces 16-pixel right/bottom visible bands (6px offset plus 10px spread):

| Scenario | Previous retained bytes | New retained bytes | Theoretical reduction |
| --- | ---: | ---: | ---: |
| 1 floating 1280x720 window | 3.59 MiB | 0.12 MiB | 3.47 MiB |
| 5 floating 1280x720 windows | 17.96 MiB | 0.61 MiB | 17.35 MiB |
| 10 floating 1280x720 windows | 35.92 MiB | 1.22 MiB | 34.70 MiB |
| 5 floating 1920x1080 windows | 40.12 MiB | 0.92 MiB | 39.21 MiB |

For a 1280x720 window, shadow-pixel loops traversed by initial shading and worst-case live selection compositing reduce by about 96.6%; for 1920x1080 about 97.7%. **This is a pixel-work reduction, not a promised total CPU percentage.** X11 round trips, uploads, Xorg pixmaps and compositor work remain. Typical idle CPU was already expected to approach zero when the event loops block; no new periodic wakeups are added. A desktop with no eligible floating shadows saves no memory from this patch. Wayland session memory is not expected to change.

## Proof and gaps

- Four in-source Rust tests added, including comparing the original full-frame compositing to compact composition on every repainted visible shadow pixel.
- Independent Python geometry differential ran 120 frame shapes and negative screen origins with 545,526 pixel-coverage checks successfully.
- Selected existing Python benchmark/security tests: 18/18 passed.
- Broader static contract tests: 11/13 passed. Two existing `tools/test_flamewm_pass45_memory_contract.py` tests fail on assertions expecting exact single-line source text in the untouched screenshot implementation. This is unrelated to these changes, and the assertions have not been altered.
- This environment has no `cargo` or `rustc`, and no graphical nested X11 compositor session. The Rust unit tests, compilation, and actual PSS/CPU before-after measurements are **NOT VERIFIED**. No production performance claim is made from estimates alone.

## User-owned verification gate (real Linux machine)

Run an unmodified release benchmark before and after this source pass, using the same monitor/window/settings/workload:

```sh
cargo test -p flamewm-wm-x11 --lib frame::shadow -- --nocapture
cargo build --release --workspace
# In a second terminal, after scripts/xephyr has started on the isolated display:
python3 -I tools/bench-flamewm-cpu.py --display :316 --window 10 --repeat 3 --allow-input --cases idle,rubber-band,window-move
```

The `:316` above is illustrative: use the actual isolated X display reported by `scripts/xephyr`. Never run the XTEST workload against the user's real desktop display. The nested Wayland benchmark `bench-flamewm-session.py` primarily exercises Wayland/XWayland rather than this X11 shadow implementation; use Xephyr with the X11 WM and measure PSS from `/proc/*/smaps_rollup` to validate the claimed memory savings. Acceptance requires identical visible window-shadow pixels, selection dragging, moving/resizing, final selection state, stable idle CPU and decreased X11 WM PSS when floating shadows exist. Test on 24-bit X11/Xorg, multiple monitors (including negative origins), normal/large windows, and shadow on/off.

## Intentionally skipped

- Cache-budget reductions: may increase icon/animation reloads, violating the goal of equal or better operational performance.
- Increasing polling intervals: may delay responsiveness; most inspected hot paths already use event-driven wakeups.
- Wayland compositor memory changes: no source-proven duplication equivalent to X11 shadow buffers was identified in this focused pass.
- Restructuring the X11 pixmap/window representation: could yield further X-server memory savings but requires live visual/shape proof before a safe change.
